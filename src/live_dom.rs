//! Bounded, clean-room live DOM and event dispatch layer.
//!
//! The parser deliberately keeps a compact tree for document loading. This
//! module owns the mutable, identity-stable form used after a document becomes
//! live. Mutations are coalesced into a dirty render snapshot so layout, paint
//! and accessibility outputs update together without rebuilding on reads.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use crate::accessibility::{self, AccessibilityTree};
use crate::css_parser::CssRule;
use crate::dynamic_render::{DynamicInvalidation, DynamicRenderMetrics, DynamicRenderer};
use crate::layout::LayoutNode;
use crate::paint::DisplayList;
use crate::parser::Element;

const MAX_LIVE_NODES: usize = 50_000;
const MAX_EVENT_LISTENERS: usize = 10_000;
const MAX_EVENT_PATH: usize = 256;
const MAX_TEXT_BYTES: usize = 1024 * 1024;
const MAX_ATTRIBUTE_BYTES: usize = 64 * 1024;
const MAX_SHADOW_ROOTS: usize = 128;
const MAX_SHADOW_HTML_BYTES: usize = 2 * 1024 * 1024;

pub type NodeId = u64;
pub type ListenerId = u64;

/// Shadow-root visibility mode. Closed roots are still rendered, but page
/// script cannot reach them through `element.shadowRoot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowMode {
    Open,
    Closed,
}

/// Shadow-root attachment record. The shadow tree lives in the same node
/// store as the light tree; the shadow root's parent is its host, so event
/// paths and composed exports cross the boundary naturally.
#[derive(Debug, Clone)]
struct ShadowRootRecord {
    host: NodeId,
    root: NodeId,
    mode: ShadowMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveNodeKind {
    Element {
        tag: String,
        attrs: BTreeMap<String, String>,
        is_void: bool,
    },
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub kind: LiveNodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPhase {
    None,
    Capturing,
    AtTarget,
    Bubbling,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomEvent {
    pub event_type: String,
    pub target: NodeId,
    pub current_target: Option<NodeId>,
    pub phase: EventPhase,
    pub bubbles: bool,
    pub cancelable: bool,
    pub default_prevented: bool,
    pub propagation_stopped: bool,
    pub immediate_propagation_stopped: bool,
    pub key: Option<String>,
    pub pointer_x: Option<i32>,
    pub pointer_y: Option<i32>,
    /// Bounded serialized payload for CustomEvent-style host events.
    /// The host bridge stores the JavaScript detail value as its display
    /// string so the DOM layer stays independent of the interpreter.
    pub detail: Option<String>,
    passive_listener: bool,
}

impl DomEvent {
    pub fn new(event_type: impl Into<String>, target: NodeId) -> Self {
        Self {
            event_type: event_type.into(),
            target,
            current_target: None,
            phase: EventPhase::None,
            bubbles: true,
            cancelable: true,
            default_prevented: false,
            propagation_stopped: false,
            immediate_propagation_stopped: false,
            key: None,
            pointer_x: None,
            pointer_y: None,
            detail: None,
            passive_listener: false,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn prevent_default(&mut self) {
        if self.cancelable && !self.passive_listener {
            self.default_prevented = true;
        }
    }

    pub fn stop_propagation(&mut self) {
        self.propagation_stopped = true;
    }

    pub fn stop_immediate_propagation(&mut self) {
        self.immediate_propagation_stopped = true;
        self.propagation_stopped = true;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListenerOptions {
    pub capture: bool,
    pub once: bool,
    pub passive: bool,
}

pub type EventCallback = Arc<dyn Fn(&mut DomEvent) + Send + Sync + 'static>;

#[derive(Clone)]
struct EventListener {
    id: ListenerId,
    event_type: String,
    options: ListenerOptions,
    callback: EventCallback,
}

impl fmt::Debug for EventListener {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output
            .debug_struct("EventListener")
            .field("id", &self.id)
            .field("event_type", &self.event_type)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultAction {
    ToggleChecked(NodeId, bool),
    SubmitForm(NodeId),
    Navigate(String),
    Focus(NodeId),
    InsertText(NodeId, String),
    /// A `<select>` control advanced to a new option (Phase 21 forms).
    SelectOption(NodeId),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DispatchReport {
    pub invoked_listeners: usize,
    pub default_prevented: bool,
    pub default_actions: Vec<DefaultAction>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MutationInvalidation {
    pub style: bool,
    pub layout: bool,
    pub paint: bool,
    pub accessibility: bool,
}

impl MutationInvalidation {
    fn all() -> Self {
        Self {
            style: true,
            layout: true,
            paint: true,
            accessibility: true,
        }
    }

    fn is_dirty(self) -> bool {
        self.style || self.layout || self.paint || self.accessibility
    }
}

#[derive(Debug, Clone)]
pub struct LiveRenderState {
    pub dom: Element,
    pub layout: Option<LayoutNode>,
    pub display_list: DisplayList,
    pub accessibility: AccessibilityTree,
    pub revision: u64,
    pub mutation_count: usize,
    /// Cache/reuse and frame-budget accounting from the Phase 14 renderer.
    pub dynamic: DynamicRenderMetrics,
}

impl Default for LiveRenderState {
    fn default() -> Self {
        let dom = Element::new("root");
        Self {
            accessibility: accessibility::build_tree(&dom),
            dom,
            layout: None,
            display_list: DisplayList::default(),
            revision: 0,
            mutation_count: 0,
            dynamic: DynamicRenderMetrics::default(),
        }
    }
}

/// Mutable document with stable `NodeId` values. IDs survive attributes/text
/// changes, focus transitions and rendering refreshes; a node receives a new
/// ID only when it is newly created.
#[derive(Debug)]
pub struct LiveDocument {
    root: NodeId,
    nodes: BTreeMap<NodeId, LiveNode>,
    listeners: BTreeMap<NodeId, Vec<EventListener>>,
    next_node_id: NodeId,
    next_listener_id: ListenerId,
    focused: Option<NodeId>,
    invalidation: MutationInvalidation,
    render: LiveRenderState,
    css_rules: Vec<CssRule>,
    viewport_width: u32,
    dynamic_renderer: DynamicRenderer,
    shadows: BTreeMap<NodeId, ShadowRootRecord>,
    /// Canvas 2D shapes drawn by the page, keyed by the canvas element.
    canvas_shapes: BTreeMap<NodeId, Vec<crate::paint::VectorShape>>,
}

impl LiveDocument {
    pub fn from_element(root: &Element, css_rules: Vec<CssRule>, viewport_width: u32) -> Self {
        let mut document = Self {
            root: 1,
            nodes: BTreeMap::new(),
            listeners: BTreeMap::new(),
            next_node_id: 1,
            next_listener_id: 1,
            focused: None,
            invalidation: MutationInvalidation::all(),
            render: LiveRenderState::default(),
            css_rules,
            viewport_width: viewport_width.max(1),
            dynamic_renderer: DynamicRenderer::new(),
            shadows: BTreeMap::new(),
            canvas_shapes: BTreeMap::new(),
        };
        let root_id = document
            .import_element(root, None)
            .expect("root fits empty document");
        document.root = root_id;
        document.refresh();
        document
    }

    pub fn parse(html: &str, css_rules: Vec<CssRule>, viewport_width: u32) -> Self {
        Self::from_element(&crate::parser::parse_html(html), css_rules, viewport_width)
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn focused(&self) -> Option<NodeId> {
        self.focused
    }

    pub fn node(&self, node: NodeId) -> Option<&LiveNode> {
        self.nodes.get(&node)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn invalidation(&self) -> MutationInvalidation {
        self.invalidation
    }

    pub fn viewport_width(&self) -> u32 {
        self.viewport_width
    }

    pub fn render_state(&self) -> &LiveRenderState {
        &self.render
    }

    pub fn refresh(&mut self) -> &LiveRenderState {
        if !self.invalidation.is_dirty() {
            // A clean frame is served directly from the retained display list.
            self.render.dynamic.display_list_reused = true;
            self.render.dynamic.frame_time_ms = 0;
            return &self.render;
        }
        let dom = self.to_element();
        let dynamic = self.dynamic_renderer.refresh(
            &dom,
            &self.css_rules,
            self.viewport_width,
            DynamicInvalidation {
                style: self.invalidation.style,
                layout: self.invalidation.layout,
                paint: self.invalidation.paint,
            },
        );
        let accessibility = accessibility::build_tree(&dom);
        let mut display_list = dynamic.display_list;
        self.append_page_shapes(&dom, &dynamic.layout, &mut display_list);
        self.render = LiveRenderState {
            dom,
            layout: dynamic.layout,
            display_list,
            accessibility,
            revision: self.render.revision.saturating_add(1),
            mutation_count: self.render.mutation_count.saturating_add(1),
            dynamic: dynamic.metrics,
        };
        self.invalidation = MutationInvalidation::default();
        &self.render
    }

    /// Append Canvas 2D shapes (from the host bridge) and SVG shape elements
    /// (from the composed DOM) to the display list at their layout position.
    fn append_page_shapes(
        &self,
        dom: &Element,
        layout: &Option<LayoutNode>,
        display_list: &mut DisplayList,
    ) {
        let mut shape_count = 0usize;
        for (node, shapes) in &self.canvas_shapes {
            if shape_count >= 512 {
                break;
            }
            let Some((x, y)) = self.layout_position(*node, layout) else {
                continue;
            };
            for shape in shapes.iter().take(256) {
                let mut item = shape.clone();
                item.x += x;
                item.y += y;
                display_list
                    .items
                    .push(crate::paint::DisplayItem::VectorShape(item));
                shape_count += 1;
            }
        }
        collect_svg_shapes(dom, display_list, layout, &mut shape_count);
    }

    fn layout_position(&self, node: NodeId, layout: &Option<LayoutNode>) -> Option<(f32, f32)> {
        fn visit(layout: &LayoutNode, node: NodeId) -> Option<(f32, f32)> {
            if layout.element.node_id == Some(node) {
                return Some((layout.rect.x as f32, layout.rect.y as f32));
            }
            for child in &layout.children {
                if let Some(found) = visit(child, node) {
                    return Some(found);
                }
            }
            None
        }
        layout.as_ref().and_then(|layout| visit(layout, node))
    }

    /// Retrieve the computed layout rect of a node if present in the current layout tree.
    pub fn node_rect(&self, node: NodeId) -> Option<crate::layout::RectModel> {
        fn visit(layout: &LayoutNode, node: NodeId) -> Option<crate::layout::RectModel> {
            if layout.element.node_id == Some(node) {
                return Some(layout.rect);
            }
            for child in &layout.children {
                if let Some(found) = visit(child, node) {
                    return Some(found);
                }
            }
            None
        }
        self.render
            .layout
            .as_ref()
            .and_then(|layout| visit(layout, node))
    }

    /// Record a Canvas 2D shape drawn through the host bridge.
    pub fn add_canvas_shape(
        &mut self,
        canvas: NodeId,
        shape: crate::paint::VectorShape,
    ) -> Result<(), String> {
        self.require_node(canvas)?;
        let entries = self.canvas_shapes.entry(canvas).or_default();
        if entries.len() >= 256 {
            return Err("QuotaExceededError: canvas shape budget exceeded".to_string());
        }
        entries.push(shape);
        self.invalidation.paint = true;
        Ok(())
    }

    /// Clear shapes drawn on a canvas (`clearRect` full clear or reset).
    pub fn clear_canvas_shapes(&mut self, canvas: NodeId) -> Result<(), String> {
        self.require_node(canvas)?;
        self.canvas_shapes.remove(&canvas);
        self.invalidation.paint = true;
        Ok(())
    }

    /// Read-only access used by the bounded Canvas 2D pixel-readback bridge.
    /// Coordinates remain canvas-local here; document offsets are applied only
    /// when the display list is assembled.
    pub fn canvas_shapes(&self, canvas: NodeId) -> &[crate::paint::VectorShape] {
        self.canvas_shapes
            .get(&canvas)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Advance the bounded transition/animation timeline and refresh pixels
    /// without recomputing cascade or layout when only visual values move.
    pub fn advance_animations(&mut self, elapsed_ms: u64) -> &LiveRenderState {
        if self.dynamic_renderer.advance_animations(elapsed_ms) {
            self.invalidation.paint = true;
        }
        self.refresh()
    }

    pub fn query_selector(&self, selector: &str) -> Option<NodeId> {
        self.query_selector_all(selector).into_iter().next()
    }

    pub fn query_selector_all(&self, selector: &str) -> Vec<NodeId> {
        let parts: Vec<&str> = selector.split_ascii_whitespace().collect();
        if parts.is_empty() || parts.len() > 8 {
            return Vec::new();
        }
        self.document_order()
            .into_iter()
            .filter(|node| self.matches_selector_path(*node, &parts))
            .collect()
    }

    pub fn get_element_by_id(&self, id: &str) -> Option<NodeId> {
        self.query_selector(&format!("#{}", id))
    }

    pub fn create_element(&mut self, tag: &str) -> Result<NodeId, String> {
        let tag = normalize_tag(tag)?;
        self.allocate(LiveNodeKind::Element {
            tag,
            attrs: BTreeMap::new(),
            is_void: false,
        })
    }

    pub fn create_text_node(&mut self, text: &str) -> Result<NodeId, String> {
        self.allocate(LiveNodeKind::Text(bounded_text(text)))
    }

    /// Import a parsed element as a detached live subtree. The imported root
    /// Import a parsed element as a detached live subtree. The imported root
    /// receives a fresh identity unless the supplied preferred id is unused.
    /// This is the bridge used by inert template fragments.
    pub fn import_subtree(&mut self, element: &Element) -> Result<NodeId, String> {
        let mut detached = element.clone();
        clear_node_ids(&mut detached);
        let node = self.import_element(&detached, None)?;
        self.mark_mutated();
        Ok(node)
    }

    /// Recursively clone a node and optionally its children.
    pub fn clone_node(&mut self, node: NodeId, deep: bool) -> Result<NodeId, String> {
        let entry = self.require_node(node)?.clone();
        let cloned_id = match entry.kind {
            LiveNodeKind::Element {
                tag,
                attrs,
                is_void,
            } => self.allocate(LiveNodeKind::Element {
                tag,
                attrs,
                is_void,
            })?,
            LiveNodeKind::Text(text) => self.allocate(LiveNodeKind::Text(text))?,
        };
        if deep {
            for child in entry.children {
                let child_clone = self.clone_node(child, true)?;
                self.append_child(cloned_id, child_clone)?;
            }
        }
        self.mark_mutated();
        Ok(cloned_id)
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), String> {
        self.require_element(parent)?;
        self.require_node(child)?;
        if parent == child || self.is_ancestor(child, parent) {
            return Err("HierarchyRequestError: append would create a cycle".to_string());
        }
        self.detach(child)?;
        self.nodes
            .get_mut(&parent)
            .expect("validated parent")
            .children
            .push(child);
        self.nodes.get_mut(&child).expect("validated child").parent = Some(parent);
        self.mark_mutated();
        Ok(())
    }

    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), String> {
        let position = self
            .require_node(parent)?
            .children
            .iter()
            .position(|candidate| *candidate == child)
            .ok_or_else(|| "NotFoundError: node is not a child of parent".to_string())?;
        self.nodes
            .get_mut(&parent)
            .expect("validated parent")
            .children
            .remove(position);
        self.nodes.get_mut(&child).expect("validated child").parent = None;
        self.mark_mutated();
        Ok(())
    }

    pub fn set_text_content(&mut self, node: NodeId, value: &str) -> Result<(), String> {
        self.require_node(node)?;
        let text = bounded_text(value);
        if matches!(self.nodes[&node].kind, LiveNodeKind::Text(_)) {
            self.nodes.get_mut(&node).expect("validated node").kind = LiveNodeKind::Text(text);
        } else {
            let previous = self.nodes[&node].children.clone();
            for child in previous {
                self.nodes.get_mut(&child).expect("existing child").parent = None;
            }
            self.nodes
                .get_mut(&node)
                .expect("validated node")
                .children
                .clear();
            if !text.is_empty() {
                let child = self.create_text_node(&text)?;
                self.append_child_without_invalidation(node, child)?;
            }
        }
        self.mark_mutated();
        Ok(())
    }

    pub fn text_content(&self, node: NodeId) -> Result<String, String> {
        self.require_node(node)?;
        let mut output = String::new();
        self.collect_text(node, &mut output);
        Ok(output)
    }

    pub fn set_attribute(&mut self, node: NodeId, name: &str, value: &str) -> Result<(), String> {
        let name = normalize_attribute_name(name)?;
        let element = self.require_element_mut(node)?;
        let LiveNodeKind::Element { attrs, .. } = &mut element.kind else {
            unreachable!("require_element_mut returned text node")
        };
        attrs.insert(name.clone(), bounded_attribute(value));
        self.mark_attribute_mutated(name.as_str(), value);
        Ok(())
    }

    pub fn remove_attribute(&mut self, node: NodeId, name: &str) -> Result<(), String> {
        let name = normalize_attribute_name(name)?;
        let element = self.require_element_mut(node)?;
        let LiveNodeKind::Element { attrs, .. } = &mut element.kind else {
            unreachable!("require_element_mut returned text node")
        };
        attrs.remove(&name);
        self.mark_attribute_mutated(name.as_str(), "");
        Ok(())
    }

    pub fn get_attribute(&self, node: NodeId, name: &str) -> Option<&str> {
        let LiveNodeKind::Element { attrs, .. } = &self.nodes.get(&node)?.kind else {
            return None;
        };
        attrs.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    /// Attach a shadow root to an element (Phase 21 composed Shadow DOM).
    /// The shadow tree shares the node store; the root's parent is the host,
    /// so event paths and composed exports cross the boundary naturally.
    pub fn attach_shadow(&mut self, host: NodeId, mode: ShadowMode) -> Result<NodeId, String> {
        if self.shadows.len() >= MAX_SHADOW_ROOTS {
            return Err("QuotaExceededError: shadow-root budget exceeded".to_string());
        }
        let element = self.require_element(host)?;
        if matches!(&element.kind, LiveNodeKind::Element { tag, .. } if tag == "shadow-root") {
            return Err("NotSupportedError: shadow root is not a valid host".to_string());
        }
        if self.shadows.contains_key(&host) {
            return Err("NotSupportedError: host already has a shadow root".to_string());
        }
        let root = self.allocate(LiveNodeKind::Element {
            tag: "shadow-root".to_string(),
            attrs: BTreeMap::new(),
            is_void: false,
        })?;
        self.append_child_without_invalidation(host, root)?;
        self.shadows
            .insert(host, ShadowRootRecord { host, root, mode });
        self.mark_mutated();
        Ok(root)
    }

    /// Replace the children of a shadow root with parsed HTML (the
    /// `shadowRoot.innerHTML = ...` path). Bounded by the HTML byte cap.
    pub fn set_shadow_html(&mut self, root: NodeId, html: &str) -> Result<(), String> {
        if html.len() > MAX_SHADOW_HTML_BYTES {
            return Err("QuotaExceededError: shadow markup exceeds 2 MB".to_string());
        }
        let shadow = self
            .shadows
            .values()
            .find(|record| record.root == root)
            .cloned()
            .ok_or_else(|| "NotFoundError: node is not a shadow root".to_string())?;
        let previous = self.nodes[&root].children.clone();
        for child in previous {
            self.nodes.get_mut(&child).expect("existing child").parent = None;
        }
        self.nodes
            .get_mut(&root)
            .expect("validated root")
            .children
            .clear();
        let parsed = crate::parser::parse_html(html);
        self.append_parsed_fragment(&parsed, root)?;
        let _ = shadow;
        self.mark_mutated();
        Ok(())
    }

    /// Append a parsed fragment under a parent, handling the parser's
    /// single-top-level-element unwrap (`parse_html` returns the first
    /// element directly instead of wrapping it in a synthetic root).
    fn append_parsed_fragment(&mut self, parsed: &Element, parent: NodeId) -> Result<(), String> {
        if matches!(parsed.tag.as_str(), "root" | "html" | "body") {
            for child in &parsed.children {
                let node = self.import_subtree(child)?;
                self.append_child_without_invalidation(parent, node)?;
            }
        } else {
            let node = self.import_subtree(parsed)?;
            self.append_child_without_invalidation(parent, node)?;
        }
        if !parsed.text.is_empty() {
            let text = self.create_text_node(&parsed.text)?;
            self.append_child_without_invalidation(parent, text)?;
        }
        Ok(())
    }

    /// Replace the children of any element (or shadow root) with parsed HTML,
    /// used by the `innerHTML` property path. Bounded by the shadow HTML cap.
    pub fn set_inner_html(&mut self, node: NodeId, html: &str) -> Result<(), String> {
        if html.len() > MAX_SHADOW_HTML_BYTES {
            return Err("QuotaExceededError: innerHTML markup exceeds 2 MB".to_string());
        }
        self.require_node(node)?;
        let previous = self.nodes[&node].children.clone();
        for child in previous {
            self.nodes.get_mut(&child).expect("existing child").parent = None;
        }
        self.nodes
            .get_mut(&node)
            .expect("validated node")
            .children
            .clear();
        let parsed = crate::parser::parse_html(html);
        self.append_parsed_fragment(&parsed, node)?;
        self.mark_mutated();
        Ok(())
    }

    /// The shadow root attached to a host, honoring closed-mode visibility.
    pub fn shadow_root(&self, host: NodeId) -> Option<NodeId> {
        self.shadows
            .get(&host)
            .filter(|record| record.mode == ShadowMode::Open)
            .map(|record| record.root)
    }

    /// Whether a node is a shadow root of the given mode (internal use).
    pub fn shadow_root_mode(&self, node: NodeId) -> Option<ShadowMode> {
        self.shadows
            .values()
            .find(|record| record.root == node)
            .map(|record| record.mode)
    }

    /// Event-path retargeting: when `current` lies outside a shadow root that
    /// contains `target`, page-visible listeners must observe the host as the
    /// target (the shadow boundary hides inner nodes).
    pub fn effective_target(&self, target: NodeId, current: NodeId) -> NodeId {
        let mut observed = target;
        let mut cursor = Some(target);
        while let Some(node) = cursor {
            if node == current {
                break;
            }
            // Crossing a shadow root boundary retargets to its host.
            if let Some(record) = self.shadows.values().find(|record| record.root == node) {
                observed = record.host;
            }
            cursor = self.nodes.get(&node).and_then(|entry| entry.parent);
        }
        observed
    }

    /// The composed event path (including shadow-boundary hosts), bounded by
    /// `MAX_EVENT_PATH`.
    pub fn composed_path(&self, target: NodeId) -> Result<Vec<NodeId>, String> {
        let mut path = Vec::new();
        let mut current = Some(target);
        while let Some(node) = current {
            if path.len() >= MAX_EVENT_PATH {
                return Err("QuotaExceededError: event path depth exceeded".to_string());
            }
            path.push(node);
            current = self.nodes.get(&node).and_then(|entry| entry.parent);
        }
        Ok(path)
    }

    pub fn add_event_listener(
        &mut self,
        node: NodeId,
        event_type: &str,
        options: ListenerOptions,
        callback: EventCallback,
    ) -> Result<ListenerId, String> {
        self.require_node(node)?;
        if self.listener_count() >= MAX_EVENT_LISTENERS {
            return Err("QuotaExceededError: event listener budget exceeded".to_string());
        }
        let event_type = normalize_event_type(event_type)?;
        let id = self.next_listener_id;
        self.next_listener_id = self
            .next_listener_id
            .checked_add(1)
            .ok_or_else(|| "Event listener identifier space exhausted".to_string())?;
        self.listeners.entry(node).or_default().push(EventListener {
            id,
            event_type,
            options,
            callback,
        });
        Ok(id)
    }

    pub fn remove_event_listener(&mut self, node: NodeId, listener: ListenerId) -> bool {
        let Some(listeners) = self.listeners.get_mut(&node) else {
            return false;
        };
        let before = listeners.len();
        listeners.retain(|candidate| candidate.id != listener);
        before != listeners.len()
    }

    pub fn listener_count(&self) -> usize {
        self.listeners.values().map(Vec::len).sum()
    }

    pub fn dispatch_event(
        &mut self,
        target: NodeId,
        event: &mut DomEvent,
    ) -> Result<DispatchReport, String> {
        self.require_node(target)?;
        if event.target != target {
            return Err(
                "InvalidStateError: event target does not match dispatch target".to_string(),
            );
        }
        let path = self.event_path(target)?;
        let mut report = DispatchReport::default();

        for node in path.iter().rev().take(path.len().saturating_sub(1)) {
            self.invoke_listeners(*node, event, EventPhase::Capturing, true, &mut report);
            if event.propagation_stopped {
                report.default_prevented = event.default_prevented;
                return Ok(report);
            }
        }
        self.invoke_listeners(target, event, EventPhase::AtTarget, true, &mut report);
        if !event.immediate_propagation_stopped {
            self.invoke_listeners(target, event, EventPhase::AtTarget, false, &mut report);
        }
        if event.bubbles && !event.propagation_stopped {
            for node in path.iter().skip(1) {
                self.invoke_listeners(*node, event, EventPhase::Bubbling, false, &mut report);
                if event.propagation_stopped {
                    break;
                }
            }
        }
        event.current_target = None;
        event.phase = EventPhase::None;
        report.default_prevented = event.default_prevented;
        Ok(report)
    }

    pub fn click(&mut self, target: NodeId) -> Result<DispatchReport, String> {
        let mut event = DomEvent::new("click", target);
        let mut report = self.dispatch_event(target, &mut event)?;
        if !event.default_prevented {
            report
                .default_actions
                .extend(self.apply_click_default(target)?);
        }
        Ok(report)
    }

    pub fn focus(&mut self, node: NodeId) -> Result<(), String> {
        self.require_node(node)?;
        if !self.is_focusable(node) {
            return Err("InvalidStateError: node is not focusable".to_string());
        }
        if self.focused == Some(node) {
            return Ok(());
        }
        if let Some(previous) = self.focused {
            let mut blur = DomEvent::new("blur", previous);
            blur.bubbles = false;
            let _ = self.dispatch_event(previous, &mut blur)?;
            let mut out = DomEvent::new("focusout", previous);
            let _ = self.dispatch_event(previous, &mut out)?;
        }
        self.focused = Some(node);
        let mut focus = DomEvent::new("focus", node);
        focus.bubbles = false;
        let _ = self.dispatch_event(node, &mut focus)?;
        let mut input = DomEvent::new("focusin", node);
        let _ = self.dispatch_event(node, &mut input)?;
        Ok(())
    }

    pub fn dispatch_keyboard(
        &mut self,
        event_type: &str,
        key: &str,
    ) -> Result<DispatchReport, String> {
        let target = self.focused.unwrap_or(self.root);
        let mut event = DomEvent::new(event_type, target);
        event.key = Some(key.chars().take(128).collect());
        let mut report = self.dispatch_event(target, &mut event)?;
        if event_type == "keydown" && !event.default_prevented {
            report
                .default_actions
                .extend(self.apply_key_default(target, key)?);
        }
        Ok(report)
    }

    pub fn dispatch_pointer(
        &mut self,
        event_type: &str,
        x: i32,
        y: i32,
    ) -> Result<Option<DispatchReport>, String> {
        self.refresh();
        let target = self.hit_test(x as f64, y as f64);
        let Some(target) = target else {
            return Ok(None);
        };
        let mut event = DomEvent::new(event_type, target);
        event.pointer_x = Some(x);
        event.pointer_y = Some(y);
        let report = self.dispatch_event(target, &mut event)?;
        Ok(Some(report))
    }

    pub fn hit_test(&self, x: f64, y: f64) -> Option<NodeId> {
        fn visit(node: &LayoutNode, x: f64, y: f64, found: &mut Option<NodeId>) {
            let rect = &node.rect;
            if rect.display == crate::layout::DisplayType::None
                || x < rect.x
                || y < rect.y
                || x > rect.x + rect.outer_width()
                || y > rect.y + rect.outer_height()
            {
                return;
            }
            if let Some(id) = node.element.node_id {
                *found = Some(id);
            }
            for child in &node.children {
                visit(child, x, y, found);
            }
        }
        let mut found = None;
        if let Some(layout) = self.render.layout.as_ref() {
            visit(layout, x, y, &mut found);
        }
        found.or_else(|| {
            (x >= 0.0
                && y >= 0.0
                && x <= f64::from(self.render.display_list.width)
                && y <= f64::from(self.render.display_list.height))
            .then_some(self.root)
        })
    }

    fn allocate(&mut self, kind: LiveNodeKind) -> Result<NodeId, String> {
        self.allocate_preferred(kind, None)
    }

    fn allocate_preferred(
        &mut self,
        kind: LiveNodeKind,
        preferred: Option<NodeId>,
    ) -> Result<NodeId, String> {
        if self.nodes.len() >= MAX_LIVE_NODES {
            return Err("QuotaExceededError: live DOM node budget exceeded".to_string());
        }
        let id = preferred
            .filter(|candidate| *candidate != 0 && !self.nodes.contains_key(candidate))
            .unwrap_or(self.next_node_id);
        self.next_node_id = self
            .next_node_id
            .max(id)
            .checked_add(1)
            .ok_or_else(|| "DOM node identifier space exhausted".to_string())?;
        self.nodes.insert(
            id,
            LiveNode {
                id,
                parent: None,
                children: Vec::new(),
                kind,
            },
        );
        Ok(id)
    }

    fn import_element(
        &mut self,
        element: &Element,
        parent: Option<NodeId>,
    ) -> Result<NodeId, String> {
        let id = self.allocate_preferred(
            LiveNodeKind::Element {
                tag: element.tag.clone(),
                attrs: element
                    .attrs
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
                is_void: element.is_void,
            },
            element.node_id,
        )?;
        if let Some(parent) = parent {
            self.append_child_without_invalidation(parent, id)?;
        }
        if !element.text.is_empty() {
            let text = self.allocate(LiveNodeKind::Text(bounded_text(&element.text)))?;
            self.append_child_without_invalidation(id, text)?;
        }
        for child in &element.children {
            self.import_element(child, Some(id))?;
        }
        Ok(id)
    }

    fn append_child_without_invalidation(
        &mut self,
        parent: NodeId,
        child: NodeId,
    ) -> Result<(), String> {
        self.require_element(parent)?;
        self.require_node(child)?;
        self.nodes
            .get_mut(&parent)
            .expect("validated parent")
            .children
            .push(child);
        self.nodes.get_mut(&child).expect("validated child").parent = Some(parent);
        Ok(())
    }

    fn detach(&mut self, child: NodeId) -> Result<(), String> {
        let parent = self.require_node(child)?.parent;
        if let Some(parent) = parent {
            let children = &mut self
                .nodes
                .get_mut(&parent)
                .expect("existing parent")
                .children;
            children.retain(|candidate| *candidate != child);
            self.nodes.get_mut(&child).expect("existing child").parent = None;
        }
        Ok(())
    }

    fn to_element(&self) -> Element {
        self.export_element(self.root)
            .unwrap_or_else(|| Element::new("root"))
    }

    /// Export the composed tree (public accessor used by tests and embedders
    /// to inspect what the renderer sees).
    pub fn to_element_public(&self) -> Element {
        self.to_element()
    }

    fn export_element(&self, node: NodeId) -> Option<Element> {
        self.export_element_inner(node, None)
    }

    fn export_element_inner(&self, node: NodeId, shadow_host: Option<NodeId>) -> Option<Element> {
        let live = self.nodes.get(&node)?;
        let LiveNodeKind::Element {
            tag,
            attrs,
            is_void,
        } = &live.kind
        else {
            return None;
        };
        let mut element = Element::new(tag);
        element.node_id = Some(node);
        element.attrs = attrs
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        element.is_void = *is_void;
        if let Some(host) = shadow_host {
            if tag == "slot" {
                // A slot renders its assigned light children (or its own
                // fallback children when nothing is assigned).
                let name = self.get_attribute(node, "name").unwrap_or("").to_string();
                let assigned: Vec<NodeId> = self
                    .nodes
                    .get(&host)?
                    .children
                    .iter()
                    .copied()
                    .filter(|light| {
                        // Shadow roots are tree structure, not light
                        // children, and must never be slotted.
                        if matches!(
                            &self.nodes.get(light).map(|node| &node.kind),
                            Some(LiveNodeKind::Element { tag, .. }) if tag == "shadow-root"
                        ) {
                            return false;
                        }
                        let slot = self.get_attribute(*light, "slot").unwrap_or("");
                        slot == name
                    })
                    .collect();
                if assigned.is_empty() {
                    for child in &live.children {
                        self.export_into(*child, &mut element)?;
                    }
                } else {
                    for light in assigned {
                        self.export_into(light, &mut element)?;
                    }
                }
                return Some(element);
            }
        }
        if let Some(shadow) = self.shadows.get(&node) {
            // Composed tree: the shadow children render in place of the light
            // children, except light children assigned to slots.
            self.export_shadow_children(shadow.root, node, &mut element)?;
        } else if tag == "shadow-root" {
            // A bare shadow root exports nothing (it only appears composed
            // inside its host).
            return None;
        } else {
            for child in &live.children {
                match &self.nodes.get(child)?.kind {
                    LiveNodeKind::Text(text) => element.text.push_str(text),
                    LiveNodeKind::Element { .. } => element
                        .children
                        .push(self.export_element_inner(*child, shadow_host)?),
                }
            }
        }
        Some(element)
    }

    /// Export the composed children of a shadow root into the host element,
    /// replacing `<slot>` elements with their assigned light children (or the
    /// slot's own fallback children when nothing is assigned).
    fn export_shadow_children(
        &self,
        root: NodeId,
        host: NodeId,
        element: &mut Element,
    ) -> Option<()> {
        let live = self.nodes.get(&root)?;
        for child in &live.children {
            match &self.nodes.get(child)?.kind {
                LiveNodeKind::Text(text) => element.text.push_str(text),
                LiveNodeKind::Element { tag, .. } if tag == "slot" => {
                    let name = self.get_attribute(*child, "name").unwrap_or("").to_string();
                    let assigned: Vec<NodeId> = self
                        .nodes
                        .get(&host)?
                        .children
                        .iter()
                        .copied()
                        .filter(|light| {
                            // Shadow roots are tree structure, not light
                            // children, and must never be slotted.
                            if matches!(
                                &self.nodes.get(light).map(|node| &node.kind),
                                Some(LiveNodeKind::Element { tag, .. }) if tag == "shadow-root"
                            ) {
                                return false;
                            }
                            let slot = self.get_attribute(*light, "slot").unwrap_or("");
                            slot == name
                        })
                        .collect();
                    if assigned.is_empty() {
                        self.export_into(*child, element)?;
                    } else {
                        for light in assigned {
                            self.export_into(light, element)?;
                        }
                    }
                }
                LiveNodeKind::Element { .. } => element
                    .children
                    .push(self.export_element_inner(*child, Some(host))?),
            }
        }
        Some(())
    }

    /// Export one node into an existing element (text or a child subtree).
    fn export_into(&self, node: NodeId, element: &mut Element) -> Option<()> {
        match &self.nodes.get(&node)?.kind {
            LiveNodeKind::Text(text) => {
                element.text.push_str(text);
                Some(())
            }
            LiveNodeKind::Element { .. } => {
                element.children.push(self.export_element(node)?);
                Some(())
            }
        }
    }

    fn collect_text(&self, node: NodeId, output: &mut String) {
        let Some(current) = self.nodes.get(&node) else {
            return;
        };
        if let LiveNodeKind::Text(text) = &current.kind {
            output.push_str(text);
        }
        for child in &current.children {
            self.collect_text(*child, output);
        }
    }

    fn require_node(&self, node: NodeId) -> Result<&LiveNode, String> {
        self.nodes
            .get(&node)
            .ok_or_else(|| "NotFoundError: node does not belong to this document".to_string())
    }

    fn require_element(&self, node: NodeId) -> Result<&LiveNode, String> {
        let node = self.require_node(node)?;
        if matches!(node.kind, LiveNodeKind::Element { .. }) {
            Ok(node)
        } else {
            Err("HierarchyRequestError: parent must be an element".to_string())
        }
    }

    fn require_element_mut(&mut self, node: NodeId) -> Result<&mut LiveNode, String> {
        let node = self
            .nodes
            .get_mut(&node)
            .ok_or_else(|| "NotFoundError: node does not belong to this document".to_string())?;
        if matches!(node.kind, LiveNodeKind::Element { .. }) {
            Ok(node)
        } else {
            Err("HierarchyRequestError: node must be an element".to_string())
        }
    }

    /// Document-order node listing used by form serialization and radio-group
    /// activation. Bounded by the live-node budget.
    pub fn document_order(&self) -> Vec<NodeId> {
        let mut output = Vec::new();
        let mut stack = vec![self.root];
        while let Some(node) = stack.pop() {
            output.push(node);
            if let Some(current) = self.nodes.get(&node) {
                stack.extend(current.children.iter().rev().copied());
            }
        }
        output
    }

    fn is_ancestor(&self, candidate: NodeId, node: NodeId) -> bool {
        let mut current = Some(node);
        for _ in 0..MAX_EVENT_PATH {
            let Some(id) = current else { return false };
            if id == candidate {
                return true;
            }
            current = self.nodes.get(&id).and_then(|entry| entry.parent);
        }
        true
    }

    /// The full ancestor chain for a dispatch, used by the host bridge to run
    /// JavaScript listener passes in the same capture/target/bubble order.
    pub fn event_path(&self, target: NodeId) -> Result<Vec<NodeId>, String> {
        let mut path = Vec::new();
        let mut current = Some(target);
        while let Some(node) = current {
            if path.len() >= MAX_EVENT_PATH {
                return Err("QuotaExceededError: event path depth exceeded".to_string());
            }
            path.push(node);
            current = self.require_node(node)?.parent;
        }
        Ok(path)
    }

    fn invoke_listeners(
        &mut self,
        node: NodeId,
        event: &mut DomEvent,
        phase: EventPhase,
        capture: bool,
        report: &mut DispatchReport,
    ) {
        let listeners: Vec<EventListener> = self
            .listeners
            .get(&node)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|listener| {
                        listener.event_type == event.event_type
                            && listener.options.capture == capture
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut remove_once = BTreeSet::new();
        for listener in listeners {
            event.current_target = Some(node);
            event.phase = phase;
            event.passive_listener = listener.options.passive;
            (listener.callback)(event);
            event.passive_listener = false;
            report.invoked_listeners = report.invoked_listeners.saturating_add(1);
            if listener.options.once {
                remove_once.insert(listener.id);
            }
            if event.immediate_propagation_stopped {
                break;
            }
        }
        if !remove_once.is_empty() {
            if let Some(entries) = self.listeners.get_mut(&node) {
                entries.retain(|listener| !remove_once.contains(&listener.id));
            }
        }
        event.immediate_propagation_stopped = false;
    }

    /// Default activation behavior for a click, exposed to the host bridge so
    /// combined native + JavaScript dispatch can decide default actions once
    /// the full propagation pass has finished.
    pub fn apply_click_default(&mut self, target: NodeId) -> Result<Vec<DefaultAction>, String> {
        let mut actions = Vec::new();
        let tag = self.element_tag(target).unwrap_or_default();
        match tag.as_str() {
            "input" => match self.get_attribute(target, "type").unwrap_or("text") {
                "checkbox" => {
                    let checked = self.get_attribute(target, "checked").is_none();
                    if checked {
                        self.set_attribute(target, "checked", "")?;
                    } else {
                        self.remove_attribute(target, "checked")?;
                    }
                    actions.push(DefaultAction::ToggleChecked(target, checked));
                }
                "radio" => {
                    self.activate_radio(target)?;
                    actions.push(DefaultAction::ToggleChecked(target, true));
                }
                "submit" => {
                    if let Some(form) = self.closest_tag(target, "form") {
                        self.set_attribute(form, "data-ghita-submitted", "true")?;
                        actions.push(DefaultAction::SubmitForm(form));
                    }
                }
                _ => {
                    self.focus(target)?;
                    actions.push(DefaultAction::Focus(target));
                }
            },
            "button" => {
                if self.get_attribute(target, "type").unwrap_or("submit") == "submit" {
                    if let Some(form) = self.closest_tag(target, "form") {
                        self.set_attribute(form, "data-ghita-submitted", "true")?;
                        actions.push(DefaultAction::SubmitForm(form));
                    }
                } else {
                    self.focus(target)?;
                    actions.push(DefaultAction::Focus(target));
                }
            }
            "select" => {
                let options: Vec<NodeId> = self
                    .document_order()
                    .into_iter()
                    .filter(|candidate| {
                        self.is_ancestor(target, *candidate)
                            && matches!(
                                self.nodes.get(candidate).map(|node| &node.kind),
                                Some(LiveNodeKind::Element { tag, .. }) if tag == "option"
                            )
                    })
                    .collect();
                if !options.is_empty() {
                    // No explicit selection: the first option is the default
                    // selection, so a click advances to the next one.
                    let current = options
                        .iter()
                        .position(|option| self.get_attribute(*option, "selected").is_some())
                        .unwrap_or(0);
                    let next = options[(current + 1) % options.len()];
                    for option in &options {
                        self.remove_attribute(*option, "selected")?;
                    }
                    self.set_attribute(next, "selected", "")?;
                    actions.push(DefaultAction::SelectOption(next));
                }
            }
            "a" => {
                if let Some(href) = self.get_attribute(target, "href") {
                    actions.push(DefaultAction::Navigate(href.to_string()));
                }
            }
            _ => {}
        }
        Ok(actions)
    }

    /// Default keyboard behavior for the focused node, exposed to the host
    /// bridge for the same combined-dispatch reason as `apply_click_default`.
    pub fn apply_key_default(
        &mut self,
        target: NodeId,
        key: &str,
    ) -> Result<Vec<DefaultAction>, String> {
        let mut actions = Vec::new();
        if key == "Tab" {
            if let Some(next) = self.next_focusable(target) {
                self.focus(next)?;
                actions.push(DefaultAction::Focus(next));
            }
            return Ok(actions);
        }
        if matches!(key, "Enter" | " ") && self.is_focusable(target) {
            let click = self.click(target)?;
            actions.extend(click.default_actions);
            return Ok(actions);
        }
        if key.chars().count() == 1 && self.is_text_entry(target) {
            if self.element_tag(target).as_deref() == Some("textarea") {
                // Textarea content lives in its text children, not the value
                // attribute; edit the composed text content instead.
                let mut current = self.text_content(target).unwrap_or_default();
                current.push_str(key);
                self.set_text_content(target, &current)?;
            } else {
                let mut value = self
                    .get_attribute(target, "value")
                    .unwrap_or("")
                    .to_string();
                value.push_str(key);
                self.set_attribute(target, "value", &value)?;
            }
            let mut input = DomEvent::new("input", target);
            let _ = self.dispatch_event(target, &mut input)?;
            actions.push(DefaultAction::InsertText(target, key.to_string()));
        }
        Ok(actions)
    }

    fn activate_radio(&mut self, target: NodeId) -> Result<(), String> {
        let name = self.get_attribute(target, "name").unwrap_or("").to_string();
        let form = self.closest_tag(target, "form");
        for node in self.document_order() {
            if node != target
                && self.element_tag(node).as_deref() == Some("input")
                && self.get_attribute(node, "type") == Some("radio")
                && self.get_attribute(node, "name").unwrap_or("") == name
                && self.closest_tag(node, "form") == form
            {
                self.remove_attribute(node, "checked")?;
            }
        }
        self.set_attribute(target, "checked", "")
    }

    fn closest_tag(&self, node: NodeId, tag: &str) -> Option<NodeId> {
        let mut current = Some(node);
        while let Some(candidate) = current {
            if self.element_tag(candidate).as_deref() == Some(tag) {
                return Some(candidate);
            }
            current = self.nodes.get(&candidate).and_then(|entry| entry.parent);
        }
        None
    }

    fn next_focusable(&self, current: NodeId) -> Option<NodeId> {
        let candidates: Vec<NodeId> = self
            .document_order()
            .into_iter()
            .filter(|node| self.is_focusable(*node))
            .collect();
        let index = candidates.iter().position(|node| *node == current)?;
        candidates.get((index + 1) % candidates.len()).copied()
    }

    fn is_focusable(&self, node: NodeId) -> bool {
        if self.get_attribute(node, "disabled").is_some() {
            return false;
        }
        // Shadow hosts delegate focus into their shadow tree (Phase 21).
        if self.shadows.contains_key(&node) {
            return true;
        }
        match self.element_tag(node).as_deref() {
            Some("input" | "textarea" | "select" | "button") => true,
            Some("a") => self.get_attribute(node, "href").is_some(),
            _ => self.get_attribute(node, "tabindex").is_some(),
        }
    }

    fn is_text_entry(&self, node: NodeId) -> bool {
        matches!(self.element_tag(node).as_deref(), Some("textarea"))
            || (self.element_tag(node).as_deref() == Some("input")
                && !matches!(
                    self.get_attribute(node, "type"),
                    Some("checkbox" | "radio" | "submit" | "button")
                ))
    }

    fn element_tag(&self, node: NodeId) -> Option<String> {
        let LiveNodeKind::Element { tag, .. } = &self.nodes.get(&node)?.kind else {
            return None;
        };
        Some(tag.clone())
    }

    fn mark_mutated(&mut self) {
        self.invalidation = MutationInvalidation::all();
    }

    fn mark_attribute_mutated(&mut self, name: &str, value: &str) {
        if name.starts_with("aria-") {
            self.invalidation.accessibility = true;
            return;
        }
        let (style, layout, paint) = if name == "style" {
            let lower = value.to_ascii_lowercase();
            let affects_layout = [
                "display",
                "position",
                "width",
                "height",
                "margin",
                "padding",
                "border",
                "flex",
                "grid",
                "top",
                "right",
                "bottom",
                "left",
                "font-size",
                "line-height",
            ]
            .iter()
            .any(|property| lower.contains(property));
            (true, affects_layout, true)
        } else if matches!(name, "class" | "id" | "hidden") {
            (true, true, true)
        } else if matches!(name, "value" | "checked" | "placeholder" | "src" | "href") {
            (false, name != "href", true)
        } else {
            (false, false, true)
        };
        self.invalidation.style |= style;
        self.invalidation.layout |= layout;
        self.invalidation.paint |= paint;
        self.invalidation.accessibility = true;
    }

    fn matches_selector_path(&self, node: NodeId, parts: &[&str]) -> bool {
        if !self.matches_simple_selector(node, parts.last().copied().unwrap_or_default()) {
            return false;
        }
        let mut current = self.nodes.get(&node).and_then(|entry| entry.parent);
        for selector in parts[..parts.len().saturating_sub(1)].iter().rev() {
            loop {
                let Some(candidate) = current else {
                    return false;
                };
                if self.matches_simple_selector(candidate, selector) {
                    current = self.nodes.get(&candidate).and_then(|entry| entry.parent);
                    break;
                }
                current = self.nodes.get(&candidate).and_then(|entry| entry.parent);
            }
        }
        true
    }

    fn matches_simple_selector(&self, node: NodeId, selector: &str) -> bool {
        let Some(tag) = self.element_tag(node) else {
            return false;
        };
        let attrs = match &self.nodes[&node].kind {
            LiveNodeKind::Element { attrs, .. } => attrs,
            LiveNodeKind::Text(_) => return false,
        };
        let selector = selector.trim();
        if selector.is_empty() {
            return false;
        }
        let (base, attribute) = if let Some(open) = selector.find('[') {
            if !selector.ends_with(']') {
                return false;
            }
            (
                &selector[..open],
                Some(&selector[open + 1..selector.len() - 1]),
            )
        } else {
            (selector, None)
        };
        if let Some(attribute) = attribute {
            let mut parts = attribute.splitn(2, '=');
            let name = parts.next().unwrap_or("").trim().to_ascii_lowercase();
            if name.is_empty() {
                return false;
            }
            if let Some(value) = parts.next() {
                let value = value.trim().trim_matches(['\'', '"']);
                if attrs.get(&name).map(String::as_str) != Some(value) {
                    return false;
                }
            } else if !attrs.contains_key(&name) {
                return false;
            }
        }
        let mut rest = base;
        if let Some(index) = rest.find('#') {
            let id = &rest[index + 1..];
            if attrs.get("id").map(String::as_str) != Some(id) {
                return false;
            }
            rest = &rest[..index];
        }
        if let Some(index) = rest.find('.') {
            let class = &rest[index + 1..];
            if !attrs
                .get("class")
                .is_some_and(|classes| classes.split_ascii_whitespace().any(|item| item == class))
            {
                return false;
            }
            rest = &rest[..index];
        }
        rest.is_empty() || tag.eq_ignore_ascii_case(rest)
    }
}

/// Collect SVG shape elements (`rect`, `circle`, `line`, `ellipse`) from the
/// composed DOM into display-list items positioned at the SVG container's
/// layout origin plus the shape's attribute offsets.
fn collect_svg_shapes(
    dom: &Element,
    display_list: &mut DisplayList,
    layout: &Option<LayoutNode>,
    shape_count: &mut usize,
) {
    let (origin_x, origin_y) = layout
        .as_ref()
        .map(|layout| (layout.rect.x as f32, layout.rect.y as f32))
        .unwrap_or((0.0, 0.0));
    fn visit(
        element: &Element,
        container_x: f32,
        container_y: f32,
        display_list: &mut DisplayList,
        shape_count: &mut usize,
    ) {
        if *shape_count >= 512 {
            return;
        }
        for child in &element.children {
            if child.tag == "svg" {
                visit(child, container_x, container_y, display_list, shape_count);
                continue;
            }
            let shape = match child.tag.as_str() {
                "rect" => {
                    let x = attr_f32(child, "x").unwrap_or(0.0);
                    let y = attr_f32(child, "y").unwrap_or(0.0);
                    let w = attr_f32(child, "width").unwrap_or(0.0);
                    let h = attr_f32(child, "height").unwrap_or(0.0);
                    Some(crate::paint::VectorShape {
                        kind: crate::paint::VectorShapeKind::Rect,
                        x: container_x + x,
                        y: container_y + y,
                        w,
                        h,
                        fill: attr_color(child, "fill"),
                        stroke: attr_color(child, "stroke"),
                        stroke_width: attr_f32(child, "stroke-width").unwrap_or(1.0),
                    })
                }
                "circle" => {
                    let cx = attr_f32(child, "cx").unwrap_or(0.0);
                    let cy = attr_f32(child, "cy").unwrap_or(0.0);
                    let r = attr_f32(child, "r").unwrap_or(0.0);
                    Some(crate::paint::VectorShape {
                        kind: crate::paint::VectorShapeKind::Ellipse,
                        x: container_x + cx - r,
                        y: container_y + cy - r,
                        w: r * 2.0,
                        h: r * 2.0,
                        fill: attr_color(child, "fill"),
                        stroke: attr_color(child, "stroke"),
                        stroke_width: attr_f32(child, "stroke-width").unwrap_or(1.0),
                    })
                }
                "line" => {
                    let x1 = attr_f32(child, "x1").unwrap_or(0.0);
                    let y1 = attr_f32(child, "y1").unwrap_or(0.0);
                    let x2 = attr_f32(child, "x2").unwrap_or(0.0);
                    let y2 = attr_f32(child, "y2").unwrap_or(0.0);
                    Some(crate::paint::VectorShape {
                        kind: crate::paint::VectorShapeKind::Line,
                        x: container_x + x1,
                        y: container_y + y1,
                        w: x2 - x1,
                        h: y2 - y1,
                        fill: None,
                        stroke: attr_color(child, "stroke").or_else(|| attr_color(child, "fill")),
                        stroke_width: attr_f32(child, "stroke-width").unwrap_or(1.0),
                    })
                }
                _ => None,
            };
            if let Some(shape) = shape {
                display_list
                    .items
                    .push(crate::paint::DisplayItem::VectorShape(shape));
                *shape_count += 1;
            }
            if !child.children.is_empty() {
                visit(child, container_x, container_y, display_list, shape_count);
            }
        }
    }
    visit(dom, origin_x, origin_y, display_list, shape_count);
}

fn attr_f32(element: &Element, name: &str) -> Option<f32> {
    element
        .get_attr(name)
        .and_then(|value| value.trim().parse::<f64>().ok())
        .map(|value| value.clamp(-1_000_000.0, 1_000_000.0) as f32)
}

/// Parse a CSS color string (`#rrggbb`, `#rgb`, named colors) into RGBA.
fn attr_color(element: &Element, name: &str) -> Option<crate::paint::Rgba> {
    let value = element.get_attr(name)?;
    parse_css_color(value)
}

fn parse_css_color(value: &str) -> Option<crate::paint::Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        // Only slice after validating every byte is ASCII hex: length alone
        // does not guard char boundaries for multi-byte input.
        if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let parse = |s: &str| u8::from_str_radix(s, 16).ok();
            let (r, g, b) = (&hex[0..2], &hex[2..4], &hex[4..6]);
            return Some(crate::paint::Rgba {
                r: parse(r)? as f32 / 255.0,
                g: parse(g)? as f32 / 255.0,
                b: parse(b)? as f32 / 255.0,
                a: 1.0,
            });
        }
        if hex.len() == 3 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let parse = |s: &str| u8::from_str_radix(s, 16).ok();
            let (r, g, b) = (&hex[0..1], &hex[1..2], &hex[2..3]);
            return Some(crate::paint::Rgba {
                r: parse(r)? as f32 / 15.0,
                g: parse(g)? as f32 / 15.0,
                b: parse(b)? as f32 / 15.0,
                a: 1.0,
            });
        }
    }
    let named = match value.to_ascii_lowercase().as_str() {
        "red" => (1.0, 0.0, 0.0),
        "green" | "lime" => (0.0, 1.0, 0.0),
        "blue" => (0.0, 0.0, 1.0),
        "black" => (0.0, 0.0, 0.0),
        "white" => (1.0, 1.0, 1.0),
        "yellow" => (1.0, 1.0, 0.0),
        "orange" => (1.0, 0.65, 0.0),
        "gray" | "grey" => (0.5, 0.5, 0.5),
        "purple" => (0.5, 0.0, 0.5),
        "transparent" => (0.0, 0.0, 0.0),
        _ => return None,
    };
    Some(crate::paint::Rgba {
        r: named.0,
        g: named.1,
        b: named.2,
        a: if value.eq_ignore_ascii_case("transparent") {
            0.0
        } else {
            1.0
        },
    })
}

fn clear_node_ids(element: &mut Element) {
    element.node_id = None;
    for child in &mut element.children {
        clear_node_ids(child);
    }
}

fn normalize_tag(tag: &str) -> Result<String, String> {
    let tag = tag.trim().to_ascii_lowercase();
    if tag.is_empty()
        || tag.len() > 64
        || !tag
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("InvalidCharacterError: invalid element tag".to_string());
    }
    Ok(tag)
}

fn normalize_attribute_name(name: &str) -> Result<String, String> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty()
        || name.len() > 128
        || name.starts_with("on")
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_:.".contains(character))
    {
        return Err("InvalidCharacterError: invalid attribute name".to_string());
    }
    Ok(name)
}

fn normalize_event_type(event_type: &str) -> Result<String, String> {
    let event_type = event_type.trim().to_ascii_lowercase();
    if event_type.is_empty()
        || event_type.len() > 64
        || !event_type
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("InvalidCharacterError: invalid event type".to_string());
    }
    Ok(event_type)
}

fn bounded_text(value: &str) -> String {
    value.chars().take(MAX_TEXT_BYTES).collect()
}

fn bounded_attribute(value: &str) -> String {
    value.chars().take(MAX_ATTRIBUTE_BYTES).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn document() -> LiveDocument {
        LiveDocument::parse(
            "<main id='app'><form id='form'><input id='name'><input id='choice' type='checkbox'><button id='submit'>Send</button></form><p id='message'>old</p></main>",
            Vec::new(),
            800,
        )
    }

    #[test]
    fn stable_nodes_mutate_and_refresh_all_render_outputs() {
        let mut document = document();
        let message = document.get_element_by_id("message").unwrap();
        let original_revision = document.render_state().revision;
        document
            .set_text_content(message, "new accessible text")
            .unwrap();
        assert!(document.invalidation().layout);
        assert_eq!(document.get_element_by_id("message"), Some(message));
        let render = document.refresh();
        let refreshed_revision = render.revision;
        assert!(refreshed_revision > original_revision);
        assert!(render.display_list.items.iter().any(|item| matches!(item, crate::paint::DisplayItem::TextRun { content, .. } if content.contains("new accessible text"))));
        assert!(render
            .accessibility
            .root
            .as_ref()
            .is_some_and(|root| format!("{root:?}").contains("new accessible text")));
        let settled = document.refresh().revision;
        assert_eq!(
            settled, refreshed_revision,
            "clean reads do not rebuild render state"
        );
    }

    #[test]
    fn selectors_tree_mutations_and_node_identity_are_bounded() {
        let mut document = document();
        let root = document.query_selector("main#app").unwrap();
        let paragraph = document.query_selector("main [id='message']").unwrap();
        let span = document.create_element("span").unwrap();
        let text = document.create_text_node("child").unwrap();
        document.append_child(span, text).unwrap();
        document.append_child(paragraph, span).unwrap();
        assert_eq!(document.text_content(paragraph).unwrap(), "oldchild");
        document.set_attribute(span, "data-state", "live").unwrap();
        assert_eq!(
            document.query_selector("main span[data-state=live]"),
            Some(span)
        );
        assert!(document.append_child(span, root).is_err());
        document.remove_child(paragraph, span).unwrap();
        assert_eq!(document.node(span).unwrap().parent, None);
    }

    #[test]
    fn capture_target_bubble_once_and_passive_are_observable() {
        let mut document = document();
        let main = document.root();
        let target = document.get_element_by_id("message").unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        for (node, label, options) in [
            (
                main,
                "capture",
                ListenerOptions {
                    capture: true,
                    ..Default::default()
                },
            ),
            (
                target,
                "target",
                ListenerOptions {
                    once: true,
                    ..Default::default()
                },
            ),
            (main, "bubble", ListenerOptions::default()),
        ] {
            let calls = calls.clone();
            document
                .add_event_listener(
                    node,
                    "click",
                    options,
                    Arc::new(move |_| calls.lock().unwrap().push(label)),
                )
                .unwrap();
        }
        let report = document.click(target).unwrap();
        assert_eq!(*calls.lock().unwrap(), vec!["capture", "target", "bubble"]);
        assert_eq!(report.invoked_listeners, 3);
        let _ = document.click(target).unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["capture", "target", "bubble", "capture", "bubble"]
        );

        document
            .add_event_listener(
                target,
                "wheel",
                ListenerOptions {
                    passive: true,
                    ..Default::default()
                },
                Arc::new(|event| event.prevent_default()),
            )
            .unwrap();
        let mut wheel = DomEvent::new("wheel", target);
        assert!(
            !document
                .dispatch_event(target, &mut wheel)
                .unwrap()
                .default_prevented
        );
    }

    #[test]
    fn form_focus_keyboard_pointer_and_default_actions_update_dom() {
        let mut document = document();
        let name = document.get_element_by_id("name").unwrap();
        let choice = document.get_element_by_id("choice").unwrap();
        let submit = document.get_element_by_id("submit").unwrap();
        let form = document.get_element_by_id("form").unwrap();
        document.focus(name).unwrap();
        let keyboard = document.dispatch_keyboard("keydown", "A").unwrap();
        assert_eq!(document.get_attribute(name, "value"), Some("A"));
        assert!(keyboard
            .default_actions
            .contains(&DefaultAction::InsertText(name, "A".into())));
        document.click(choice).unwrap();
        assert!(document.get_attribute(choice, "checked").is_some());
        document.click(submit).unwrap();
        assert_eq!(
            document.get_attribute(form, "data-ghita-submitted"),
            Some("true")
        );

        let x = document.render_state().layout.as_ref().unwrap().rect.x as i32 + 1;
        let y = document.render_state().layout.as_ref().unwrap().rect.y as i32 + 1;
        assert!(document
            .dispatch_pointer("pointerdown", x, y)
            .unwrap()
            .is_some());
    }
}

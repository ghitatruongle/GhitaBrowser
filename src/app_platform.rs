//! Bounded application-platform primitives layered on the live DOM.
//!
//! This module keeps application state explicit instead of embedding another
//! browser engine: custom-element definitions produce lifecycle records for the
//! script host, shadow trees have independent live documents, slot assignment
//! is deterministic, templates remain inert until cloned, and ES module source
//! is handed to the existing bounded module graph.

use std::collections::{BTreeMap, BTreeSet};

use crate::css_parser::CssRule;
use crate::javascript::{JsvModuleGraph, ModuleNamespace};
use crate::live_dom::{LiveDocument, LiveNodeKind, NodeId};
use crate::parser::Element;

const MAX_CUSTOM_DEFINITIONS: usize = 512;
const MAX_CUSTOM_INSTANCES: usize = 10_000;
const MAX_LIFECYCLE_RECORDS: usize = 16_384;
const MAX_SHADOW_ROOTS: usize = 2_048;
const MAX_TEMPLATES: usize = 1_024;
const MAX_TEMPLATE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomElementDefinition {
    pub name: String,
    pub observed_attributes: BTreeSet<String>,
}

impl CustomElementDefinition {
    pub fn new<I, S>(name: &str, observed_attributes: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        validate_custom_element_name(name)?;
        let observed_attributes = observed_attributes
            .into_iter()
            .map(|name| name.as_ref().trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .take(256)
            .collect();
        Ok(Self {
            name: name.to_ascii_lowercase(),
            observed_attributes,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct CustomElementRegistry {
    definitions: BTreeMap<String, CustomElementDefinition>,
}

impl CustomElementRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn define(&mut self, definition: CustomElementDefinition) -> Result<(), String> {
        if self.definitions.len() >= MAX_CUSTOM_DEFINITIONS {
            return Err("QuotaExceededError: custom-element registry is full".to_string());
        }
        if self.definitions.contains_key(&definition.name) {
            return Err(format!(
                "NotSupportedError: custom element '{}' is already defined",
                definition.name
            ));
        }
        self.definitions.insert(definition.name.clone(), definition);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&CustomElementDefinition> {
        self.definitions.get(&name.to_ascii_lowercase())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

fn validate_custom_element_name(name: &str) -> Result<(), String> {
    let lower = name.to_ascii_lowercase();
    let reserved = [
        "annotation-xml",
        "color-profile",
        "font-face",
        "font-face-src",
        "font-face-uri",
        "font-face-format",
        "font-face-name",
        "missing-glyph",
    ];
    if lower != name
        || !lower.contains('-')
        || lower.starts_with("xml")
        || reserved.contains(&lower.as_str())
        || lower.len() > 128
        || !lower.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'_')
        })
    {
        return Err(format!("SyntaxError: invalid custom-element name '{name}'"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleKind {
    Constructed,
    Connected,
    Disconnected,
    AttributeChanged {
        name: String,
        old_value: Option<String>,
        new_value: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleRecord {
    pub element: NodeId,
    pub custom_name: String,
    pub kind: LifecycleKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowMode {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotAssignment {
    pub slot: NodeId,
    pub name: String,
    pub assigned_nodes: Vec<NodeId>,
}

#[derive(Debug)]
pub struct ShadowRoot {
    pub host: NodeId,
    pub mode: ShadowMode,
    document: LiveDocument,
    assignments: Vec<SlotAssignment>,
}

impl ShadowRoot {
    pub fn document(&self) -> &LiveDocument {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut LiveDocument {
        &mut self.document
    }

    pub fn slot_assignments(&self) -> &[SlotAssignment] {
        &self.assignments
    }
}

#[derive(Debug, Clone)]
struct HtmlTemplate {
    fragment: Element,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HydrationReport {
    pub custom_elements: usize,
    pub shadow_roots: usize,
    pub templates: usize,
    pub evaluated_modules: usize,
    pub document_revision: u64,
}

/// A page application document. Lifecycle records are deliberately queued for
/// the embedding JavaScript realm; consumers drain them at microtask
/// checkpoints and invoke their own callbacks.
#[derive(Debug)]
pub struct ApplicationDocument {
    document: LiveDocument,
    registry: CustomElementRegistry,
    custom_instances: BTreeMap<NodeId, String>,
    connected: BTreeSet<NodeId>,
    lifecycle: Vec<LifecycleRecord>,
    shadows: BTreeMap<NodeId, ShadowRoot>,
    templates: BTreeMap<String, HtmlTemplate>,
    modules: JsvModuleGraph,
    evaluated_modules: BTreeSet<String>,
    css_rules: Vec<CssRule>,
    viewport_width: u32,
    hydrated: bool,
}

impl ApplicationDocument {
    pub fn parse(html: &str, css_rules: Vec<CssRule>, viewport_width: u32) -> Self {
        let mut root = crate::parser::parse_html(html);
        let mut templates = BTreeMap::new();
        extract_templates(&mut root, &mut templates);
        let document = LiveDocument::from_element(&root, css_rules.clone(), viewport_width);
        Self {
            document,
            registry: CustomElementRegistry::new(),
            custom_instances: BTreeMap::new(),
            connected: BTreeSet::new(),
            lifecycle: Vec::new(),
            shadows: BTreeMap::new(),
            templates,
            modules: JsvModuleGraph::new(),
            evaluated_modules: BTreeSet::new(),
            css_rules,
            viewport_width: viewport_width.max(1),
            hydrated: false,
        }
    }

    pub fn document(&self) -> &LiveDocument {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut LiveDocument {
        &mut self.document
    }

    pub fn registry(&self) -> &CustomElementRegistry {
        &self.registry
    }

    pub fn define_custom_element(
        &mut self,
        definition: CustomElementDefinition,
    ) -> Result<usize, String> {
        let name = definition.name.clone();
        self.registry.define(definition)?;
        let candidates = self.document.query_selector_all(&name);
        let mut upgraded = 0;
        for node in candidates {
            if self.upgrade_node(node)? {
                upgraded += 1;
            }
        }
        Ok(upgraded)
    }

    pub fn create_element(&mut self, tag: &str) -> Result<NodeId, String> {
        let node = self.document.create_element(tag)?;
        self.upgrade_node(node)?;
        Ok(node)
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), String> {
        self.document.append_child(parent, child)?;
        let subtree = subtree_ids(&self.document, child);
        for node in &subtree {
            self.upgrade_node(*node)?;
        }
        for node in subtree {
            if let Some(name) = self.custom_instances.get(&node).cloned() {
                if self.is_connected(node) && self.connected.insert(node) {
                    self.queue_lifecycle(node, name, LifecycleKind::Connected);
                }
            }
        }
        self.recalculate_all_slots();
        Ok(())
    }

    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), String> {
        let disconnected = subtree_ids(&self.document, child)
            .into_iter()
            .filter_map(|node| {
                self.custom_instances
                    .get(&node)
                    .cloned()
                    .filter(|_| self.connected.contains(&node))
                    .map(|name| (node, name))
            })
            .collect::<Vec<_>>();
        self.document.remove_child(parent, child)?;
        for (node, name) in disconnected {
            self.connected.remove(&node);
            self.queue_lifecycle(node, name, LifecycleKind::Disconnected);
        }
        self.recalculate_all_slots();
        Ok(())
    }

    pub fn set_attribute(&mut self, node: NodeId, name: &str, value: &str) -> Result<(), String> {
        let name = name.to_ascii_lowercase();
        let old_value = self.document.get_attribute(node, &name).map(str::to_string);
        self.document.set_attribute(node, &name, value)?;
        self.attribute_changed(node, &name, old_value, Some(value.to_string()));
        if name == "slot" {
            self.recalculate_all_slots();
        }
        Ok(())
    }

    pub fn remove_attribute(&mut self, node: NodeId, name: &str) -> Result<(), String> {
        let name = name.to_ascii_lowercase();
        let old_value = self.document.get_attribute(node, &name).map(str::to_string);
        self.document.remove_attribute(node, &name)?;
        self.attribute_changed(node, &name, old_value, None);
        if name == "slot" {
            self.recalculate_all_slots();
        }
        Ok(())
    }

    pub fn attach_shadow(
        &mut self,
        host: NodeId,
        mode: ShadowMode,
        html: &str,
    ) -> Result<(), String> {
        if self.shadows.len() >= MAX_SHADOW_ROOTS {
            return Err("QuotaExceededError: shadow-root budget exceeded".to_string());
        }
        if html.len() > MAX_TEMPLATE_BYTES {
            return Err("QuotaExceededError: shadow markup exceeds 2 MB".to_string());
        }
        if self.shadows.contains_key(&host) {
            return Err("NotSupportedError: host already has a shadow root".to_string());
        }
        let Some(node) = self.document.node(host) else {
            return Err("NotFoundError: shadow host is not in this document".to_string());
        };
        if !matches!(node.kind, LiveNodeKind::Element { .. }) {
            return Err("NotSupportedError: shadow host must be an element".to_string());
        }
        let shadow = ShadowRoot {
            host,
            mode,
            document: LiveDocument::parse(html, self.css_rules.clone(), self.viewport_width),
            assignments: Vec::new(),
        };
        self.shadows.insert(host, shadow);
        self.recalculate_slots(host);
        Ok(())
    }

    /// The DOM-facing accessor respects closed mode.
    pub fn shadow_root(&self, host: NodeId) -> Option<&ShadowRoot> {
        self.shadows
            .get(&host)
            .filter(|shadow| shadow.mode == ShadowMode::Open)
    }

    /// Internal inspection for rendering and conformance tools.
    pub fn shadow_root_internal(&self, host: NodeId) -> Option<&ShadowRoot> {
        self.shadows.get(&host)
    }

    pub fn template_names(&self) -> Vec<&str> {
        self.templates.keys().map(String::as_str).collect()
    }

    pub fn define_template(&mut self, name: &str, html: &str) -> Result<(), String> {
        if name.is_empty() || name.len() > 256 {
            return Err("SyntaxError: invalid template name".to_string());
        }
        if html.len() > MAX_TEMPLATE_BYTES || self.templates.len() >= MAX_TEMPLATES {
            return Err("QuotaExceededError: template budget exceeded".to_string());
        }
        self.templates.insert(
            name.to_string(),
            HtmlTemplate {
                fragment: crate::parser::parse_html(html),
            },
        );
        Ok(())
    }

    pub fn instantiate_template(
        &mut self,
        name: &str,
        parent: NodeId,
    ) -> Result<Vec<NodeId>, String> {
        let fragment = self
            .templates
            .get(name)
            .ok_or_else(|| format!("NotFoundError: template '{name}' is not defined"))?
            .fragment
            .clone();
        let mut inserted = Vec::new();
        if !fragment.text.is_empty() {
            let text = self.document.create_text_node(&fragment.text)?;
            self.append_child(parent, text)?;
            inserted.push(text);
        }
        for child in fragment.children {
            let node = self.document.import_subtree(&child)?;
            self.append_child(parent, node)?;
            inserted.push(node);
        }
        Ok(inserted)
    }

    pub fn register_module(&mut self, specifier: &str, source: &str) -> Result<(), String> {
        self.modules.register(specifier, source)
    }

    pub fn evaluate_module(&mut self, specifier: &str) -> Result<ModuleNamespace, String> {
        let namespace = self.modules.evaluate(specifier)?;
        self.evaluated_modules.insert(specifier.to_string());
        Ok(namespace)
    }

    pub fn hydrate(&mut self) -> HydrationReport {
        self.document.refresh();
        for shadow in self.shadows.values_mut() {
            shadow.document.refresh();
        }
        self.hydrated = true;
        HydrationReport {
            custom_elements: self.custom_instances.len(),
            shadow_roots: self.shadows.len(),
            templates: self.templates.len(),
            evaluated_modules: self.evaluated_modules.len(),
            document_revision: self.document.render_state().revision,
        }
    }

    pub fn is_hydrated(&self) -> bool {
        self.hydrated
    }

    pub fn lifecycle_records(&self) -> &[LifecycleRecord] {
        &self.lifecycle
    }

    pub fn take_lifecycle_records(&mut self) -> Vec<LifecycleRecord> {
        std::mem::take(&mut self.lifecycle)
    }

    fn upgrade_node(&mut self, node: NodeId) -> Result<bool, String> {
        if self.custom_instances.contains_key(&node) {
            return Ok(false);
        }
        let Some(tag) = self.element_tag(node) else {
            return Ok(false);
        };
        if !self.registry.contains(&tag) {
            return Ok(false);
        }
        if self.custom_instances.len() >= MAX_CUSTOM_INSTANCES {
            return Err("QuotaExceededError: custom-element instance budget exceeded".to_string());
        }
        self.custom_instances.insert(node, tag.clone());
        self.queue_lifecycle(node, tag.clone(), LifecycleKind::Constructed);
        if self.is_connected(node) {
            self.connected.insert(node);
            self.queue_lifecycle(node, tag, LifecycleKind::Connected);
        }
        Ok(true)
    }

    fn attribute_changed(
        &mut self,
        node: NodeId,
        name: &str,
        old_value: Option<String>,
        new_value: Option<String>,
    ) {
        if old_value == new_value {
            return;
        }
        let Some(custom_name) = self.custom_instances.get(&node).cloned() else {
            return;
        };
        let observes = self
            .registry
            .get(&custom_name)
            .is_some_and(|definition| definition.observed_attributes.contains(name));
        if observes {
            self.queue_lifecycle(
                node,
                custom_name,
                LifecycleKind::AttributeChanged {
                    name: name.to_string(),
                    old_value,
                    new_value,
                },
            );
        }
    }

    fn queue_lifecycle(&mut self, element: NodeId, custom_name: String, kind: LifecycleKind) {
        if self.lifecycle.len() < MAX_LIFECYCLE_RECORDS {
            self.lifecycle.push(LifecycleRecord {
                element,
                custom_name,
                kind,
            });
        }
    }

    fn element_tag(&self, node: NodeId) -> Option<String> {
        let LiveNodeKind::Element { tag, .. } = &self.document.node(node)?.kind else {
            return None;
        };
        Some(tag.clone())
    }

    fn is_connected(&self, node: NodeId) -> bool {
        let mut current = Some(node);
        for _ in 0..256 {
            let Some(id) = current else { return false };
            if id == self.document.root() {
                return true;
            }
            current = self.document.node(id).and_then(|entry| entry.parent);
        }
        false
    }

    fn recalculate_all_slots(&mut self) {
        let hosts = self.shadows.keys().copied().collect::<Vec<_>>();
        for host in hosts {
            self.recalculate_slots(host);
        }
    }

    fn recalculate_slots(&mut self, host: NodeId) {
        let Some(shadow) = self.shadows.get(&host) else {
            return;
        };
        let assignments = compute_slot_assignments(&self.document, shadow);
        if let Some(shadow) = self.shadows.get_mut(&host) {
            shadow.assignments = assignments;
        }
    }
}

fn subtree_ids(document: &LiveDocument, root: NodeId) -> Vec<NodeId> {
    let mut result = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        result.push(node);
        if let Some(entry) = document.node(node) {
            pending.extend(entry.children.iter().rev().copied());
        }
        if result.len() >= MAX_CUSTOM_INSTANCES {
            break;
        }
    }
    result
}

fn compute_slot_assignments(document: &LiveDocument, shadow: &ShadowRoot) -> Vec<SlotAssignment> {
    let light_children = document
        .node(shadow.host)
        .map(|host| host.children.clone())
        .unwrap_or_default();
    shadow
        .document
        .query_selector_all("slot")
        .into_iter()
        .map(|slot| {
            let name = shadow
                .document
                .get_attribute(slot, "name")
                .unwrap_or_default()
                .to_string();
            let assigned_nodes = light_children
                .iter()
                .copied()
                .filter(|node| document.get_attribute(*node, "slot").unwrap_or_default() == name)
                .collect();
            SlotAssignment {
                slot,
                name,
                assigned_nodes,
            }
        })
        .collect()
}

fn extract_templates(element: &mut Element, templates: &mut BTreeMap<String, HtmlTemplate>) {
    for child in &mut element.children {
        if child.tag == "template" {
            if let Some(name) = child.attrs.get("id").cloned() {
                if !name.is_empty()
                    && name.len() <= 256
                    && templates.len() < MAX_TEMPLATES
                    && child.to_html().len() <= MAX_TEMPLATE_BYTES
                {
                    let mut fragment = Element::new("root");
                    fragment.text = std::mem::take(&mut child.text);
                    fragment.children = std::mem::take(&mut child.children);
                    templates.insert(name, HtmlTemplate { fragment });
                }
            }
        } else {
            extract_templates(child, templates);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_elements_upgrade_and_emit_lifecycle_in_order() {
        let mut app = ApplicationDocument::parse("<main><x-card></x-card></main>", vec![], 800);
        let definition = CustomElementDefinition::new("x-card", ["title"]).unwrap();
        assert_eq!(app.define_custom_element(definition).unwrap(), 1);
        let card = app.document().query_selector("x-card").unwrap();
        app.set_attribute(card, "title", "Hello").unwrap();
        let kinds = app
            .lifecycle_records()
            .iter()
            .map(|record| &record.kind)
            .collect::<Vec<_>>();
        assert!(matches!(kinds[0], LifecycleKind::Constructed));
        assert!(matches!(kinds[1], LifecycleKind::Connected));
        assert!(matches!(kinds[2], LifecycleKind::AttributeChanged { .. }));
    }

    #[test]
    fn shadow_slots_and_inert_templates_are_materialized_explicitly() {
        let mut app = ApplicationDocument::parse(
            "<template id='row'><button class='row'>Open</button></template><x-list><span slot='label'>Files</span></x-list>",
            vec![],
            800,
        );
        let host = app.document().query_selector("x-list").unwrap();
        app.attach_shadow(
            host,
            ShadowMode::Open,
            "<section><slot name='label'></slot><slot></slot></section>",
        )
        .unwrap();
        let assignments = app.shadow_root(host).unwrap().slot_assignments();
        assert_eq!(assignments[0].assigned_nodes.len(), 1);
        assert!(app.document().query_selector("button.row").is_none());
        app.instantiate_template("row", host).unwrap();
        assert!(app.document().query_selector("button.row").is_some());
    }
}

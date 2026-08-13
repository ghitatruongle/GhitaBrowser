//! Incremental, bounded dynamic rendering for live documents.
//!
//! The parser and layout engine remain clean-room components. This layer owns
//! the reusable pieces between live-DOM revisions: a NodeId-keyed cascade
//! cache, a retained base layout, a retained display list and a small timeline
//! for opacity/axis-aligned transform animation. It deliberately fails closed
//! at fixed node, cache and animation budgets.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::css_parser::{parse_class_attr, ComputedStyle, CssRule, Transform2D};
use crate::layout::{self, LayoutNode};
use crate::live_dom::NodeId;
use crate::paint::{self, DisplayList};
use crate::parser::Element;
use crate::scene_compositor::RetainedScene;

const MAX_STYLE_CACHE_ENTRIES: usize = 50_000;
const MAX_ACTIVE_ANIMATIONS: usize = 4_096;
const MAX_ANIMATION_DURATION_MS: u64 = 60_000;
const ESTIMATED_STYLE_CACHE_BYTES: usize = 768;
const ESTIMATED_DISPLAY_ITEM_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DynamicInvalidation {
    pub style: bool,
    pub layout: bool,
    pub paint: bool,
}

impl DynamicInvalidation {
    pub fn full() -> Self {
        Self {
            style: true,
            layout: true,
            paint: true,
        }
    }

    pub fn paint_only() -> Self {
        Self {
            style: true,
            layout: false,
            paint: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynamicRenderMetrics {
    pub cascade_nodes: usize,
    pub cascade_cache_hits: usize,
    pub cascade_cache_misses: usize,
    pub layout_reused: bool,
    pub display_list_reused: bool,
    pub active_animations: usize,
    pub frame_time_ms: u64,
    pub estimated_retained_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct DynamicRenderFrame {
    pub layout: Option<LayoutNode>,
    pub display_list: DisplayList,
    pub scene: RetainedScene,
    pub metrics: DynamicRenderMetrics,
}

#[derive(Debug, Clone)]
struct CachedStyle {
    input_fingerprint: u64,
    style: ComputedStyle,
}

#[derive(Debug, Clone)]
struct ElementPaintData {
    attrs: HashMap<String, String>,
    text: String,
    is_void: bool,
}

#[derive(Debug, Clone, Default)]
struct StyleSnapshot {
    styles: BTreeMap<NodeId, ComputedStyle>,
    paint_data: BTreeMap<NodeId, ElementPaintData>,
    nodes: usize,
    hits: usize,
    misses: usize,
}

#[derive(Debug, Clone)]
enum AnimatedProperty {
    Opacity { from: f64, to: f64 },
    Transform { from: Transform2D, to: Transform2D },
}

#[derive(Debug, Clone)]
struct ActiveAnimation {
    property: AnimatedProperty,
    elapsed_ms: u64,
    duration_ms: u64,
    remaining_iterations: u16,
}

/// State retained for one live document. It has no I/O authority and can be
/// discarded together with the tab document.
#[derive(Debug, Default)]
pub struct DynamicRenderer {
    style_cache: BTreeMap<NodeId, CachedStyle>,
    base_layout: Option<LayoutNode>,
    retained_display_list: Option<DisplayList>,
    retained_scene: Option<RetainedScene>,
    previous_styles: BTreeMap<NodeId, ComputedStyle>,
    animations: BTreeMap<NodeId, Vec<ActiveAnimation>>,
    revision: u64,
}

impl DynamicRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn style_cache_len(&self) -> usize {
        self.style_cache.len()
    }

    pub fn active_animation_count(&self) -> usize {
        self.animations.values().map(Vec::len).sum()
    }

    /// Advance only the bounded visual timeline. Returns true when at least
    /// one visual property changed and a paint-only refresh is required.
    pub fn advance_animations(&mut self, elapsed_ms: u64) -> bool {
        if elapsed_ms == 0 || self.animations.is_empty() {
            return false;
        }
        let elapsed_ms = elapsed_ms.min(MAX_ANIMATION_DURATION_MS);
        let mut changed = false;
        self.animations.retain(|_, entries| {
            for animation in entries.iter_mut() {
                animation.elapsed_ms = animation.elapsed_ms.saturating_add(elapsed_ms);
                changed = true;
            }
            entries.retain_mut(|animation| {
                if animation.elapsed_ms < animation.duration_ms {
                    return true;
                }
                if animation.remaining_iterations > 1 {
                    animation.remaining_iterations -= 1;
                    animation.elapsed_ms %= animation.duration_ms.max(1);
                    true
                } else {
                    false
                }
            });
            !entries.is_empty()
        });
        changed
    }

    pub fn refresh(
        &mut self,
        root: &Element,
        rules: &[CssRule],
        viewport_width: u32,
        invalidation: DynamicInvalidation,
    ) -> DynamicRenderFrame {
        let started = Instant::now();
        let needs_snapshot =
            invalidation.style || invalidation.layout || self.base_layout.is_none();
        let snapshot = if needs_snapshot {
            let mut snapshot = StyleSnapshot::default();
            let rules_fingerprint = fingerprint_rules(rules);
            cascade_element(
                root,
                None,
                rules,
                rules_fingerprint,
                &mut self.style_cache,
                &mut snapshot,
                &[],
                true,
            );
            Some(snapshot)
        } else {
            None
        };

        if let Some(snapshot) = snapshot.as_ref() {
            self.synchronize_animations(&snapshot.styles);
        }

        let mut metrics = DynamicRenderMetrics::default();
        if let Some(snapshot) = snapshot.as_ref() {
            metrics.cascade_nodes = snapshot.nodes;
            metrics.cascade_cache_hits = snapshot.hits;
            metrics.cascade_cache_misses = snapshot.misses;
        }

        let rebuild_layout = invalidation.layout || self.base_layout.is_none();
        if rebuild_layout {
            let empty_styles = BTreeMap::new();
            let styles = snapshot
                .as_ref()
                .map(|snapshot| &snapshot.styles)
                .unwrap_or(&empty_styles);
            self.base_layout =
                layout::create_layout_tree_with_styles(root, rules, viewport_width, styles);
        } else if let (Some(snapshot), Some(layout)) =
            (snapshot.as_ref(), self.base_layout.as_mut())
        {
            apply_snapshot_to_layout(layout, snapshot);
            metrics.layout_reused = true;
        } else {
            metrics.layout_reused = self.base_layout.is_some();
        }

        let animation_frame = !self.animations.is_empty();
        let rebuild_display = invalidation.paint
            || invalidation.layout
            || invalidation.style
            || self.retained_display_list.is_none()
            || animation_frame;
        let layout = self.base_layout.clone();
        let display_list = if rebuild_display {
            let mut animated_layout = layout.clone();
            if let Some(layout) = animated_layout.as_mut() {
                apply_animation_overrides(layout, &self.animations);
            }
            let list = animated_layout
                .as_ref()
                .map(paint::build_display_list)
                .unwrap_or_default();
            self.retained_display_list = Some(list.clone());
            animated_layout.as_ref().map(|_| list).unwrap_or_default()
        } else {
            metrics.display_list_reused = true;
            self.retained_display_list.clone().unwrap_or_default()
        };
        let rendered_layout = if animation_frame {
            let mut layout = layout;
            if let Some(layout) = layout.as_mut() {
                apply_animation_overrides(layout, &self.animations);
            }
            layout
        } else {
            layout
        };
        let scene = if rebuild_display {
            let next =
                RetainedScene::from_display_list(self.revision.saturating_add(1), &display_list)
                    .unwrap_or_default();
            if let Some(retained) = self.retained_scene.as_mut() {
                retained.update_from(next);
            } else {
                self.retained_scene = Some(next);
            }
            self.retained_scene.clone().unwrap_or_default()
        } else {
            self.retained_scene.clone().unwrap_or_default()
        };

        self.previous_styles = snapshot
            .as_ref()
            .map(|snapshot| snapshot.styles.clone())
            .unwrap_or_else(|| self.previous_styles.clone());
        self.revision = self.revision.saturating_add(1);
        metrics.active_animations = self.active_animation_count();
        metrics.frame_time_ms = started.elapsed().as_millis() as u64;
        metrics.estimated_retained_bytes = self
            .style_cache
            .len()
            .saturating_mul(ESTIMATED_STYLE_CACHE_BYTES)
            .saturating_add(
                rendered_layout
                    .as_ref()
                    .map(layout::count_layout_nodes)
                    .unwrap_or_default()
                    .saturating_mul(384),
            )
            .saturating_add(
                display_list
                    .items
                    .len()
                    .saturating_mul(ESTIMATED_DISPLAY_ITEM_BYTES),
            )
            .saturating_add(scene.estimated_bytes());
        DynamicRenderFrame {
            layout: rendered_layout,
            display_list,
            scene,
            metrics,
        }
    }

    fn synchronize_animations(&mut self, styles: &BTreeMap<NodeId, ComputedStyle>) {
        let previous = self.previous_styles.clone();
        for (node, style) in styles {
            let Some(previous_style) = previous.get(node) else {
                self.start_named_animation(*node, style);
                continue;
            };
            let duration = transition_duration(style, "opacity");
            if duration > 0 && opacity_of(previous_style) != opacity_of(style) {
                self.push_animation(
                    *node,
                    ActiveAnimation {
                        property: AnimatedProperty::Opacity {
                            from: opacity_of(previous_style),
                            to: opacity_of(style),
                        },
                        elapsed_ms: 0,
                        duration_ms: duration,
                        remaining_iterations: 1,
                    },
                );
            }
            let duration = transition_duration(style, "transform");
            if duration > 0 && previous_style.transform != style.transform {
                self.push_animation(
                    *node,
                    ActiveAnimation {
                        property: AnimatedProperty::Transform {
                            from: previous_style.transform,
                            to: style.transform,
                        },
                        elapsed_ms: 0,
                        duration_ms: duration,
                        remaining_iterations: 1,
                    },
                );
            }
            if previous_style.animation_name != style.animation_name {
                self.start_named_animation(*node, style);
            }
        }
    }

    fn start_named_animation(&mut self, node: NodeId, style: &ComputedStyle) {
        let duration = style.animation_duration_ms.min(MAX_ANIMATION_DURATION_MS);
        if duration == 0 {
            return;
        }
        let iterations = style.animation_iterations.clamp(1, 1_000);
        match style.animation_name.as_deref() {
            Some("ghita-fade-in") => self.push_animation(
                node,
                ActiveAnimation {
                    property: AnimatedProperty::Opacity {
                        from: 0.0,
                        to: opacity_of(style),
                    },
                    elapsed_ms: 0,
                    duration_ms: duration,
                    remaining_iterations: iterations,
                },
            ),
            Some("ghita-slide-in") => self.push_animation(
                node,
                ActiveAnimation {
                    property: AnimatedProperty::Transform {
                        from: Transform2D {
                            translate_x: style.transform.translate_x - 16.0,
                            ..style.transform
                        },
                        to: style.transform,
                    },
                    elapsed_ms: 0,
                    duration_ms: duration,
                    remaining_iterations: iterations,
                },
            ),
            _ => {}
        }
    }

    fn push_animation(&mut self, node: NodeId, animation: ActiveAnimation) {
        if self.active_animation_count() >= MAX_ACTIVE_ANIMATIONS {
            return;
        }
        let entries = self.animations.entry(node).or_default();
        if entries.len() < 2 {
            entries.push(animation);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cascade_element(
    element: &Element,
    parent_style: Option<&ComputedStyle>,
    rules: &[CssRule],
    rules_fingerprint: u64,
    cache: &mut BTreeMap<NodeId, CachedStyle>,
    snapshot: &mut StyleSnapshot,
    ancestry: &[crate::css_parser::ElementAncestry],
    is_root: bool,
) {
    let classes = parse_class_attr(element.get_attr("class").map(String::as_str));
    let element_id = element.get_attr("id").map(String::as_str);
    let parent_fingerprint = parent_style
        .map(inherited_style_fingerprint)
        .unwrap_or_default();
    let input_fingerprint = fingerprint_element(element, rules_fingerprint, parent_fingerprint);
    let style = if let Some(node) = element.node_id {
        if let Some(cached) = cache
            .get(&node)
            .filter(|cached| cached.input_fingerprint == input_fingerprint)
        {
            snapshot.hits = snapshot.hits.saturating_add(1);
            cached.style.clone()
        } else {
            let style = crate::css_parser::compute_computed_style_with_ancestors(
                &element.tag,
                &classes,
                element_id,
                rules,
                parent_style,
                &element.attrs,
                is_root,
                ancestry,
            );
            if cache.len() < MAX_STYLE_CACHE_ENTRIES || cache.contains_key(&node) {
                cache.insert(
                    node,
                    CachedStyle {
                        input_fingerprint,
                        style: style.clone(),
                    },
                );
            }
            snapshot.misses = snapshot.misses.saturating_add(1);
            style
        }
    } else {
        snapshot.misses = snapshot.misses.saturating_add(1);
        crate::css_parser::compute_computed_style_with_ancestors(
            &element.tag,
            &classes,
            element_id,
            rules,
            parent_style,
            &element.attrs,
            is_root,
            ancestry,
        )
    };
    snapshot.nodes = snapshot.nodes.saturating_add(1);
    if let Some(node) = element.node_id {
        snapshot.styles.insert(node, style.clone());
        snapshot.paint_data.insert(
            node,
            ElementPaintData {
                attrs: element.attrs.clone(),
                text: element.text.clone(),
                is_void: element.is_void,
            },
        );
    }
    // Extend the ancestry chain for descendants (nearest ancestor first).
    let mut child_ancestry: Vec<crate::css_parser::ElementAncestry> =
        Vec::with_capacity(ancestry.len() + 1);
    child_ancestry.push(crate::css_parser::ElementAncestry {
        tag: element.tag.clone(),
        classes,
        id: element_id.map(str::to_string),
    });
    child_ancestry.extend_from_slice(ancestry);
    if child_ancestry.len() > 32 {
        child_ancestry.truncate(32);
    }
    for child in &element.children {
        cascade_element(
            child,
            Some(&style),
            rules,
            rules_fingerprint,
            cache,
            snapshot,
            &child_ancestry,
            false,
        );
    }
}

fn apply_snapshot_to_layout(node: &mut LayoutNode, snapshot: &StyleSnapshot) {
    if let Some(node_id) = node.element.node_id {
        if let Some(style) = snapshot.styles.get(&node_id) {
            node.computed_style = style.clone();
        }
        if let Some(paint_data) = snapshot.paint_data.get(&node_id) {
            node.element.attrs = paint_data.attrs.clone();
            node.element.text = paint_data.text.clone();
            node.element.is_void = paint_data.is_void;
        }
    }
    for child in &mut node.children {
        apply_snapshot_to_layout(child, snapshot);
    }
}

fn apply_animation_overrides(
    node: &mut LayoutNode,
    animations: &BTreeMap<NodeId, Vec<ActiveAnimation>>,
) {
    if let Some(node_id) = node.element.node_id {
        if let Some(entries) = animations.get(&node_id) {
            for animation in entries {
                let progress = animation_progress(animation);
                match animation.property {
                    AnimatedProperty::Opacity { from, to } => {
                        node.computed_style.opacity = Some(interpolate(from, to, progress));
                    }
                    AnimatedProperty::Transform { from, to } => {
                        let base = node.computed_style.transform;
                        let transform = Transform2D {
                            translate_x: interpolate(from.translate_x, to.translate_x, progress),
                            translate_y: interpolate(from.translate_y, to.translate_y, progress),
                            scale_x: interpolate(from.scale_x, to.scale_x, progress),
                            scale_y: interpolate(from.scale_y, to.scale_y, progress),
                        };
                        node.rect.x += transform.translate_x - base.translate_x;
                        node.rect.y += transform.translate_y - base.translate_y;
                        node.rect.width =
                            (node.rect.width / base.scale_x.max(0.01) * transform.scale_x).max(0.0);
                        node.rect.height = (node.rect.height / base.scale_y.max(0.01)
                            * transform.scale_y)
                            .max(0.0);
                        node.computed_style.transform = transform;
                    }
                }
            }
        }
    }
    for child in &mut node.children {
        apply_animation_overrides(child, animations);
    }
}

fn animation_progress(animation: &ActiveAnimation) -> f64 {
    let duration = animation.duration_ms.max(1);
    (animation.elapsed_ms.min(duration) as f64 / duration as f64).clamp(0.0, 1.0)
}

fn interpolate(from: f64, to: f64, progress: f64) -> f64 {
    from + (to - from) * progress
}

fn opacity_of(style: &ComputedStyle) -> f64 {
    style.opacity.unwrap_or(1.0).clamp(0.0, 1.0)
}

fn transition_duration(style: &ComputedStyle, property: &str) -> u64 {
    let Some(value) = style.transition.as_deref() else {
        return 0;
    };
    let first = value.split(',').next().unwrap_or_default();
    let tokens = first.split_whitespace().collect::<Vec<_>>();
    let property_matches = tokens
        .first()
        .is_some_and(|candidate| *candidate == property || *candidate == "all");
    if !property_matches {
        return 0;
    }
    tokens
        .iter()
        .find_map(|token| parse_duration_ms(token))
        .unwrap_or(0)
        .min(MAX_ANIMATION_DURATION_MS)
}

fn parse_duration_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Some(value) = value.strip_suffix("ms") {
        return value
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| value.clamp(0.0, MAX_ANIMATION_DURATION_MS as f64) as u64);
    }
    value
        .strip_suffix('s')
        .and_then(|value| value.trim().parse::<f64>().ok())
        .map(|value| (value * 1_000.0).clamp(0.0, MAX_ANIMATION_DURATION_MS as f64) as u64)
}

fn fingerprint_rules(rules: &[CssRule]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{rules:?}").hash(&mut hasher);
    hasher.finish()
}

fn fingerprint_element(element: &Element, rules: u64, parent: u64) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rules.hash(&mut hasher);
    parent.hash(&mut hasher);
    element.tag.hash(&mut hasher);
    element.text.hash(&mut hasher);
    let mut attrs = element.attrs.iter().collect::<Vec<_>>();
    attrs.sort_unstable_by(|left, right| left.0.cmp(right.0));
    for (name, value) in attrs {
        name.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    hasher.finish()
}

fn inherited_style_fingerprint(style: &ComputedStyle) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    style.color.hash(&mut hasher);
    style.font_family.hash(&mut hasher);
    format!("{:?}", style.font_size).hash(&mut hasher);
    style.font_weight.hash(&mut hasher);
    style.font_style.hash(&mut hasher);
    style.text_align.hash(&mut hasher);
    style.line_height.map(f64::to_bits).hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    #[test]
    fn cascade_cache_retains_layout_and_display_on_idle_frames() {
        let root = crate::parser::parse_html("<main><p id='message'>hello</p></main>");
        let mut live = crate::live_dom::LiveDocument::from_element(&root, vec![], 800);
        let message = live.get_element_by_id("message").unwrap();
        live.set_attribute(message, "style", "color:red").unwrap();
        let first = live.refresh().clone();
        assert!(first.dynamic.layout_reused);
        let second = live.refresh().clone();
        assert!(second.dynamic.display_list_reused);
        assert!(second.dynamic.estimated_retained_bytes > 0);
    }

    #[test]
    fn dynamic_frames_publish_a_toolkit_independent_retained_scene() {
        let root = crate::parser::parse_html("<main><p>scene output</p></main>");
        let mut renderer = super::DynamicRenderer::new();
        let first = renderer.refresh(&root, &[], 800, super::DynamicInvalidation::full());
        assert!(!first.scene.primitives().is_empty());
        assert!(!first.scene.damage().is_empty());
        let second = renderer.refresh(&root, &[], 800, super::DynamicInvalidation::default());
        assert_eq!(first.scene.primitives(), second.scene.primitives());
    }
}

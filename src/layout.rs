#![allow(dead_code)]

use crate::parser::Element;
use crate::css_parser::{compute_computed_style, ComputedStyle, CssRule};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayType {
    Block,
    Inline,
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct RectModel {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub margin_top: i32,
    pub margin_right: i32,
    pub margin_bottom: i32,
    pub margin_left: i32,
    pub padding_top: i32,
    pub padding_right: i32,
    pub padding_bottom: i32,
    pub padding_left: i32,
    pub border_top: i32,
    pub border_right: i32,
    pub border_bottom: i32,
    pub border_left: i32,
    pub display: DisplayType,
}

impl RectModel {
    pub fn content_width(&self) -> i32 {
        (self.width - self.padding_left - self.padding_right - self.border_left - self.border_right).max(0)
    }

    pub fn content_height(&self) -> i32 {
        (self.height - self.padding_top - self.padding_bottom - self.border_top - self.border_bottom).max(0)
    }
}

#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub element: Element,
    pub rect: RectModel,
    pub children: Vec<LayoutNode>,
}

impl LayoutNode {
    pub fn new(element: Element, rect: RectModel) -> Self {
        Self {
            element,
            rect,
            children: Vec::new(),
        }
    }

    pub fn add_child(&mut self, child: LayoutNode) {
        self.children.push(child);
    }
}

pub fn parse_display_style(style: &ComputedStyle, tag: &str) -> DisplayType {
    if let Some(ref d) = style.display {
        match d.to_lowercase().as_str() {
            "block" => DisplayType::Block,
            "inline" | "inline-block" => DisplayType::Inline,
            "none" => DisplayType::None,
            _ => default_display_for_tag(tag),
        }
    } else {
        default_display_for_tag(tag)
    }
}

fn default_display_for_tag(tag: &str) -> DisplayType {
    match tag {
        "span" | "a" | "i" | "b" | "em" | "strong" | "img" => DisplayType::Inline,
        "head" | "script" | "style" | "meta" | "link" => DisplayType::None,
        _ => DisplayType::Block,
    }
}

pub fn compute_element_style(
    element: &Element,
    parent_style: Option<&ComputedStyle>,
    css_rules: &[CssRule],
) -> ComputedStyle {
    compute_computed_style(&element.tag, css_rules, parent_style)
}

pub fn matches_selector(tag: &str, selector: &str) -> bool {
    selector.trim().eq_ignore_ascii_case(tag) || selector == "*"
}

pub fn create_layout_tree(
    root: &Element,
    css_rules: &[CssRule],
    viewport_width: u32,
) -> Option<LayoutNode> {
    let node = build_layout_node(root, None, css_rules, viewport_width)?;
    let mut root_node = node;
    perform_layout(&mut root_node, viewport_width);
    Some(root_node)
}

fn build_layout_node(
    element: &Element,
    parent_style: Option<&ComputedStyle>,
    css_rules: &[CssRule],
    viewport_width: u32,
) -> Option<LayoutNode> {
    let computed_style = compute_element_style(element, parent_style, css_rules);
    let display_type = parse_display_style(&computed_style, &element.tag);

    if display_type == DisplayType::None {
        return None;
    }

    let default_width = match display_type {
        DisplayType::Block => (viewport_width as i32 - 20).max(100),
        DisplayType::Inline => 100,
        DisplayType::None => 0,
    };

    let rect = RectModel {
        x: 0,
        y: 0,
        width: default_width,
        height: 30, // Default base height, updated during perform_layout
        margin_top: 5,
        margin_right: 5,
        margin_bottom: 5,
        margin_left: 5,
        padding_top: 2,
        padding_right: 2,
        padding_bottom: 2,
        padding_left: 2,
        border_top: 1,
        border_right: 1,
        border_bottom: 1,
        border_left: 1,
        display: display_type,
    };

    let mut layout_node = LayoutNode::new(element.clone(), rect);

    for child in &element.children {
        if let Some(child_layout) = build_layout_node(child, Some(&computed_style), css_rules, viewport_width) {
            layout_node.add_child(child_layout);
        }
    }

    Some(layout_node)
}

pub fn perform_layout(root: &mut LayoutNode, viewport_width: u32) {
    layout_node_recursive(root, 0, 0, viewport_width as i32);
}

fn layout_node_recursive(node: &mut LayoutNode, current_x: i32, current_y: i32, parent_width: i32) -> i32 {
    node.rect.x = current_x + node.rect.margin_left;
    node.rect.y = current_y + node.rect.margin_top;

    if node.rect.display == DisplayType::Block {
        node.rect.width = (parent_width - node.rect.margin_left - node.rect.margin_right).max(50);
    }

    let content_x = node.rect.x + node.rect.padding_left + node.rect.border_left;
    let content_y = node.rect.y + node.rect.padding_top + node.rect.border_top;
    let inner_width = node.rect.content_width();

    let mut child_y = content_y;
    let mut total_child_height = 0;

    for child in &mut node.children {
        let child_height = layout_node_recursive(child, content_x, child_y, inner_width);
        child_y += child_height + child.rect.margin_top + child.rect.margin_bottom;
        total_child_height += child_height + child.rect.margin_top + child.rect.margin_bottom;
    }

    let text_lines = if node.element.text.is_empty() { 0 } else { 1 };
    let text_height = text_lines * 20;

    node.rect.height = (total_child_height + text_height + node.rect.padding_top + node.rect.padding_bottom + node.rect.border_top + node.rect.border_bottom).max(25);
    node.rect.height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_display_block() {
        let mut style = ComputedStyle::default();
        style.display = Some("block".to_string());
        assert_eq!(parse_display_style(&style, "div"), DisplayType::Block);
    }
}

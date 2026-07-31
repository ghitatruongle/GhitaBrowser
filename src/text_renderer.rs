// src/text_renderer.rs - ASCII text rendering of layout tree (v0.5.0)
#![allow(dead_code)]

use super::layout::{DisplayType, LayoutNode};

pub struct TextRenderer {
    width: u32,
    height: u32,
}

impl TextRenderer {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn render_to_text(&self, layout: &LayoutNode) -> String {
        let mut output = String::new();
        output.push_str(&format!("╔═ GhitaBrowser v0.5.0 ═{}╗\n", "═".repeat(40)));
        self.render_node(layout, 0, &mut output);
        output.push_str(&format!("╚{}╝\n", "═".repeat(54)));
        output
    }

    fn render_node(&self, node: &LayoutNode, indent: usize, output: &mut String) {
        let space = "  ".repeat(indent);
        
        match node.rect.display {
            DisplayType::Block | DisplayType::ListItem => {
                let tag_display = if !node.element.tag.is_empty() && node.element.tag != "root" {
                    format!("<{}>", node.element.tag)
                } else {
                    String::new()
                };
                
                if !node.element.text.is_empty() {
                    output.push_str(&format!("{}{}{}\n", space, tag_display, node.element.text));
                } else if !tag_display.is_empty() {
                    output.push_str(&format!("{}{}\n", space, tag_display));
                }
                
                for child in &node.children {
                    self.render_node(child, indent + 1, output);
                }
            }
            DisplayType::Inline | DisplayType::InlineBlock => {
                if !node.element.text.is_empty() {
                    output.push_str(&format!("{}{}", space, node.element.text));
                    if indent == 0 || !node.children.is_empty() {
                        output.push('\n');
                    }
                }
                for child in &node.children {
                    self.render_node(child, indent, output);
                }
            }
            DisplayType::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Element;
    use crate::layout::{RectModel, DisplayType, LayoutNode};

    #[test]
    fn test_renderer_creation() {
        let tr = TextRenderer::new(800, 600);
        let elem = Element::new("div");
        let rect = RectModel {
            x: 0.0, y: 0.0, width: 100.0, height: 50.0,
            margin_top: 0.0, margin_right: 0.0, margin_bottom: 0.0, margin_left: 0.0,
            padding_top: 0.0, padding_right: 0.0, padding_bottom: 0.0, padding_left: 0.0,
            border_top: 0.0, border_right: 0.0, border_bottom: 0.0, border_left: 0.0,
            display: DisplayType::Block,
        };
        let style = crate::css_parser::ComputedStyle::default();
        let node = LayoutNode::new(elem, rect, style);
        let out = tr.render_to_text(&node);
        assert!(out.contains("GhitaBrowser"));
    }
}

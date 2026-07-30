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
        self.render_node(layout, 0, String::new())
    }

    fn render_node(&self, node: &LayoutNode, indent: usize, mut output: String) -> String {
        let rect_model = &node.rect;
        let space = "  ".repeat(indent);
        match rect_model.display {
            DisplayType::Block => {
                output.push_str(&format!("{}[BLOCK {}] {}", space, node.element.tag, node.element.text));
                for child in &node.children {
                    output = self.render_node(child, indent + 1, output);
                }
                output.push_str("\n");
            }
            DisplayType::Inline => {
                if !node.element.text.is_empty() {
                    output.push_str(&format!("{}<i>{}</i> ", space, node.element.text));
                }
                for child in &node.children {
                    output = self.render_node(child, indent, output);
                }
            }
            DisplayType::None => {}
        }
        output
    }
}

// Layout text renderer

use super::layout::{DisplayType, LayoutNode};

pub struct TextRenderer {
    pub width: u32,
    pub height: u32,
}

impl TextRenderer {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn render_to_text(&self, layout: &LayoutNode) -> String {
        let mut output = String::new();
        output.push_str(&format!(
            "╔═ GhitaBrowser v{} ═{}╗\n",
            crate::VERSION,
            "═".repeat(40)
        ));
        self.render_node(layout, 0, &mut output);
        output.push_str(&format!("╚{}╝\n", "═".repeat(54)));
        output
    }

    /// Iterative pre-order walk (explicit stack, no recursion) with a depth
    /// limit and an output cap so adversarially deep/wide layouts cannot
    /// overflow the stack or grow the string without bound.
    fn render_node(&self, node: &LayoutNode, indent: usize, output: &mut String) {
        const MAX_RENDER_DEPTH: usize = 512;
        const MAX_RENDER_BYTES: usize = 2 * 1024 * 1024;
        // Explicit (node, indent, depth) stack; children pushed in reverse so
        // output order matches the former recursive pre-order walk.
        let mut work: Vec<(&LayoutNode, usize, usize)> = vec![(node, indent, 0)];
        while let Some((current, current_indent, depth)) = work.pop() {
            if output.len() >= MAX_RENDER_BYTES {
                output.push_str("\n…[truncated: output cap reached]\n");
                return;
            }
            if depth > MAX_RENDER_DEPTH {
                continue;
            }
            Self::render_single(current, current_indent, depth, output, &mut work);
        }
    }

    /// Emit one node and push its children onto the iterative work stack.
    fn render_single<'a>(
        node: &'a LayoutNode,
        indent: usize,
        depth: usize,
        output: &mut String,
        work: &mut Vec<(&'a LayoutNode, usize, usize)>,
    ) {
        let space = "  ".repeat(indent);

        match node.rect.display {
            DisplayType::Block
            | DisplayType::ListItem
            | DisplayType::Flex
            | DisplayType::Grid
            | DisplayType::Table
            | DisplayType::TableRowGroup
            | DisplayType::TableHeaderGroup
            | DisplayType::TableFooterGroup
            | DisplayType::TableRow
            | DisplayType::TableCaption
            | DisplayType::FlowRoot => {
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

                for child in node.children.iter().rev() {
                    work.push((child, indent + 1, depth + 1));
                }
            }
            DisplayType::Inline | DisplayType::InlineBlock | DisplayType::TableCell => {
                if !node.element.text.is_empty() {
                    output.push_str(&format!("{}{}", space, node.element.text));
                    if indent == 0 || !node.children.is_empty() {
                        output.push('\n');
                    }
                }
                for child in node.children.iter().rev() {
                    work.push((child, indent, depth + 1));
                }
            }
            DisplayType::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{DisplayType, LayoutNode, RectModel};
    use crate::parser::Element;

    #[test]
    fn test_renderer_creation() {
        let tr = TextRenderer::new(800, 600);
        let elem = Element::new("div");
        let rect = RectModel {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 50.0,
            margin_top: 0.0,
            margin_right: 0.0,
            margin_bottom: 0.0,
            margin_left: 0.0,
            padding_top: 0.0,
            padding_right: 0.0,
            padding_bottom: 0.0,
            padding_left: 0.0,
            border_top: 0.0,
            border_right: 0.0,
            border_bottom: 0.0,
            border_left: 0.0,
            display: DisplayType::Block,
        };
        let style = crate::css_parser::ComputedStyle::default();
        let node = LayoutNode::new(elem, rect, style, "GhitaBrowser".to_string());
        let out = tr.render_to_text(&node);
        assert!(out.contains("GhitaBrowser"));
    }
}

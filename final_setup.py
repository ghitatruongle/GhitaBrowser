#!/usr/bin/env python3
import os

os.makedirs('src', exist_ok=True)

layout = '''use super::parser::Element;
use super::css_parser::{ComputedStyle, CssRule};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayType { Block, Inline, None }

#[derive(Debug, Clone, Copy)]
pub struct RectModel {
    pub x: i32, pub y: i32, pub width: i32, pub height: i32,
    pub margin_top: i32, pub margin_right: i32, pub margin_bottom: i32, pub margin_left: i32,
    pub padding_top: i32, pub padding_right: i32, pub padding_bottom: i32, pub padding_left: i32,
    pub border_top: i32, pub border_right: i32, pub border_bottom: i32, pub border_left: i32,
    pub display: DisplayType,
}

impl RectModel {
    pub fn content_width(&self) -> i32 { self.width - self.padding_left - self.padding_right - self.border_left - self.border_right }
    pub fn content_height(&self) -> i32 { self.height - self.padding_top - self.padding_bottom - self.border_top - self.border_bottom }
}

#[derive(Debug)]
pub struct LayoutNode {
    pub element: Element,
    pub rect: RectModel,
    pub children: Vec<LayoutNode>,
}

impl LayoutNode {
    pub fn new(element: Element, rect: RectModel) -> Self {
        Self { element, rect, children: Vec::new() }
    }
    pub fn add_child(&mut self, child: LayoutNode) { self.children.push(child); }
}

fn parse_display_style(style: &ComputedStyle) -> DisplayType {
    match style.display.as_deref() {
        Some("block") => DisplayType::Block, Some("inline") => DisplayType::Inline, Some("none") => DisplayType::None, _ => DisplayType::Block,
    }
}

pub fn create_layout_tree(root: &Element, css_rules: &[CssRule], _viewport_width: u32) -> Option<LayoutNode> {
    build_layout_node(root, None, css_rules)
}

fn build_layout_node(
    element: &Element,
    parent_style: Option<&ComputedStyle>,
    css_rules: &[CssRule],
) -> Option<LayoutNode> {
    let computed_style = compute_element_style(element, parent_style, css_rules);
    
    if computed_style.display == Some("None".to_string()) { return None; }
    
    let mut display_type = parse_display_style(&computed_style);
    
    let rect = RectModel {
        x: 0, y: 0, width: 800, height: 100,
        margin_top: 10, margin_right: 10, margin_bottom: 10, margin_left: 10,
        padding_top: 5, padding_right: 5, padding_bottom: 5, padding_left: 5,
        border_top: 1, border_right: 1, border_bottom: 1, border_left: 1,
        display: display_type,
    };
    
    let mut layout_node = LayoutNode::new(element.clone(), rect);
    
    for child in &element.children {
        if let Some(child_layout) = build_layout_node(child, Some(&computed_style), css_rules) {
            layout_node.add_child(child_layout);
        }
    }
    
    Some(layout_node)
}

fn get_margin(_style: &ComputedStyle, _side: &str) -> i32 { 10 }
fn get_padding(_style: &ComputedStyle, _side: &str) -> i32 { 5 }

pub fn compute_element_style(_element: &Element, _parent_style: Option<&ComputedStyle>, _css_rules: &[CssRule]) -> ComputedStyle {
    ComputedStyle::default()
}

pub fn matches_selector(tag: &str, selector: &str) -> bool { tag == selector }

pub fn perform_layout(_root: &mut LayoutNode, _viewport_width: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Element;
    #[test]
    fn test_parse_display_block() {
        let mut style = ComputedStyle::default();
        style.display = Some("block".to_string());
        assert_eq!(parse_display_style(&style), DisplayType::Block);
    }
}
'''

text_renderer = '''use super::parser::Element;
use super::layout::{LayoutNode, RectModel, DisplayType};

pub struct TextRenderer {
    width: u32,
    height: u32,
}

impl TextRenderer {
    pub fn new(width: u32, height: u32) -> Self { Self { width, height } }
    pub fn render_to_text(&self, layout: &LayoutNode) -> String { self.render_node(layout, 0, String::new()) }
    fn render_node(&self, node: &LayoutNode, indent: usize, mut output: String) -> String {
        let rect_model = &node.rect;
        let space = "  ".repeat(indent);
        match rect_model.display {
            DisplayType::Block => {
                output.push_str(&format!("{}[BLOCK {}] {}", space, node.element.tag, node.element.text));
                for child in &node.children { output = self.render_node(child, indent + 1, output); }
                output.push_str("\n");
            },
            DisplayType::Inline => {
                if !node.element.text.is_empty() {
                    output.push_str(&format!("{}<i>{}</i> ", space, node.element.text));
                }
                for child in &node.children { output = self.render_node(child, indent, output); }
            },
            DisplayType::None => {}
        }
        output
    }
}
'''

main_content = '''mod parser;
mod layout;
mod text_renderer;
mod renderer;
mod image_loader;

use parser::{Element, parse_html};
use layout::{LayoutNode, create_layout_tree, perform_layout};
use text_renderer::TextRenderer;

fn main() {
    println!("\u267B GhitaBrowser v0.1.0");
    let test_html = "<html><body><h1>Welcome</h1></body></html>";
    let dom = parse_html(test_html);
    let css_rules: Vec<_> = vec![];
    match create_layout_tree(&dom, &css_rules, 1024) {
        Some(mut root) => {
            perform_layout(&mut root, 1024);
            let tr = TextRenderer::new(1024, 768);
            let out = tr.render_to_text(&root);
            println!("{}", out);
            println!("\u2705 Working!");
        },
        None => println!("Error"),
    }
}
'''

# Write files
with open('src/layout.rs', 'w') as f: f.write(layout)
with open('src/text_renderer.rs', 'w') as f: f.write(text_renderer)
with open('src/main.rs', 'w') as f: f.write(main_content)

print('Files written successfully:')
for f in ['src/layout.rs', 'src/text_renderer.rs', 'src/main.rs']:
    print(f'  {f}: {len(open(f).read())} bytes')

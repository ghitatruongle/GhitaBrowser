// src/layout.rs - Advanced Layout Engine with Text Wrapping (v0.5.0)
#![allow(dead_code)]

use crate::parser::Element;
use crate::css_parser::{CssRule, ComputedStyle, compute_computed_style, parse_class_attr};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayType {
    Block,
    Inline,
    InlineBlock,
    None,
    ListItem,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectModel {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub margin_top: f64,
    pub margin_right: f64,
    pub margin_bottom: f64,
    pub margin_left: f64,
    pub padding_top: f64,
    pub padding_right: f64,
    pub padding_bottom: f64,
    pub padding_left: f64,
    pub border_top: f64,
    pub border_right: f64,
    pub border_bottom: f64,
    pub border_left: f64,
    pub display: DisplayType,
}

impl RectModel {
    pub fn content_width(&self) -> f64 {
        (self.width - self.padding_left - self.padding_right - self.border_left - self.border_right).max(0.0)
    }

    pub fn content_height(&self) -> f64 {
        (self.height - self.padding_top - self.padding_bottom - self.border_top - self.border_bottom).max(0.0)
    }
    
    pub fn outer_width(&self) -> f64 {
        self.width + self.margin_left + self.margin_right
    }
    
    pub fn outer_height(&self) -> f64 {
        self.height + self.margin_top + self.margin_bottom
    }
}

#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub element: Element,
    pub rect: RectModel,
    pub children: Vec<LayoutNode>,
    pub computed_style: ComputedStyle,
}

impl LayoutNode {
    pub fn new(element: Element, rect: RectModel, style: ComputedStyle) -> Self {
        Self {
            element,
            rect,
            children: Vec::new(),
            computed_style: style,
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
            "inline" => DisplayType::Inline,
            "inline-block" => DisplayType::InlineBlock,
            "none" => DisplayType::None,
            "list-item" => DisplayType::ListItem,
            _ => default_display_for_tag(tag),
        }
    } else {
        default_display_for_tag(tag)
    }
}

fn default_display_for_tag(tag: &str) -> DisplayType {
    match tag {
        "span" | "a" | "i" | "b" | "em" | "strong" | "img" | "code" | "label" => DisplayType::Inline,
        "head" | "script" | "style" | "meta" | "link" | "noscript" => DisplayType::None,
        "li" => DisplayType::ListItem,
        _ => DisplayType::Block,
    }
}

/// Estimate text width for a given text in pixels (monospace approximation)
pub fn estimate_text_width(text: &str, font_size: f64) -> f64 {
    // Simple character-width estimation (average char width ≈ 0.6 * font_size)
    let char_width = font_size * 0.6;
    text.chars().count() as f64 * char_width
}

/// Get font size from computed style
pub fn get_font_size(style: &ComputedStyle, parent_font_size: f64) -> f64 {
    style.font_size
        .as_ref()
        .map(|fs| fs.to_pixels(parent_font_size, 16.0))
        .unwrap_or(parent_font_size)
}

/// UA-stylesheet default font size per tag (like Chrome's built-in styles)
pub fn default_font_size_for_tag(tag: &str, parent_font_size: f64) -> f64 {
    match tag {
        "h1" => 32.0,
        "h2" => 24.0,
        "h3" => 18.72,
        "h4" => 16.0,
        "h5" => 13.28,
        "h6" => 10.72,
        "small" | "sub" | "sup" => 13.28,
        "code" | "pre" | "kbd" | "samp" => 13.0,
        _ => parent_font_size,
    }
}

/// Effective font size: CSS value if present, otherwise UA default for the tag
pub fn effective_font_size(style: &ComputedStyle, tag: &str, parent_font_size: f64) -> f64 {
    style.font_size
        .as_ref()
        .map(|fs| fs.to_pixels(parent_font_size, 16.0))
        .unwrap_or_else(|| default_font_size_for_tag(tag, parent_font_size))
}

/// Wrap text to fit within a given width
pub fn wrap_text(text: &str, max_width: f64, font_size: f64) -> Vec<String> {
    if max_width <= 0.0 || text.is_empty() {
        return vec![text.to_string()];
    }
    
    let char_width = font_size * 0.6;
    let max_chars = (max_width / char_width).max(1.0) as usize;
    
    let mut lines = Vec::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut current_line = String::new();
    
    for word in words {
        if current_line.is_empty() {
            current_line = word.to_string();
        } else if current_line.len() + 1 + word.len() <= max_chars {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(current_line);
            current_line = word.to_string();
        }
    }
    
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    
    if lines.is_empty() {
        lines.push(text.to_string());
    }
    
    lines
}

/// Build a complete layout tree from DOM + CSS rules
pub fn create_layout_tree(
    root: &Element,
    css_rules: &[CssRule],
    viewport_width: u32,
) -> Option<LayoutNode> {
    let (node, _) = build_layout_node(root, None, css_rules, viewport_width as f64, 16.0)?;
    let mut root_node = node;
    perform_layout(&mut root_node, viewport_width as f64);
    Some(root_node)
}

fn build_layout_node(
    element: &Element,
    parent_style: Option<&ComputedStyle>,
    css_rules: &[CssRule],
    viewport_width: f64,
    parent_font_size: f64,
) -> Option<(LayoutNode, ComputedStyle)> {
    let classes = parse_class_attr(element.get_attr("class").map(|s| s.as_str()));
    let elem_id = element.get_attr("id").map(|s| s.as_str());
    
    let computed_style = compute_computed_style(
        &element.tag,
        &classes,
        elem_id,
        css_rules,
        parent_style,
    );
    
    let display_type = parse_display_style(&computed_style, &element.tag);
    let font_size = effective_font_size(&computed_style, &element.tag, parent_font_size);

    if display_type == DisplayType::None {
        return None;
    }

    // Compute box model values
    let margin_top = computed_style.margin_top.as_ref()
        .map(|m| m.to_pixels(viewport_width, 16.0)).unwrap_or(0.0);
    let margin_right = computed_style.margin_right.as_ref()
        .map(|m| m.to_pixels(viewport_width, 16.0)).unwrap_or(0.0);
    let margin_bottom = computed_style.margin_bottom.as_ref()
        .map(|m| m.to_pixels(viewport_width, 16.0)).unwrap_or(0.0);
    let margin_left = computed_style.margin_left.as_ref()
        .map(|m| m.to_pixels(viewport_width, 16.0)).unwrap_or(0.0);
    let padding_top = computed_style.padding_top.as_ref()
        .map(|m| m.to_pixels(viewport_width, 16.0)).unwrap_or(0.0);
    let padding_right = computed_style.padding_right.as_ref()
        .map(|m| m.to_pixels(viewport_width, 16.0)).unwrap_or(0.0);
    let padding_bottom = computed_style.padding_bottom.as_ref()
        .map(|m| m.to_pixels(viewport_width, 16.0)).unwrap_or(0.0);
    let padding_left = computed_style.padding_left.as_ref()
        .map(|m| m.to_pixels(viewport_width, 16.0)).unwrap_or(0.0);

    let default_width = match display_type {
        DisplayType::Block | DisplayType::ListItem => {
            viewport_width - margin_left - margin_right
        }
        DisplayType::Inline | DisplayType::InlineBlock => {
            estimate_text_width(&element.text, font_size) + padding_left + padding_right
        }
        DisplayType::None => 0.0,
    };

    // Resolve width from CSS
    let width = computed_style.width.as_ref()
        .map(|w| w.to_pixels(viewport_width, 16.0))
        .unwrap_or(default_width);

    let rect = RectModel {
        x: 0.0,
        y: 0.0,
        width: width.max(0.0),
        height: 0.0, // Computed during perform_layout
        margin_top,
        margin_right,
        margin_bottom,
        margin_left,
        padding_top,
        padding_right,
        padding_bottom,
        padding_left,
        border_top: 0.0, // Simplified
        border_right: 0.0,
        border_bottom: 0.0,
        border_left: 0.0,
        display: display_type,
    };

    let mut layout_node = LayoutNode::new(element.clone(), rect, computed_style);

    // Build children
    for child in &element.children {
        if let Some((child_layout, _)) = build_layout_node(
            child,
            Some(&layout_node.computed_style),
            css_rules,
            layout_node.rect.content_width(),
            font_size,
        ) {
            layout_node.add_child(child_layout);
        }
    }

    let style = layout_node.computed_style.clone();
    Some((layout_node, style))
}

/// Perform layout: position elements and compute sizes
pub fn perform_layout(root: &mut LayoutNode, viewport_width: f64) {
    layout_node_recursive(root, 0.0, 0.0, viewport_width, 16.0);
}

fn layout_node_recursive(
    node: &mut LayoutNode,
    current_x: f64,
    current_y: f64,
    parent_width: f64,
    parent_font_size: f64,
) -> f64 {
    let font_size = effective_font_size(&node.computed_style, &node.element.tag, parent_font_size);
    
    // Set position
    node.rect.x = current_x + node.rect.margin_left;
    node.rect.y = current_y + node.rect.margin_top;

    // Block/inline width calculation
    match node.rect.display {
        DisplayType::Block | DisplayType::ListItem => {
            node.rect.width = (parent_width - node.rect.margin_left - node.rect.margin_right)
                .max(0.0);
        }
        DisplayType::Inline | DisplayType::InlineBlock => {
            if node.rect.width == 0.0 {
                node.rect.width = estimate_text_width(&node.element.text, font_size)
                    + node.rect.padding_left + node.rect.padding_right;
            }
        }
        DisplayType::None => {}
    }

    // Content area
    let content_x = node.rect.x + node.rect.padding_left + node.rect.border_left;
    let content_y = node.rect.y + node.rect.padding_top + node.rect.border_top;
    let inner_width = node.rect.content_width();

    // Text wrapping
    let text_lines = if node.element.text.is_empty() {
        Vec::new()
    } else {
        wrap_text(&node.element.text, inner_width, font_size)
    };
    let text_height = text_lines.len() as f64 * (font_size * 1.4); // line height

    // If this node has BOTH direct text and element children, reserve room for the
    // text so children are laid out below it instead of overlapping (visible now
    // that pages are painted with real pixels).
    let own_text_height = if node.children.is_empty() { 0.0 } else { text_height };

    // Inline formatting context: inline / inline-block children flow horizontally
    // and wrap to a new line when they no longer fit; block / list-item children
    // always break onto their own full-width line (like Chrome).
    let start_y = content_y + own_text_height;
    let mut line_x = content_x;
    let mut line_y = start_y;
    let mut line_height: f64 = 0.0;
    let default_line = font_size * 1.4;

    for child in &mut node.children {
        let is_inline = matches!(
            child.rect.display,
            DisplayType::Inline | DisplayType::InlineBlock
        );

        if is_inline {
            // Width is known from the build step (text width + padding); use it to
            // decide whether the inline box still fits on the current line.
            let child_outer = child.rect.width
                + child.rect.margin_left + child.rect.margin_right;
            if line_x > content_x && line_x + child_outer > content_x + inner_width {
                // Wrap to the next line
                line_y += if line_height > 0.0 { line_height } else { default_line };
                line_x = content_x;
                line_height = 0.0;
            }
            let child_height = layout_node_recursive(child, line_x, line_y, inner_width, font_size);
            line_x += child.rect.outer_width();
            line_height = line_height.max(child_height);
        } else {
            // Block-level child: finish the current inline line first
            if line_x > content_x {
                line_y += if line_height > 0.0 { line_height } else { default_line };
                line_x = content_x;
                line_height = 0.0;
            }
            let child_height = layout_node_recursive(child, content_x, line_y, inner_width, font_size);
            line_y += child_height;
        }
    }
    // Include the height of the last (unfinished) inline line
    if line_x > content_x {
        line_y += if line_height > 0.0 { line_height } else { default_line };
    }

    // Compute final height
    let content_height = if node.children.is_empty() {
        text_height
    } else {
        (line_y - content_y).max(own_text_height)
    };

    node.rect.height = (content_height
        + node.rect.padding_top + node.rect.padding_bottom
        + node.rect.border_top + node.rect.border_bottom)
        .max(font_size * 1.4); // Minimum height for one line

    node.rect.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css_parser;

    #[test]
    fn test_parse_display_block() {
        let mut style = ComputedStyle::default();
        style.display = Some("block".to_string());
        assert_eq!(parse_display_style(&style, "div"), DisplayType::Block);
    }

    #[test]
    fn test_wrap_text_simple() {
        let lines = wrap_text("Hello World", 200.0, 16.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "Hello World");
    }

    #[test]
    fn test_wrap_text_long() {
        let text = "This is a very long text that should be wrapped into multiple lines because it exceeds the available width";
        let lines = wrap_text(text, 100.0, 16.0);
        assert!(lines.len() > 1);
    }

    #[test]
    fn test_estimate_text_width() {
        let w = estimate_text_width("Hello", 16.0);
        assert!(w > 0.0);
        assert_eq!(w, 5.0 * 16.0 * 0.6);
    }

    #[test]
    fn test_layout_creation() {
        let html = "<html><body><h1>Title</h1><p>Paragraph text</p></body></html>";
        let dom = crate::parser::parse_html(html);
        let css = "body { font-family: Arial; }";
        let rules = css_parser::parse_css(css);
        
        if let Some(mut layout) = create_layout_tree(&dom, &rules, 800) {
            perform_layout(&mut layout, 800.0);
            assert!(layout.rect.width > 0.0);
            assert!(layout.rect.height > 0.0);
        }
    }

    #[test]
    fn test_layout_with_css() {
        let html = "<div class='content'><p>Hello</p></div>";
        let dom = crate::parser::parse_html(html);
        let css = ".content { margin: 10px; padding: 5px; } p { color: red; }";
        let rules = css_parser::parse_css(css);
        
        if let Some(mut layout) = create_layout_tree(&dom, &rules, 1024) {
            perform_layout(&mut layout, 1024.0);
            let p_node = &layout.children[0];
            assert_eq!(p_node.element.tag, "p");
            assert_eq!(p_node.computed_style.color, Some("red".to_string()));
        }
    }

    #[test]
    fn test_display_none() {
        let html = "<div><script>var x=1;</script><p>Visible</p></div>";
        let dom = crate::parser::parse_html(html);
        let rules = vec![];
        
        let (root, _) = build_layout_node(&dom, None, &rules, 800.0, 16.0).unwrap();
        assert_eq!(root.children.len(), 1); // script should be filtered out
        assert_eq!(root.children[0].element.tag, "p");
    }

    #[test]
    fn test_inline_children_share_a_line() {
        // Two inline links in a block should sit on the same line, side by side
        let html = "<div><a href='#'>Home</a><a href='#'>About</a></div>";
        let dom = crate::parser::parse_html(html);
        let rules: Vec<css_parser::CssRule> = vec![];
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);
        let links: Vec<&LayoutNode> = layout.children.iter()
            .filter(|c| c.element.tag == "a").collect();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].rect.y, links[1].rect.y, "inline links must share a line");
        assert!(links[1].rect.x > links[0].rect.x, "second link must be to the right");
    }

    #[test]
    fn test_block_children_stack_vertically() {
        let html = "<div><p>One</p><p>Two</p></div>";
        let dom = crate::parser::parse_html(html);
        let rules: Vec<css_parser::CssRule> = vec![];
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);
        let ps: Vec<&LayoutNode> = layout.children.iter()
            .filter(|c| c.element.tag == "p").collect();
        assert_eq!(ps.len(), 2);
        assert!(ps[1].rect.y > ps[0].rect.y, "block children must stack vertically");
    }
}

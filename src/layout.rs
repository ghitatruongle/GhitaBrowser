use crate::css_parser::{
    compute_computed_style, parse_class_attr, ComputedStyle, CssRule, CssUnit, PositionMode,
    Transform2D,
};
use crate::parser::Element;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DisplayType {
    Block,
    Inline,
    InlineBlock,
    Flex,
    Grid,
    Table,
    TableRowGroup,
    TableHeaderGroup,
    TableFooterGroup,
    TableRow,
    TableCell,
    TableCaption,
    FlowRoot,
    None,
    ListItem,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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
        (self.width - self.padding_left - self.padding_right - self.border_left - self.border_right)
            .max(0.0)
    }

    pub fn content_height(&self) -> f64 {
        (self.height
            - self.padding_top
            - self.padding_bottom
            - self.border_top
            - self.border_bottom)
            .max(0.0)
    }

    pub fn padding_box_width(&self) -> f64 {
        (self.width - self.border_left - self.border_right).max(0.0)
    }

    pub fn padding_box_height(&self) -> f64 {
        (self.height - self.border_top - self.border_bottom).max(0.0)
    }

    pub fn border_box_width(&self) -> f64 {
        self.width.max(0.0)
    }

    pub fn border_box_height(&self) -> f64 {
        self.height.max(0.0)
    }

    pub fn outer_width(&self) -> f64 {
        self.width + self.margin_left + self.margin_right
    }

    pub fn outer_height(&self) -> f64 {
        self.height + self.margin_top + self.margin_bottom
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayoutNode {
    pub element: Element,
    pub rect: RectModel,
    pub children: Vec<LayoutNode>,
    pub computed_style: ComputedStyle,
    /// Full descendant text, precomputed at build time. `element` carries a
    /// CHILDLESS copy (a per-node subtree clone was O(n·depth) memory), so
    /// inline width estimation still has the whole text available.
    pub desc_text: String,
}

impl LayoutNode {
    pub fn new(element: Element, rect: RectModel, style: ComputedStyle, desc_text: String) -> Self {
        Self {
            element,
            rect,
            children: Vec::new(),
            computed_style: style,
            desc_text,
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
            "flex" | "inline-flex" => DisplayType::Flex,
            "grid" | "inline-grid" => DisplayType::Grid,
            "table" | "inline-table" => DisplayType::Table,
            "table-row-group" | "tbody" => DisplayType::TableRowGroup,
            "table-header-group" | "thead" => DisplayType::TableHeaderGroup,
            "table-footer-group" | "tfoot" => DisplayType::TableFooterGroup,
            "table-row" | "tr" => DisplayType::TableRow,
            "table-cell" | "td" | "th" => DisplayType::TableCell,
            "table-caption" | "caption" => DisplayType::TableCaption,
            "flow-root" => DisplayType::FlowRoot,
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
        "span" | "a" | "i" | "b" | "em" | "strong" | "img" | "code" | "label" | "small" | "sub"
        | "sup" | "abbr" | "cite" | "kbd" | "time" | "var" => DisplayType::Inline,
        "input" | "button" | "select" | "textarea" => DisplayType::InlineBlock,
        "head" | "script" | "style" | "meta" | "link" | "noscript" => DisplayType::None,
        "li" => DisplayType::ListItem,
        "table" => DisplayType::Table,
        "thead" => DisplayType::TableHeaderGroup,
        "tbody" => DisplayType::TableRowGroup,
        "tfoot" => DisplayType::TableFooterGroup,
        "tr" => DisplayType::TableRow,
        "td" | "th" => DisplayType::TableCell,
        "caption" => DisplayType::TableCaption,
        _ => DisplayType::Block,
    }
}

/// Estimate text width for a given text in pixels with proportional advances
pub fn estimate_text_width(text: &str, font_size: f64) -> f64 {
    text.chars()
        .map(|c| {
            if is_cjk(c) {
                font_size
            } else if matches!(
                c,
                'i' | 'l'
                    | 'j'
                    | 't'
                    | '!'
                    | '.'
                    | ':'
                    | ';'
                    | '\''
                    | '`'
                    | '|'
                    | ' '
                    | ','
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '/'
                    | '\\'
                    | '-'
            ) {
                font_size * 0.32
            } else if matches!(c, 'm' | 'w' | 'M' | 'W' | '@' | '%' | '&' | '#' | '+') {
                font_size * 0.85
            } else if c.is_ascii_uppercase() {
                font_size * 0.70
            } else {
                font_size * 0.55
            }
        })
        .sum()
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}' |   // CJK Unified
        '\u{3400}'..='\u{4DBF}' |   // CJK Ext A
        '\u{F900}'..='\u{FAFF}' |   // CJK Compat
        '\u{3000}'..='\u{303F}' |   // CJK Symbols/Punctuation
        '\u{1F300}'..='\u{1F9FF}'   // Emoji & Symbols
    )
}

/// Get font size from computed style
pub fn get_font_size(style: &ComputedStyle, parent_font_size: f64) -> f64 {
    style
        .font_size
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
    style
        .font_size
        .as_ref()
        .map(|fs| fs.to_pixels(parent_font_size, 16.0))
        .unwrap_or_else(|| default_font_size_for_tag(tag, parent_font_size))
}

/// Wrap text to fit within a given width, respecting CSS whitespace and word-break rules
pub fn wrap_text(text: &str, max_width: f64, font_size: f64) -> Vec<String> {
    wrap_text_with_rules(text, max_width, font_size, None, None)
}

pub fn wrap_text_with_rules(
    text: &str,
    max_width: f64,
    font_size: f64,
    white_space: Option<&str>,
    word_break: Option<&str>,
) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    if let Some(ws) = white_space {
        if ws == "nowrap" {
            return vec![text.replace('\n', " ")];
        }
        if ws == "pre" || ws == "pre-wrap" {
            let mut result = Vec::new();
            for raw_line in text.split('\n') {
                if ws == "pre" || max_width <= 0.0 {
                    result.push(raw_line.to_string());
                } else {
                    let wrapped = wrap_single_line(raw_line, max_width, font_size, word_break);
                    result.extend(wrapped);
                }
            }
            return if result.is_empty() {
                vec![text.to_string()]
            } else {
                result
            };
        }
    }

    if max_width <= 0.0 {
        return vec![text.to_string()];
    }

    wrap_single_line(text, max_width, font_size, word_break)
}

fn wrap_single_line(
    text: &str,
    max_width: f64,
    font_size: f64,
    _word_break: Option<&str>,
) -> Vec<String> {
    if estimate_text_width(text, font_size) <= max_width + 1.0 {
        return vec![text.to_string()];
    }

    let char_width = (font_size * 0.58).max(1.0);
    let max_chars = (max_width / char_width).max(1.0) as usize;

    let mut lines = Vec::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut current_line = String::new();

    for word in words {
        let word_chars = word.chars().count();
        let cjk = word.chars().any(is_cjk);
        if cjk || word_chars > max_chars {
            let mut remaining: String = word.to_string();
            loop {
                if remaining.is_empty() {
                    break;
                }
                let line_chars = current_line.chars().count();
                let room =
                    max_chars.saturating_sub(line_chars + if line_chars == 0 { 0 } else { 1 });
                if room == 0 {
                    lines.push(current_line);
                    current_line = String::new();
                    continue;
                }
                let take = room.min(remaining.chars().count());
                let take_str: String = remaining.chars().take(take).collect();
                if current_line.is_empty() {
                    current_line = take_str.clone();
                } else {
                    current_line.push(' ');
                    current_line.push_str(&take_str);
                }
                remaining = remaining.chars().skip(take_str.chars().count()).collect();
                if !remaining.is_empty() {
                    lines.push(current_line);
                    current_line = String::new();
                }
            }
            continue;
        }
        if current_line.is_empty() {
            current_line = word.to_string();
        } else if current_line.chars().count() + 1 + word_chars <= max_chars {
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

/// Whether a computed style sets a real (non-auto) width.
fn has_explicit_width(style: &ComputedStyle) -> bool {
    matches!(
        style.width,
        Some(
            CssUnit::Pixels(_)
                | CssUnit::Percent(_)
                | CssUnit::Em(_)
                | CssUnit::Rem(_)
                | CssUnit::Vw(_)
                | CssUnit::Vh(_)
                | CssUnit::Calc(_)
        )
    )
}

/// Build a complete layout tree from DOM + CSS rules
pub fn create_layout_tree(
    root: &Element,
    css_rules: &[CssRule],
    viewport_width: u32,
) -> Option<LayoutNode> {
    create_layout_tree_with_optional_styles(root, css_rules, viewport_width, None)
}

/// Build a layout tree using a caller-owned computed-style snapshot
pub fn create_layout_tree_with_styles(
    root: &Element,
    css_rules: &[CssRule],
    viewport_width: u32,
    styles: &BTreeMap<u64, ComputedStyle>,
) -> Option<LayoutNode> {
    create_layout_tree_with_optional_styles(root, css_rules, viewport_width, Some(styles))
}

fn create_layout_tree_with_optional_styles(
    root: &Element,
    css_rules: &[CssRule],
    viewport_width: u32,
    styles: Option<&BTreeMap<u64, ComputedStyle>>,
) -> Option<LayoutNode> {
    let (node, _) = build_layout_node(root, None, css_rules, viewport_width as f64, 16.0, styles)?;
    let mut root_node = node;
    perform_layout(&mut root_node, viewport_width as f64);
    Some(root_node)
}

fn resolve_box_dimension(
    unit: Option<&CssUnit>,
    container_size: f64,
    parent_font_size: f64,
) -> Option<f64> {
    unit.map(|u| u.to_pixels(container_size, parent_font_size).max(0.0))
}

fn build_layout_node(
    element: &Element,
    parent_style: Option<&ComputedStyle>,
    css_rules: &[CssRule],
    viewport_width: f64,
    parent_font_size: f64,
    styles: Option<&BTreeMap<u64, ComputedStyle>>,
) -> Option<(LayoutNode, ComputedStyle)> {
    let classes = parse_class_attr(element.get_attr("class").map(|s| s.as_str()));
    let elem_id = element.get_attr("id").map(|s| s.as_str());

    let computed_style = element
        .node_id
        .and_then(|node| styles.and_then(|styles| styles.get(&node)))
        .cloned()
        .unwrap_or_else(|| {
            compute_computed_style(
                &element.tag,
                &classes,
                elem_id,
                css_rules,
                parent_style,
                &element.attrs,
            )
        });

    let display_type = parse_display_style(&computed_style, &element.tag);
    let font_size = effective_font_size(&computed_style, &element.tag, parent_font_size);

    if display_type == DisplayType::None {
        return None;
    }

    let is_border_box = computed_style.box_sizing.as_deref() == Some("border-box");

    // Margins (0.0 if Auto or None)
    let margin_top =
        resolve_box_dimension(computed_style.margin_top.as_ref(), viewport_width, 16.0)
            .unwrap_or(0.0);
    let margin_right =
        resolve_box_dimension(computed_style.margin_right.as_ref(), viewport_width, 16.0)
            .unwrap_or(0.0);
    let margin_bottom =
        resolve_box_dimension(computed_style.margin_bottom.as_ref(), viewport_width, 16.0)
            .unwrap_or(0.0);
    let margin_left =
        resolve_box_dimension(computed_style.margin_left.as_ref(), viewport_width, 16.0)
            .unwrap_or(0.0);

    // Padding
    let padding_top =
        resolve_box_dimension(computed_style.padding_top.as_ref(), viewport_width, 16.0)
            .unwrap_or(0.0);
    let padding_right =
        resolve_box_dimension(computed_style.padding_right.as_ref(), viewport_width, 16.0)
            .unwrap_or(0.0);
    let padding_bottom =
        resolve_box_dimension(computed_style.padding_bottom.as_ref(), viewport_width, 16.0)
            .unwrap_or(0.0);
    let padding_left =
        resolve_box_dimension(computed_style.padding_left.as_ref(), viewport_width, 16.0)
            .unwrap_or(0.0);

    // Borders
    let border_top = resolve_box_dimension(
        computed_style
            .border_top_width
            .as_ref()
            .or(computed_style.border_width.as_ref()),
        viewport_width,
        16.0,
    )
    .unwrap_or(0.0);
    let border_right = resolve_box_dimension(
        computed_style
            .border_right_width
            .as_ref()
            .or(computed_style.border_width.as_ref()),
        viewport_width,
        16.0,
    )
    .unwrap_or(0.0);
    let border_bottom = resolve_box_dimension(
        computed_style
            .border_bottom_width
            .as_ref()
            .or(computed_style.border_width.as_ref()),
        viewport_width,
        16.0,
    )
    .unwrap_or(0.0);
    let border_left = resolve_box_dimension(
        computed_style
            .border_left_width
            .as_ref()
            .or(computed_style.border_width.as_ref()),
        viewport_width,
        16.0,
    )
    .unwrap_or(0.0);

    let h_padding_border = padding_left + padding_right + border_left + border_right;
    let _v_padding_border = padding_top + padding_bottom + border_top + border_bottom;

    // Attributes for replaced elements
    let is_img = element.tag == "img";
    let img_width_attr = if is_img {
        element
            .get_attr("width")
            .and_then(|v| v.parse::<f64>().ok())
    } else {
        None
    };

    let form_text = match element.tag.as_str() {
        "input" => element
            .get_attr("value")
            .or_else(|| element.get_attr("placeholder"))
            .cloned()
            .unwrap_or_default(),
        "button" | "select" | "textarea" => element.text_content(),
        _ => element.text_content(),
    };
    let desc_text = form_text.clone();

    let default_width = match display_type {
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
        | DisplayType::FlowRoot => (viewport_width - margin_left - margin_right).max(0.0),
        DisplayType::Inline | DisplayType::InlineBlock | DisplayType::TableCell => {
            if is_img {
                if let Some(w) = img_width_attr {
                    w + h_padding_border
                } else {
                    200.0_f64.min(viewport_width - margin_left - margin_right)
                }
            } else {
                let text_width = estimate_text_width(&desc_text, font_size);
                if matches!(element.tag.as_str(), "input" | "select" | "textarea") {
                    text_width.max(160.0) + h_padding_border
                } else if element.tag == "button" {
                    text_width.max(48.0) + h_padding_border
                } else {
                    text_width + h_padding_border
                }
            }
        }
        DisplayType::None => 0.0,
    };

    // Width resolution with box-sizing
    let mut width = if let Some(ref w) = computed_style.width {
        let px = w.to_pixels(viewport_width, 16.0);
        if is_border_box {
            px
        } else {
            px + h_padding_border
        }
    } else {
        default_width
    };

    // Apply min-width and max-width
    if let Some(ref min_w) = computed_style.min_width {
        let min_px = min_w.to_pixels(viewport_width, 16.0);
        let min_border_box = if is_border_box {
            min_px
        } else {
            min_px + h_padding_border
        };
        width = width.max(min_border_box);
    }
    if let Some(ref max_w) = computed_style.max_width {
        let max_px = max_w.to_pixels(viewport_width, 16.0);
        let max_border_box = if is_border_box {
            max_px
        } else {
            max_px + h_padding_border
        };
        width = width.min(max_border_box);
    }

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
        border_top,
        border_right,
        border_bottom,
        border_left,
        display: display_type,
    };

    let mut layout_elem = Element::new(&element.tag);
    layout_elem.attrs = element.attrs.clone();
    layout_elem.node_id = element.node_id;
    layout_elem.text = if matches!(
        element.tag.as_str(),
        "input" | "button" | "select" | "textarea"
    ) {
        form_text
    } else {
        element.text.clone()
    };
    layout_elem.is_void = element.is_void;

    let mut layout_node = LayoutNode::new(layout_elem, rect, computed_style, desc_text);

    // Build child layout nodes
    for child in &element.children {
        if let Some((child_layout, _)) = build_layout_node(
            child,
            Some(&layout_node.computed_style),
            css_rules,
            layout_node.rect.content_width(),
            font_size,
            styles,
        ) {
            layout_node.add_child(child_layout);
        }
    }

    let style = layout_node.computed_style.clone();
    Some((layout_node, style))
}

/// Perform full layout pass across the tree
pub fn perform_layout(root: &mut LayoutNode, viewport_width: f64) {
    let viewport_width = viewport_width.max(0.0);
    let mut float_ctx = FloatContext::default();
    layout_node_recursive(root, 0.0, 0.0, viewport_width, 16.0, &mut float_ctx, 0);
}

#[derive(Debug, Clone, Default)]
struct FloatContext {
    left_floats: Vec<RectModel>,
    right_floats: Vec<RectModel>,
}

impl FloatContext {
    fn left_offset_at(&self, y: f64, h: f64) -> f64 {
        let bottom = y + h;
        self.left_floats
            .iter()
            .filter(|f| f.y < bottom && f.y + f.outer_height() > y)
            .map(|f| f.x + f.outer_width())
            .fold(0.0, f64::max)
    }

    fn right_offset_at(&self, y: f64, h: f64, container_right: f64) -> f64 {
        let bottom = y + h;
        self.right_floats
            .iter()
            .filter(|f| f.y < bottom && f.y + f.outer_height() > y)
            .map(|f| container_right - f.x)
            .fold(0.0, f64::max)
    }

    fn clear_left_bottom(&self) -> f64 {
        self.left_floats
            .iter()
            .map(|f| f.y + f.outer_height())
            .fold(0.0, f64::max)
    }

    fn clear_right_bottom(&self) -> f64 {
        self.right_floats
            .iter()
            .map(|f| f.y + f.outer_height())
            .fold(0.0, f64::max)
    }

    fn clear_all_bottom(&self) -> f64 {
        self.clear_left_bottom().max(self.clear_right_bottom())
    }
}

fn layout_node_recursive(
    node: &mut LayoutNode,
    current_x: f64,
    current_y: f64,
    parent_width: f64,
    parent_font_size: f64,
    float_ctx: &mut FloatContext,
    depth: usize,
) -> f64 {
    if depth > 128 {
        return 0.0;
    }

    let font_size = effective_font_size(&node.computed_style, &node.element.tag, parent_font_size);
    let is_border_box = node.computed_style.box_sizing.as_deref() == Some("border-box");
    let h_padding_border = node.rect.padding_left
        + node.rect.padding_right
        + node.rect.border_left
        + node.rect.border_right;
    let v_padding_border = node.rect.padding_top
        + node.rect.padding_bottom
        + node.rect.border_top
        + node.rect.border_bottom;

    // Handle auto margins for block-level elements
    if matches!(
        node.rect.display,
        DisplayType::Block | DisplayType::FlowRoot | DisplayType::Table
    ) && !is_out_of_flow(node)
    {
        let is_margin_left_auto = node.computed_style.margin_left == Some(CssUnit::Auto);
        let is_margin_right_auto = node.computed_style.margin_right == Some(CssUnit::Auto);
        if is_margin_left_auto && is_margin_right_auto {
            let available_margin = (parent_width - node.rect.width).max(0.0);
            node.rect.margin_left = available_margin / 2.0;
            node.rect.margin_right = available_margin / 2.0;
        } else if is_margin_left_auto {
            node.rect.margin_left =
                (parent_width - node.rect.width - node.rect.margin_right).max(0.0);
        } else if is_margin_right_auto {
            node.rect.margin_right =
                (parent_width - node.rect.width - node.rect.margin_left).max(0.0);
        }
    }

    // Set position
    node.rect.x = current_x + node.rect.margin_left;
    node.rect.y = current_y + node.rect.margin_top;

    // Width calculation for block-level boxes
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
            if !has_explicit_width(&node.computed_style) {
                let available_width = parent_width
                    .max(node.rect.margin_left + node.rect.margin_right)
                    .max(0.0)
                    - node.rect.margin_left
                    - node.rect.margin_right;
                node.rect.width = available_width.max(0.0);
            }
        }
        DisplayType::Inline | DisplayType::InlineBlock | DisplayType::TableCell => {
            if node.rect.width == 0.0 {
                let text_width = estimate_text_width(&node.element.text_content(), font_size);
                node.rect.width = (text_width + h_padding_border).max(0.0);
            }
        }
        DisplayType::None => {}
    }

    // Clamping min/max width
    if let Some(ref min_w) = node.computed_style.min_width {
        let min_px = min_w.to_pixels(parent_width, 16.0);
        let min_border_box = if is_border_box {
            min_px
        } else {
            min_px + h_padding_border
        };
        node.rect.width = node.rect.width.max(min_border_box);
    }
    if let Some(ref max_w) = node.computed_style.max_width {
        let max_px = max_w.to_pixels(parent_width, 16.0);
        let max_border_box = if is_border_box {
            max_px
        } else {
            max_px + h_padding_border
        };
        node.rect.width = node.rect.width.min(max_border_box);
    }

    // Content coordinates
    let content_x = node.rect.x + node.rect.padding_left + node.rect.border_left;
    let content_y = node.rect.y + node.rect.padding_top + node.rect.border_top;
    let inner_width = node.rect.content_width();

    // Text wrapping with CSS rules
    let text_lines = if node.element.text.is_empty() {
        Vec::new()
    } else {
        wrap_text_with_rules(
            &node.element.text,
            inner_width,
            font_size,
            node.computed_style.white_space.as_deref(),
            node.computed_style.word_break.as_deref(),
        )
    };
    let line_height = node
        .computed_style
        .line_height
        .map(|lh| lh * font_size)
        .unwrap_or(font_size * 1.4);
    let text_height = text_lines.len() as f64 * line_height;

    let all_inline_children = !node.children.is_empty()
        && node.children.iter().all(|c| {
            matches!(
                c.rect.display,
                DisplayType::Inline | DisplayType::InlineBlock
            )
        });
    let own_text_height = if node.children.is_empty() || all_inline_children {
        0.0
    } else {
        text_height
    };

    let establishes_new_bfc = matches!(
        node.rect.display,
        DisplayType::FlowRoot
            | DisplayType::Flex
            | DisplayType::Grid
            | DisplayType::Table
            | DisplayType::TableCell
    ) || node.computed_style.overflow.as_deref() == Some("hidden");

    let mut local_float_ctx = if establishes_new_bfc {
        FloatContext::default()
    } else {
        float_ctx.clone()
    };

    let start_y = content_y + own_text_height;
    let mut line_x = if all_inline_children {
        content_x
            + if node.element.text.is_empty() {
                0.0
            } else {
                estimate_text_width(&node.element.text, font_size)
            }
    } else {
        content_x
    };
    let mut line_y = start_y;
    let default_line = line_height;
    let mut curr_line_height: f64 = if all_inline_children && !node.element.text.is_empty() {
        default_line
    } else {
        0.0
    };

    let gap = node
        .computed_style
        .gap
        .as_ref()
        .map(|unit| unit.to_pixels(inner_width, 16.0))
        .unwrap_or(0.0)
        .clamp(0.0, inner_width.max(0.0));

    if node.rect.display == DisplayType::Flex {
        layout_flex(
            node,
            content_x,
            content_y,
            inner_width,
            font_size,
            gap,
            &mut local_float_ctx,
            depth + 1,
        );
        line_y = content_y + node.rect.content_height();
    } else if node.rect.display == DisplayType::Grid {
        layout_grid(
            node,
            content_x,
            content_y,
            inner_width,
            font_size,
            gap,
            &mut local_float_ctx,
            depth + 1,
        );
        line_y = content_y + node.rect.content_height();
    } else if node.rect.display == DisplayType::Table {
        layout_table(
            node,
            content_x,
            content_y,
            inner_width,
            font_size,
            &mut local_float_ctx,
            depth + 1,
        );
        line_y = content_y + node.rect.content_height();
    } else {
        let mut prev_margin_bottom: f64 = 0.0;
        for child in &mut node.children {
            if is_out_of_flow(child) {
                layout_node_recursive(
                    child,
                    content_x,
                    content_y,
                    inner_width,
                    font_size,
                    &mut local_float_ctx,
                    depth + 1,
                );
                continue;
            }

            // Clearance handling
            if let Some(ref clear) = child.computed_style.clear {
                match clear.as_str() {
                    "left" => line_y = line_y.max(local_float_ctx.clear_left_bottom()),
                    "right" => line_y = line_y.max(local_float_ctx.clear_right_bottom()),
                    "both" => line_y = line_y.max(local_float_ctx.clear_all_bottom()),
                    _ => {}
                }
            }

            let is_inline = matches!(
                child.rect.display,
                DisplayType::Inline | DisplayType::InlineBlock
            );

            if is_inline {
                let left_float_offset = local_float_ctx.left_offset_at(line_y, default_line);
                let right_float_offset =
                    local_float_ctx.right_offset_at(line_y, default_line, content_x + inner_width);
                let effective_left = content_x.max(left_float_offset);
                let effective_width =
                    (inner_width - (effective_left - content_x) - right_float_offset).max(0.0);

                if line_x < effective_left {
                    line_x = effective_left;
                }

                let child_outer =
                    child.rect.width + child.rect.margin_left + child.rect.margin_right;
                if line_x > effective_left
                    && line_x + child_outer > effective_left + effective_width
                {
                    line_y += if curr_line_height > 0.0 {
                        curr_line_height
                    } else {
                        default_line
                    };
                    line_x = effective_left;
                    curr_line_height = 0.0;
                }
                let child_height = layout_node_recursive(
                    child,
                    line_x,
                    line_y,
                    inner_width,
                    font_size,
                    &mut local_float_ctx,
                    depth + 1,
                );
                line_x += child.rect.outer_width();
                curr_line_height = curr_line_height.max(child_height);
                prev_margin_bottom = 0.0;
            } else if child.computed_style.float.as_deref() == Some("left") {
                let float_y = line_y;
                let float_x = content_x + local_float_ctx.left_offset_at(float_y, default_line);
                let child_height = layout_node_recursive(
                    child,
                    float_x,
                    float_y,
                    inner_width,
                    font_size,
                    &mut local_float_ctx,
                    depth + 1,
                );
                local_float_ctx.left_floats.push(child.rect);
                curr_line_height = curr_line_height.max(child_height);
                prev_margin_bottom = 0.0;
            } else if child.computed_style.float.as_deref() == Some("right") {
                let float_y = line_y;
                let right_offset =
                    local_float_ctx.right_offset_at(float_y, default_line, content_x + inner_width);
                let float_x = (content_x + inner_width - child.rect.outer_width() - right_offset)
                    .max(content_x);
                let child_height = layout_node_recursive(
                    child,
                    float_x,
                    float_y,
                    inner_width,
                    font_size,
                    &mut local_float_ctx,
                    depth + 1,
                );
                local_float_ctx.right_floats.push(child.rect);
                curr_line_height = curr_line_height.max(child_height);
                prev_margin_bottom = 0.0;
            } else {
                // Block-level child
                if line_x > content_x {
                    line_y += if curr_line_height > 0.0 {
                        curr_line_height
                    } else {
                        default_line
                    };
                    line_x = content_x;
                    curr_line_height = 0.0;
                }

                // Vertical margin collapsing
                let collapsed_margin = prev_margin_bottom.max(child.rect.margin_top);
                let target_child_y = line_y - prev_margin_bottom + collapsed_margin;
                let pass_y = target_child_y - child.rect.margin_top;

                let child_height = layout_node_recursive(
                    child,
                    content_x,
                    pass_y,
                    inner_width,
                    font_size,
                    &mut local_float_ctx,
                    depth + 1,
                );
                line_y = child.rect.y + child_height + child.rect.margin_bottom;
                prev_margin_bottom = child.rect.margin_bottom;
            }
        }

        if line_x > content_x {
            line_y += if curr_line_height > 0.0 {
                curr_line_height
            } else {
                default_line
            };
        }
    }

    if establishes_new_bfc {
        // Enclose all internal floats
        line_y = line_y.max(local_float_ctx.clear_all_bottom());
    } else {
        *float_ctx = local_float_ctx;
    }

    let content_height = if node.children.is_empty() {
        text_height
    } else {
        (line_y - content_y).max(own_text_height)
    };

    // Height resolution
    if let Some(ref h) = node.computed_style.height {
        let h_px = h.to_pixels(node.rect.width, 16.0).max(0.0);
        let border_box_h = if is_border_box {
            h_px
        } else {
            h_px + v_padding_border
        };
        node.rect.height = border_box_h;
        return finish_layout_node(node, current_x, current_y, parent_width, node.rect.height);
    }

    if matches!(
        node.element.tag.as_str(),
        "input" | "button" | "select" | "textarea"
    ) {
        node.rect.height = text_height.max(font_size * 1.4) + v_padding_border;
        return finish_layout_node(node, current_x, current_y, parent_width, node.rect.height);
    }

    let is_img = node.element.tag == "img";
    if is_img {
        if let Some(h) = node
            .element
            .get_attr("height")
            .and_then(|v| v.parse::<f64>().ok())
        {
            node.rect.height = h + node.rect.padding_top + node.rect.padding_bottom;
            return finish_layout_node(node, current_x, current_y, parent_width, node.rect.height);
        }
        if node.rect.width > 0.0 {
            let aspect_ratio = node
                .element
                .get_attr("width")
                .and_then(|w| w.parse::<f64>().ok())
                .map(|w| {
                    if let Some(h) = node
                        .element
                        .get_attr("height")
                        .and_then(|h| h.parse::<f64>().ok())
                    {
                        h / w
                    } else {
                        0.75
                    }
                })
                .unwrap_or(0.75);
            node.rect.height =
                (node.rect.width * aspect_ratio) + node.rect.padding_top + node.rect.padding_bottom;
            return finish_layout_node(node, current_x, current_y, parent_width, node.rect.height);
        }
    }

    let mut final_height = (content_height + v_padding_border).max(font_size * 1.4);

    // Clamping min/max height
    if let Some(ref min_h) = node.computed_style.min_height {
        let min_px = min_h.to_pixels(parent_width, 16.0);
        let min_box = if is_border_box {
            min_px
        } else {
            min_px + v_padding_border
        };
        final_height = final_height.max(min_box);
    }
    if let Some(ref max_h) = node.computed_style.max_height {
        let max_px = max_h.to_pixels(parent_width, 16.0);
        let max_box = if is_border_box {
            max_px
        } else {
            max_px + v_padding_border
        };
        final_height = final_height.min(max_box);
    }

    node.rect.height = final_height;
    finish_layout_node(node, current_x, current_y, parent_width, node.rect.height)
}

#[allow(clippy::too_many_arguments)]
fn layout_flex(
    node: &mut LayoutNode,
    content_x: f64,
    content_y: f64,
    inner_width: f64,
    font_size: f64,
    gap: f64,
    float_ctx: &mut FloatContext,
    depth: usize,
) {
    node.children
        .sort_by_key(|child| child.computed_style.order);
    let is_column = node.computed_style.flex_direction.as_deref() == Some("column")
        || node.computed_style.flex_direction.as_deref() == Some("column-reverse");
    let is_reverse = node.computed_style.flex_direction.as_deref() == Some("row-reverse")
        || node.computed_style.flex_direction.as_deref() == Some("column-reverse");

    if is_reverse {
        node.children.reverse();
    }

    if is_column {
        let mut curr_y = content_y;
        for child in &mut node.children {
            if is_out_of_flow(child) {
                layout_node_recursive(
                    child,
                    content_x,
                    content_y,
                    inner_width,
                    font_size,
                    float_ctx,
                    depth,
                );
                continue;
            }
            let child_height = layout_node_recursive(
                child,
                content_x,
                curr_y,
                inner_width,
                font_size,
                float_ctx,
                depth,
            );
            curr_y += child_height + gap;
        }
    } else {
        let wrap = node.computed_style.flex_wrap.as_deref() == Some("wrap")
            || node.computed_style.flex_wrap.as_deref() == Some("wrap-reverse");
        let flex_indices: Vec<usize> = node
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| !is_out_of_flow(child))
            .map(|(index, _)| index)
            .collect();
        let in_flow_count = flex_indices.len().max(1) as f64;
        let auto_width = ((inner_width - gap * (in_flow_count - 1.0)) / in_flow_count).max(0.0);

        let bases: Vec<f64> = flex_indices
            .iter()
            .map(|index| {
                let child = &node.children[*index];
                if has_explicit_width(&child.computed_style) {
                    child.rect.width.max(0.0)
                } else {
                    child
                        .computed_style
                        .flex_basis
                        .as_ref()
                        .map(|basis| basis.to_pixels(inner_width, 16.0))
                        .unwrap_or(0.0)
                        .max(0.0)
                }
            })
            .collect();

        let total_grow: f64 = flex_indices
            .iter()
            .map(|index| node.children[*index].computed_style.flex_grow)
            .sum();
        let total_shrink: f64 = flex_indices
            .iter()
            .enumerate()
            .map(|(pos, index)| bases[pos] * node.children[*index].computed_style.flex_shrink)
            .sum();
        let free_space = inner_width
            - gap * (flex_indices.len().saturating_sub(1) as f64)
            - bases.iter().sum::<f64>();

        let flex_widths: Vec<f64> = flex_indices
            .iter()
            .enumerate()
            .map(|(pos, index)| {
                let child = &node.children[*index];
                if total_grow <= f64::EPSILON && total_shrink <= f64::EPSILON {
                    if has_explicit_width(&child.computed_style) {
                        child.rect.width
                    } else {
                        auto_width
                    }
                } else if free_space >= 0.0 && total_grow > f64::EPSILON {
                    bases[pos] + free_space * child.computed_style.flex_grow / total_grow
                } else if free_space < 0.0 && total_shrink > f64::EPSILON {
                    let weight = bases[pos] * child.computed_style.flex_shrink;
                    (bases[pos] + free_space * weight / total_shrink).max(0.0)
                } else {
                    bases[pos]
                }
            })
            .collect();

        let mut widths_by_index = BTreeMap::new();
        for (pos, index) in flex_indices.iter().enumerate() {
            widths_by_index.insert(*index, flex_widths[pos]);
        }

        let used_width =
            flex_widths.iter().sum::<f64>() + gap * (flex_indices.len().saturating_sub(1) as f64);
        let remaining = (inner_width - used_width).max(0.0);
        let mut main_gap = gap;
        let mut row_x = content_x;

        if !wrap {
            match node.computed_style.justify_content.as_deref() {
                Some("center") => row_x += remaining / 2.0,
                Some("flex-end") | Some("end") => row_x += remaining,
                Some("space-between") if flex_indices.len() > 1 => {
                    main_gap += remaining / (flex_indices.len() - 1) as f64
                }
                Some("space-around") if !flex_indices.is_empty() => {
                    main_gap += remaining / flex_indices.len() as f64;
                    row_x += main_gap / 2.0;
                }
                Some("space-evenly") if !flex_indices.is_empty() => {
                    main_gap += remaining / (flex_indices.len() + 1) as f64;
                    row_x += main_gap;
                }
                _ => {}
            }
        }

        let mut row_y = content_y;
        let mut row_height: f64 = 0.0;
        for (index, child) in node.children.iter_mut().enumerate() {
            if is_out_of_flow(child) {
                layout_node_recursive(
                    child,
                    content_x,
                    content_y,
                    inner_width,
                    font_size,
                    float_ctx,
                    depth,
                );
                continue;
            }
            if !has_explicit_width(&child.computed_style) {
                child.rect.width = widths_by_index.get(&index).copied().unwrap_or(auto_width);
            }
            let outer_w = child.rect.outer_width();
            if wrap && row_x > content_x && row_x + outer_w > content_x + inner_width {
                row_y += row_height + gap;
                row_x = content_x;
                row_height = 0.0;
            }
            let child_height = layout_node_recursive(
                child,
                row_x,
                row_y,
                child.rect.width,
                font_size,
                float_ctx,
                depth,
            );
            row_height = row_height.max(child_height);
            row_x += outer_w + main_gap;
        }

        if !wrap {
            match node.computed_style.align_items.as_deref() {
                Some("center") => {
                    for child in &mut node.children {
                        if !is_out_of_flow(child) {
                            let delta = (row_height - child.rect.outer_height()).max(0.0);
                            child.rect.y += delta / 2.0;
                        }
                    }
                }
                Some("flex-end") | Some("end") => {
                    for child in &mut node.children {
                        if !is_out_of_flow(child) {
                            let delta = (row_height - child.rect.outer_height()).max(0.0);
                            child.rect.y += delta;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_grid(
    node: &mut LayoutNode,
    content_x: f64,
    content_y: f64,
    inner_width: f64,
    font_size: f64,
    gap: f64,
    float_ctx: &mut FloatContext,
    depth: usize,
) {
    let columns = grid_column_count(
        node.computed_style.grid_template_columns.as_deref(),
        node.children.len(),
    );
    let column_width =
        ((inner_width - gap * columns.saturating_sub(1) as f64) / columns.max(1) as f64).max(0.0);

    let mut line_y = content_y;
    let mut index = 0;
    while index < node.children.len() {
        let row_end = (index + columns).min(node.children.len());
        let mut row_height: f64 = 0.0;
        for (col, child) in node.children[index..row_end].iter_mut().enumerate() {
            if !has_explicit_width(&child.computed_style) {
                child.rect.width = column_width;
            }
            let x = content_x + col as f64 * (column_width + gap);
            let child_height =
                layout_node_recursive(child, x, line_y, column_width, font_size, float_ctx, depth);
            row_height = row_height.max(child_height);
        }
        line_y += row_height;
        index = row_end;
        if index < node.children.len() {
            line_y += gap;
        }
    }
}

fn layout_table(
    node: &mut LayoutNode,
    content_x: f64,
    content_y: f64,
    inner_width: f64,
    font_size: f64,
    float_ctx: &mut FloatContext,
    depth: usize,
) {
    // Collect all rows and their cells across the table hierarchy
    let mut rows: Vec<&mut LayoutNode> = Vec::new();
    for child in &mut node.children {
        if child.rect.display == DisplayType::TableRow || child.element.tag == "tr" {
            rows.push(child);
        } else if matches!(
            child.rect.display,
            DisplayType::TableRowGroup
                | DisplayType::TableHeaderGroup
                | DisplayType::TableFooterGroup
        ) || matches!(child.element.tag.as_str(), "tbody" | "thead" | "tfoot")
        {
            for row_child in &mut child.children {
                if row_child.rect.display == DisplayType::TableRow || row_child.element.tag == "tr"
                {
                    rows.push(row_child);
                }
            }
        }
    }

    if rows.is_empty() {
        return;
    }

    // Determine total column count
    let max_cols = rows
        .iter()
        .map(|r| {
            r.children
                .iter()
                .map(|c| {
                    c.element
                        .get_attr("colspan")
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(1)
                })
                .sum::<usize>()
        })
        .max()
        .unwrap_or(1)
        .max(1);

    let col_width = (inner_width / max_cols as f64).max(0.0);

    let mut curr_y = content_y;
    for row in rows {
        row.rect.x = content_x;
        row.rect.y = curr_y;
        row.rect.width = inner_width;

        let mut curr_x = content_x;
        let mut max_cell_h: f64 = 0.0;

        for cell in &mut row.children {
            let span = cell
                .element
                .get_attr("colspan")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let cell_w = col_width * span as f64;
            cell.rect.width = cell_w;

            let cell_h = layout_node_recursive(
                cell,
                curr_x,
                curr_y,
                cell_w,
                font_size,
                float_ctx,
                depth + 1,
            );
            max_cell_h = max_cell_h.max(cell_h);
            curr_x += cell_w;
        }

        row.rect.height = max_cell_h.max(font_size * 1.4);
        curr_y += row.rect.height;
    }
}

fn is_out_of_flow(node: &LayoutNode) -> bool {
    matches!(
        node.computed_style.position,
        PositionMode::Absolute | PositionMode::Fixed
    )
}

fn resolve_offset(value: Option<&CssUnit>, parent_size: f64) -> Option<f64> {
    value.map(|value| {
        value
            .to_pixels(parent_size, 16.0)
            .clamp(-1_000_000.0, 1_000_000.0)
    })
}

/// Apply relative/absolute/fixed offsets and the bounded axis-aligned transform
fn finish_layout_node(
    node: &mut LayoutNode,
    containing_x: f64,
    containing_y: f64,
    containing_width: f64,
    flow_height: f64,
) -> f64 {
    let original_x = node.rect.x;
    let original_y = node.rect.y;
    let left = resolve_offset(node.computed_style.left.as_ref(), containing_width);
    let right = resolve_offset(node.computed_style.right.as_ref(), containing_width);
    let top = resolve_offset(node.computed_style.top.as_ref(), containing_width);
    let bottom = resolve_offset(node.computed_style.bottom.as_ref(), containing_width);
    match node.computed_style.position {
        PositionMode::Static => {}
        PositionMode::Relative => {
            node.rect.x += left.unwrap_or(0.0) - right.unwrap_or(0.0);
            node.rect.y += top.unwrap_or(0.0) - bottom.unwrap_or(0.0);
        }
        PositionMode::Absolute | PositionMode::Fixed => {
            let (base_x, base_y) = if node.computed_style.position == PositionMode::Fixed {
                (0.0, 0.0)
            } else {
                (containing_x, containing_y)
            };
            node.rect.x = if let Some(left) = left {
                base_x + left + node.rect.margin_left
            } else if let Some(right) = right {
                base_x + containing_width - right - node.rect.width - node.rect.margin_right
            } else {
                base_x + node.rect.margin_left
            };
            node.rect.y = if let Some(top) = top {
                base_y + top + node.rect.margin_top
            } else if let Some(bottom) = bottom {
                base_y - bottom - node.rect.height - node.rect.margin_bottom
            } else {
                base_y + node.rect.margin_top
            };
        }
    }
    let position_dx = node.rect.x - original_x;
    let position_dy = node.rect.y - original_y;
    if position_dx != 0.0 || position_dy != 0.0 {
        for child in &mut node.children {
            translate_layout_subtree(child, position_dx, position_dy);
        }
    }
    let transform = node.computed_style.transform;
    node.rect.x += transform.translate_x;
    node.rect.y += transform.translate_y;
    node.rect.width = (node.rect.width * transform.scale_x).max(0.0);
    node.rect.height = (node.rect.height * transform.scale_y).max(0.0);
    if transform != Transform2D::default() {
        for child in &mut node.children {
            transform_layout_subtree(
                child,
                original_x + position_dx,
                original_y + position_dy,
                transform,
            );
        }
    }
    flow_height
}

fn translate_layout_subtree(node: &mut LayoutNode, dx: f64, dy: f64) {
    node.rect.x += dx;
    node.rect.y += dy;
    for child in &mut node.children {
        translate_layout_subtree(child, dx, dy);
    }
}

fn transform_layout_subtree(
    node: &mut LayoutNode,
    origin_x: f64,
    origin_y: f64,
    transform: Transform2D,
) {
    node.rect.x = origin_x + (node.rect.x - origin_x) * transform.scale_x + transform.translate_x;
    node.rect.y = origin_y + (node.rect.y - origin_y) * transform.scale_y + transform.translate_y;
    node.rect.width = (node.rect.width * transform.scale_x).max(0.0);
    node.rect.height = (node.rect.height * transform.scale_y).max(0.0);
    for child in &mut node.children {
        transform_layout_subtree(child, origin_x, origin_y, transform);
    }
}

fn grid_column_count(template: Option<&str>, child_count: usize) -> usize {
    let Some(template) = template.map(str::trim).filter(|value| !value.is_empty()) else {
        return child_count.clamp(1, 2);
    };
    if let Some(repeat) = template.strip_prefix("repeat(") {
        if let Some((count, _)) = repeat.split_once(',') {
            if let Ok(count) = count.trim().parse::<usize>() {
                return count.clamp(1, 12);
            }
        }
    }
    template
        .split_ascii_whitespace()
        .filter(|track| !track.is_empty())
        .count()
        .clamp(1, 12)
}

/// Recursively count the total number of layout nodes in the tree.
pub(crate) fn count_layout_nodes(node: &LayoutNode) -> usize {
    1 + node.children.iter().map(count_layout_nodes).sum::<usize>()
}

/// Find the first layout node with the given tag
#[cfg(test)]
fn find_node<'a>(node: &'a LayoutNode, tag: &str) -> Option<&'a LayoutNode> {
    if node.element.tag == tag {
        return Some(node);
    }
    node.children.iter().find_map(|child| find_node(child, tag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css_parser;

    #[test]
    fn test_parse_display_block() {
        let style = ComputedStyle {
            display: Some("block".to_string()),
            ..Default::default()
        };
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

        let (root, _) = build_layout_node(&dom, None, &rules, 800.0, 16.0, None).unwrap();
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].element.tag, "p");
    }

    #[test]
    fn test_inline_children_share_a_line() {
        let html = "<div><a href='#'>Home</a><a href='#'>About</a></div>";
        let dom = crate::parser::parse_html(html);
        let rules: Vec<css_parser::CssRule> = vec![];
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);
        let links: Vec<&LayoutNode> = layout
            .children
            .iter()
            .filter(|c| c.element.tag == "a")
            .collect();
        assert_eq!(links.len(), 2);
        assert_eq!(
            links[0].rect.y, links[1].rect.y,
            "inline links must share a line"
        );
        assert!(
            links[1].rect.x > links[0].rect.x,
            "second link must be to the right"
        );
    }

    #[test]
    fn test_block_children_stack_vertically() {
        let html = "<div><p>One</p><p>Two</p></div>";
        let dom = crate::parser::parse_html(html);
        let rules: Vec<css_parser::CssRule> = vec![];
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);
        let ps: Vec<&LayoutNode> = layout
            .children
            .iter()
            .filter(|c| c.element.tag == "p")
            .collect();
        assert_eq!(ps.len(), 2);
        assert!(
            ps[1].rect.y > ps[0].rect.y,
            "block children must stack vertically"
        );
    }

    #[test]
    fn test_explicit_block_width_survives_layout() {
        let html = "<div class='box'><p>x</p></div>";
        let dom = crate::parser::parse_html(html);
        let css = ".box { width: 300px; }";
        let rules = css_parser::parse_css(css);
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);
        assert_eq!(layout.element.tag, "div");
        assert!((layout.rect.width - 300.0).abs() < 0.001);
    }

    #[test]
    fn test_auto_margin_centering() {
        let html = "<div class='box'><p>x</p></div>";
        let dom = crate::parser::parse_html(html);
        let css = ".box { width: 400px; margin: 0 auto; }";
        let rules = css_parser::parse_css(css);
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);
        assert_eq!(layout.rect.margin_left, 200.0);
        assert_eq!(layout.rect.margin_right, 200.0);
        assert_eq!(layout.rect.x, 200.0);
    }

    #[test]
    fn test_box_sizing_border_box() {
        let html = "<div class='box'><p>x</p></div>";
        let dom = crate::parser::parse_html(html);
        let css = ".box { box-sizing: border-box; width: 300px; padding: 20px; border: 5px solid black; }";
        let rules = css_parser::parse_css(css);
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);
        assert!((layout.rect.width - 300.0).abs() < 0.001);
        assert!((layout.rect.content_width() - 250.0).abs() < 0.001);
    }

    #[test]
    fn test_wrap_text_counts_chars_not_bytes() {
        let text = "xin chào thế giới!";
        let lines = wrap_text(text, 192.0, 16.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], text);
    }

    #[test]
    fn test_inline_width_includes_descendant_text() {
        let html = "<div><a href='#'>Home <b>Page</b></a></div>";
        let dom = crate::parser::parse_html(html);
        let rules: Vec<css_parser::CssRule> = vec![];
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);
        let link = &layout.children[0];
        let with_child_width = link.rect.width;
        assert!(with_child_width > estimate_text_width("Home", 16.0));
    }

    #[test]
    fn test_wrap_text_breaks_long_cjk_word() {
        let long = "这是一个非常长的中文字符串没有空格需要被拆分成多行显示";
        let lines = wrap_text(long, 100.0, 16.0);
        assert!(lines.len() >= 2, "CJK run must wrap: {:?}", lines);
    }

    #[test]
    fn test_wrap_text_breaks_long_latin_word() {
        let long = "supercalifragilisticexpialidocious";
        let lines = wrap_text(long, 100.0, 16.0);
        assert!(lines.len() >= 2, "long word must wrap");
    }

    #[test]
    fn test_inline_text_and_children_share_one_line() {
        let html = "<p>Hello <a href='#'>link</a></p>";
        let dom = crate::parser::parse_html(html);
        let rules: Vec<css_parser::CssRule> = vec![];
        let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
        perform_layout(&mut layout, 800.0);

        let p = find_node(&layout, "p").expect("p in tree");
        let link = find_node(&layout, "a").expect("a in tree");
        assert!(
            (link.rect.y - p.rect.y).abs() < 1.0,
            "inline child must share the text line (y offset {})",
            link.rect.y - p.rect.y
        );
    }

    #[test]
    fn test_layout_node_element_has_no_children_clone() {
        let html = "<div><span>a</span><span>b</span></div>";
        let dom = crate::parser::parse_html(html);
        let rules: Vec<css_parser::CssRule> = vec![];
        let layout = create_layout_tree(&dom, &rules, 800).unwrap();
        let div = find_node(&layout, "div").expect("div in tree");
        assert_eq!(div.element.children.len(), 0, "element copy is childless");
        assert_eq!(div.children.len(), 2, "layout tree keeps the children");
    }

    #[test]
    fn test_flex_row_distributes_children() {
        let dom = crate::parser::parse_html("<div class='flex'><p>one</p><p>two</p></div>");
        let rules = css_parser::parse_css(".flex{display:flex;gap:10px}");
        let layout = create_layout_tree(&dom, &rules, 600).unwrap();
        let flex = find_node(&layout, "div").unwrap();
        assert_eq!(flex.rect.display, DisplayType::Flex);
        assert_eq!(flex.children.len(), 2);
        assert!((flex.children[0].rect.y - flex.children[1].rect.y).abs() < 1.0);
        assert!(flex.children[1].rect.x > flex.children[0].rect.x);
    }

    #[test]
    fn test_grid_places_items_in_rows() {
        let dom =
            crate::parser::parse_html("<div class='grid'><p>one</p><p>two</p><p>three</p></div>");
        let rules = css_parser::parse_css(
            ".grid{display:grid;grid-template-columns:repeat(2,1fr);gap:8px}",
        );
        let layout = create_layout_tree(&dom, &rules, 600).unwrap();
        let grid = find_node(&layout, "div").unwrap();
        assert_eq!(grid.rect.display, DisplayType::Grid);
        assert!(grid.children[1].rect.x > grid.children[0].rect.x);
        assert!(grid.children[2].rect.y > grid.children[0].rect.y);
    }

    #[test]
    fn test_table_layout_distributes_cells() {
        let dom = crate::parser::parse_html(
            "<table><tr><td>A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>",
        );
        let layout = create_layout_tree(&dom, &[], 600).unwrap();
        let table = find_node(&layout, "table").unwrap();
        assert_eq!(table.rect.display, DisplayType::Table);
        assert_eq!(table.children.len(), 2);
        assert_eq!(table.children[0].children.len(), 2);
        assert!(table.children[0].children[1].rect.x > table.children[0].children[0].rect.x);
        assert!(table.children[1].rect.y > table.children[0].rect.y);
    }

    #[test]
    fn test_form_control_has_accessible_visible_box() {
        let dom = crate::parser::parse_html(
            "<form><input placeholder='Search'><button>Go</button></form>",
        );
        let layout = create_layout_tree(&dom, &[], 600).unwrap();
        let input = find_node(&layout, "input").unwrap();
        assert_eq!(input.element.text, "Search");
        assert!(input.rect.width >= 160.0);
        assert!(input.rect.height > 20.0);
    }
}

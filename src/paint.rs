// Display list painter: layout tree to render commands, stacking contexts and visible metrics

use crate::layout::{effective_font_size, wrap_text_with_rules, DisplayType, LayoutNode};

/// Plain RGBA color (0.0 - 1.0), independent from any GUI framework
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// Multiply a color's alpha by `factor` (for CSS opacity composition).
pub fn mul_alpha(color: Rgba, factor: f32) -> Rgba {
    Rgba {
        r: color.r,
        g: color.g,
        b: color.b,
        a: (color.a * factor).clamp(0.0, 1.0),
    }
}

/// Apply the clip box to a rectangle. `clip = None` means "no clipping" and
/// the box passes through unchanged; `None` is returned only when the box
/// is fully outside an ACTIVE clip (item dropped).
pub fn clipped_rect(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    clip: Option<(f32, f32, f32, f32)>,
) -> Option<(f32, f32, f32, f32)> {
    let (cx, cy, cw, ch) = match clip {
        Some(c) => c,
        None => return Some((x, y, w, h)),
    };
    let nx = x.max(cx);
    let ny = y.max(cy);
    let nx2 = (x + w).min(cx + cw);
    let ny2 = (y + h).min(cy + ch);
    if nx2 <= nx || ny2 <= ny {
        None
    } else {
        Some((nx, ny, nx2 - nx, ny2 - ny))
    }
}

impl Rgba {
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const BLACK: Rgba = Rgba::rgb(0.067, 0.067, 0.067); // #111111 default text
    pub const WHITE: Rgba = Rgba::rgb(1.0, 1.0, 1.0);
    /// Standard link blue (#1A0DAB like Google results)
    pub const LINK_BLUE: Rgba = Rgba::rgb(0.102, 0.051, 0.671);
}

/// A single paint command in document coordinates
#[derive(Debug, Clone)]
pub enum DisplayItem {
    /// Filled rectangle (backgrounds)
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgba,
    },
    /// Rectangle outline (borders), drawn as 4 thin filled rects by the GUI
    Border {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        width: f32,
        color: Rgba,
    },
    /// One line of text
    TextRun {
        x: f32,
        y: f32,
        size: f32,
        color: Rgba,
        content: String,
        bold: bool,
        italic: bool,
        underline: bool,
        monospace: bool,
    },
    /// A loaded image (rendered as placeholder text if not cached yet)
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        url: String,
        alt: String,
        cached: bool,
    },
    /// A pending image that hasn't been loaded yet (lazy loading).
    PendingImage {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        url: String,
        alt: String,
    },
    /// A Canvas 2D or SVG vector shape drawn by the page
    VectorShape(VectorShape),
}

/// A page-drawn vector shape with fill and/or stroke.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorShape {
    pub kind: VectorShapeKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub fill: Option<Rgba>,
    pub stroke: Option<Rgba>,
    pub stroke_width: f32,
}

/// Geometric kind of a page-drawn vector shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VectorShapeKind {
    Rect,
    Ellipse,
    Line,
}

/// A clickable link region (hit-tested on mouse click)
#[derive(Debug, Clone)]
pub struct LinkRegion {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub href: String,
}

/// The full result of painting a page
#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
    pub links: Vec<LinkRegion>,
    /// Document size in CSS pixels
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisibleMetrics {
    pub painted_area_px: f32,
    pub visible_text_characters: usize,
    pub meaningful_item_count: usize,
    pub has_major_blank_region: bool,
    pub completeness_score: f32,
}

impl DisplayList {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Find the topmost link under a document-space point
    pub fn link_at(&self, x: f32, y: f32) -> Option<&str> {
        self.links
            .iter()
            .rev()
            .find(|l| x >= l.x && x <= l.x + l.w && y >= l.y && y <= l.y + l.h)
            .map(|l| l.href.as_str())
    }

    /// Viewport Clipping Optimization
    pub fn filter_viewport(&self, viewport_y: f32, viewport_height: f32) -> DisplayList {
        let margin = 50.0;
        let min_y = viewport_y - margin;
        let max_y = viewport_y + viewport_height + margin;

        let items = self
            .items
            .iter()
            .filter(|item| match item {
                DisplayItem::Rect { y, h, .. } => (y + h) >= min_y && *y <= max_y,
                DisplayItem::Border { y, h, .. } => (y + h) >= min_y && *y <= max_y,
                DisplayItem::Image { y, h, .. } => (y + h) >= min_y && *y <= max_y,
                DisplayItem::PendingImage { y, h, .. } => (y + h) >= min_y && *y <= max_y,
                DisplayItem::TextRun { y, size, .. } => (y + size) >= min_y && *y <= max_y,
                DisplayItem::VectorShape(shape) => (shape.y + shape.h) >= min_y && shape.y <= max_y,
            })
            .cloned()
            .collect();

        DisplayList {
            items,
            links: self.links.clone(),
            width: self.width,
            height: self.height,
        }
    }
}

/// Calculate visible content metrics for a painted display list
pub fn calculate_visible_metrics(
    list: &DisplayList,
    viewport_w: f32,
    viewport_h: f32,
) -> VisibleMetrics {
    let mut total_painted_area = 0.0_f32;
    let mut text_chars = 0_usize;
    let mut meaningful_items = 0_usize;

    for item in &list.items {
        match item {
            DisplayItem::Rect { w, h, color, .. } => {
                if color.a > 0.05 && *color != Rgba::WHITE {
                    total_painted_area += w * h;
                    meaningful_items += 1;
                }
            }
            DisplayItem::Border { w, h, color, .. } => {
                if color.a > 0.05 {
                    total_painted_area += w * h;
                    meaningful_items += 1;
                }
            }
            DisplayItem::TextRun { content, .. } => {
                let trimmed = content.trim();
                text_chars += trimmed.chars().count();
                if !trimmed.is_empty() {
                    meaningful_items += 1;
                }
            }
            DisplayItem::Image { w, h, .. } | DisplayItem::PendingImage { w, h, .. } => {
                total_painted_area += w * h;
                meaningful_items += 1;
            }
            DisplayItem::VectorShape(shape) => {
                total_painted_area += shape.w * shape.h;
                meaningful_items += 1;
            }
        }
    }

    let viewport_area = (viewport_w * viewport_h).max(1.0);
    let coverage_ratio = (total_painted_area / viewport_area).clamp(0.0, 1.0);
    let text_score = (text_chars as f32 / 100.0).clamp(0.0, 1.0);
    let completeness_score = (coverage_ratio * 0.4 + text_score * 0.6).clamp(0.0, 1.0);
    let has_major_blank_region = text_chars == 0 && total_painted_area < 100.0;

    VisibleMetrics {
        painted_area_px: total_painted_area,
        visible_text_characters: text_chars,
        meaningful_item_count: meaningful_items,
        has_major_blank_region,
        completeness_score,
    }
}

/// Inherited paint state passed down the tree
#[derive(Clone, Copy)]
pub struct PaintContext {
    pub color: Rgba,
    pub font_size: f64,
    pub bold: bool,
    pub italic: bool,
    pub link: bool,
    pub monospace: bool,
    pub opacity: f32,
    pub clip: Option<(f32, f32, f32, f32)>,
}

fn has_fixed_size(style: &crate::css_parser::ComputedStyle) -> bool {
    style.height.is_some()
}

fn clips_overflow(style: &crate::css_parser::ComputedStyle) -> bool {
    style
        .overflow
        .as_deref()
        .map(|o| o == "hidden" || o == "clip" || o == "auto" || o == "scroll")
        .unwrap_or(false)
        || style.overflow_x.as_deref() == Some("hidden")
        || style.overflow_y.as_deref() == Some("hidden")
}

pub fn build_display_list(root: &LayoutNode) -> DisplayList {
    build_display_list_with_cache(root, None)
}

pub fn build_display_list_with_cache(
    root: &LayoutNode,
    image_cache: Option<&crate::image_loader::ImageCache>,
) -> DisplayList {
    let mut list = DisplayList::default();

    let doc_width = root.rect.outer_width().max(1.0) as f32;
    let doc_height = (root.rect.y + root.rect.outer_height()).max(1.0) as f32;
    list.width = doc_width;
    list.height = doc_height + 24.0;

    // Page background: body/html background-color, else white
    let page_bg = page_background(root).unwrap_or(Rgba::WHITE);
    list.items.push(DisplayItem::Rect {
        x: 0.0,
        y: 0.0,
        w: doc_width,
        h: list.height,
        color: page_bg,
    });

    let ctx = PaintContext {
        color: Rgba::BLACK,
        font_size: 16.0,
        bold: false,
        italic: false,
        link: false,
        monospace: false,
        opacity: 1.0,
        clip: None,
    };

    paint_stacking_context(root, ctx, &mut list, image_cache);
    list
}

fn page_background(root: &LayoutNode) -> Option<Rgba> {
    if let Some(bg) = root
        .computed_style
        .background_color
        .as_deref()
        .and_then(parse_css_color)
    {
        return Some(bg);
    }
    for child in &root.children {
        if child.element.tag == "body" || child.element.tag == "html" {
            if let Some(bg) = child
                .computed_style
                .background_color
                .as_deref()
                .and_then(parse_css_color)
            {
                return Some(bg);
            }
            if child.element.tag == "html" {
                if let Some(bg) = page_background(child) {
                    return Some(bg);
                }
            }
        }
    }
    None
}

fn creates_stacking_context(node: &LayoutNode) -> bool {
    let style = &node.computed_style;
    (style.position != crate::css_parser::PositionMode::Static && style.z_index != 0)
        || style.opacity.map(|o| o < 1.0).unwrap_or(false)
        || style.transform != crate::css_parser::Transform2D::default()
}

fn apply_text_transform(text: &str, transform: Option<&str>) -> String {
    match transform {
        Some("uppercase") => text.to_uppercase(),
        Some("lowercase") => text.to_lowercase(),
        Some("capitalize") => {
            let mut result = String::new();
            let mut cap_next = true;
            for c in text.chars() {
                if c.is_whitespace() {
                    cap_next = true;
                    result.push(c);
                } else if cap_next {
                    result.extend(c.to_uppercase());
                    cap_next = false;
                } else {
                    result.push(c);
                }
            }
            result
        }
        _ => text.to_string(),
    }
}

/// Paint a node and its descendants according to CSS 2.2 / 3 Stacking Context Order
pub fn paint_stacking_context(
    node: &LayoutNode,
    parent: PaintContext,
    list: &mut DisplayList,
    image_cache: Option<&crate::image_loader::ImageCache>,
) {
    if node.rect.display == DisplayType::None {
        return;
    }

    let is_visible = node.computed_style.visibility.as_deref() != Some("hidden")
        && node.computed_style.visibility.as_deref() != Some("collapse");

    let tag = node.element.tag.as_str();
    let font_size = effective_font_size(&node.computed_style, tag, parent.font_size);

    let own_opacity = node
        .computed_style
        .opacity
        .map(|o| o.clamp(0.0, 1.0) as f32)
        .unwrap_or(1.0);
    let opacity = parent.opacity * own_opacity;

    let clip = if clips_overflow(&node.computed_style) && has_fixed_size(&node.computed_style) {
        let own = (
            node.rect.x as f32,
            node.rect.y as f32,
            node.rect.outer_width() as f32,
            node.rect.outer_height() as f32,
        );
        match parent.clip {
            Some(p) => Some((
                own.0.max(p.0),
                own.1.max(p.1),
                (own.0 + own.2).min(p.0 + p.2) - own.0.max(p.0),
                (own.1 + own.3).min(p.1 + p.3) - own.1.max(p.1),
            )),
            None => Some(own),
        }
    } else {
        parent.clip
    };

    if clipped_rect(
        node.rect.x as f32,
        node.rect.y as f32,
        node.rect.outer_width() as f32,
        node.rect.outer_height() as f32,
        clip,
    )
    .is_none()
    {
        return;
    }

    let is_link = parent.link || (tag == "a" && node.element.get_attr("href").is_some());
    let bold = parent.bold
        || matches!(
            tag,
            "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th"
        )
        || node
            .computed_style
            .font_weight
            .map(|w| w >= 600)
            .unwrap_or(false);
    let italic = parent.italic
        || matches!(tag, "i" | "em" | "cite" | "var")
        || node.computed_style.font_style.as_deref() == Some("italic");
    let monospace = parent.monospace || matches!(tag, "code" | "pre" | "kbd" | "samp" | "tt");

    let color = if let Some(c) = node
        .computed_style
        .color
        .as_deref()
        .and_then(parse_css_color)
    {
        c
    } else if is_link && !parent.link {
        Rgba::LINK_BLUE
    } else {
        parent.color
    };

    let x = node.rect.x as f32;
    let y = node.rect.y as f32;
    let w = node.rect.width as f32;
    let h = node.rect.height as f32;

    // Phase 1: Background & Borders of current context
    if is_visible && tag != "html" && tag != "body" {
        // Box shadow
        if node.computed_style.box_shadow.is_some() {
            if let Some((cx, cy, cw, ch)) = clipped_rect(x + 2.0, y + 2.0, w, h, clip) {
                list.items.push(DisplayItem::Rect {
                    x: cx,
                    y: cy,
                    w: cw,
                    h: ch,
                    color: mul_alpha(Rgba::rgb(0.0, 0.0, 0.0), opacity * 0.15),
                });
            }
        }

        // Background
        if let Some(bg) = node
            .computed_style
            .background_color
            .as_deref()
            .and_then(parse_css_color)
        {
            if let Some((cx, cy, cw, ch)) = clipped_rect(x, y, w, h, clip) {
                list.items.push(DisplayItem::Rect {
                    x: cx,
                    y: cy,
                    w: cw,
                    h: ch,
                    color: mul_alpha(bg, opacity),
                });
            }
        }

        // Border
        let border_style_ok = node
            .computed_style
            .border_style
            .as_deref()
            .map(|s| s != "none" && s != "hidden")
            .unwrap_or(false);
        let border_width = node
            .computed_style
            .border_width
            .as_ref()
            .map(|bw| bw.to_pixels(node.rect.width, 16.0) as f32)
            .unwrap_or(if border_style_ok { 1.0 } else { 0.0 });
        if border_width > 0.0 && (border_style_ok || node.computed_style.border_color.is_some()) {
            let border_color = node
                .computed_style
                .border_color
                .as_deref()
                .and_then(parse_css_color)
                .unwrap_or(color);
            if let Some((cx, cy, cw, ch)) = clipped_rect(x, y, w, h, clip) {
                list.items.push(DisplayItem::Border {
                    x: cx,
                    y: cy,
                    w: cw,
                    h: ch,
                    width: border_width,
                    color: mul_alpha(border_color, opacity),
                });
            }
        }

        if tag == "hr" {
            if let Some((cx, cy, cw, ch)) = clipped_rect(x, y + h / 2.0, w, 1.0, clip) {
                list.items.push(DisplayItem::Rect {
                    x: cx,
                    y: cy,
                    w: cw,
                    h: ch,
                    color: mul_alpha(Rgba::rgb(0.8, 0.8, 0.8), opacity),
                });
            }
        }
    }

    // Direct text of this element
    let raw_text = node.element.text.trim();
    if is_visible && !raw_text.is_empty() && tag != "title" && tag != "img" {
        let transformed_text =
            apply_text_transform(raw_text, node.computed_style.text_transform.as_deref());
        let content_x = (node.rect.x + node.rect.padding_left + node.rect.border_left) as f32;
        let content_y = (node.rect.y + node.rect.padding_top + node.rect.border_top) as f32;
        let inner_width = node.rect.content_width();
        let line_h =
            crate::css_parser::line_height_px(node.computed_style.line_height, font_size) as f32;

        let display_text = if node.rect.display == DisplayType::ListItem {
            format!("•  {}", transformed_text)
        } else {
            transformed_text
        };

        let has_underline = is_link
            || node
                .computed_style
                .text_decoration
                .as_deref()
                .map(|td| td.contains("underline"))
                .unwrap_or(false);

        let has_line_through = node
            .computed_style
            .text_decoration
            .as_deref()
            .map(|td| td.contains("line-through"))
            .unwrap_or(false);

        let lines = wrap_text_with_rules(
            &display_text,
            inner_width,
            font_size,
            node.computed_style.white_space.as_deref(),
            node.computed_style.word_break.as_deref(),
        );

        for (i, line) in lines.iter().enumerate() {
            let line_y = content_y + i as f32 * line_h;
            if clipped_rect(content_x, line_y, inner_width as f32, line_h, clip).is_none() {
                continue;
            }
            list.items.push(DisplayItem::TextRun {
                x: content_x,
                y: line_y,
                size: font_size as f32,
                color: mul_alpha(color, opacity),
                content: line.clone(),
                bold,
                italic,
                underline: has_underline,
                monospace,
            });

            if has_line_through {
                let strike_y = line_y + (font_size as f32 * 0.6);
                let strike_w =
                    (line.chars().count() as f32 * font_size as f32 * 0.58).min(inner_width as f32);
                if let Some((cx, cy, cw, ch)) =
                    clipped_rect(content_x, strike_y, strike_w, 1.5, clip)
                {
                    list.items.push(DisplayItem::Rect {
                        x: cx,
                        y: cy,
                        w: cw,
                        h: ch,
                        color: mul_alpha(color, opacity),
                    });
                }
            }
        }
    }

    // <img> tag
    if is_visible && tag == "img" {
        let src = node
            .element
            .get_attr("src")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let alt = node
            .element
            .get_attr("alt")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "image".to_string());

        if let Some((cx, cy, cw, ch)) = clipped_rect(x, y, w, h, clip) {
            if src.is_empty() {
                list.items.push(DisplayItem::PendingImage {
                    x: cx,
                    y: cy,
                    w: cw,
                    h: ch,
                    url: String::new(),
                    alt,
                });
            } else if image_cache.is_some_and(|c| c.is_decoded(&src)) {
                list.items.push(DisplayItem::Image {
                    x: cx,
                    y: cy,
                    w: cw,
                    h: ch,
                    url: src,
                    alt,
                    cached: true,
                });
            } else {
                list.items.push(DisplayItem::PendingImage {
                    x: cx,
                    y: cy,
                    w: cw,
                    h: ch,
                    url: src,
                    alt,
                });
            }
        }
    }

    // Link region registration
    if is_visible && tag == "a" {
        if let Some(href) = node.element.get_attr("href") {
            if !href.trim().is_empty() {
                list.links.push(LinkRegion {
                    x,
                    y,
                    w: w.max(node.rect.content_width() as f32),
                    h: h.max((font_size * 1.4) as f32),
                    href: href.trim().to_string(),
                });
            }
        }
    }

    let child_ctx = PaintContext {
        color,
        font_size,
        bold,
        italic,
        link: is_link,
        monospace,
        opacity,
        clip,
    };

    // Stacking Context Child Categorization:
    let mut neg_z_stacking: Vec<&LayoutNode> = Vec::new();
    let mut normal_flow_blocks: Vec<&LayoutNode> = Vec::new();
    let mut non_positioned_floats: Vec<&LayoutNode> = Vec::new();
    let mut normal_flow_inlines: Vec<&LayoutNode> = Vec::new();
    let mut auto_z_positioned: Vec<&LayoutNode> = Vec::new();
    let mut pos_z_stacking: Vec<&LayoutNode> = Vec::new();

    for child in &node.children {
        if child.rect.display == DisplayType::None {
            continue;
        }
        if creates_stacking_context(child) {
            if child.computed_style.z_index < 0 {
                neg_z_stacking.push(child);
            } else {
                pos_z_stacking.push(child);
            }
        } else if child.computed_style.position != crate::css_parser::PositionMode::Static {
            auto_z_positioned.push(child);
        } else if child.computed_style.float.is_some() {
            non_positioned_floats.push(child);
        } else if matches!(
            child.rect.display,
            DisplayType::Inline | DisplayType::InlineBlock
        ) {
            normal_flow_inlines.push(child);
        } else {
            normal_flow_blocks.push(child);
        }
    }

    neg_z_stacking.sort_by_key(|c| c.computed_style.z_index);
    pos_z_stacking.sort_by_key(|c| c.computed_style.z_index);

    // Phase 2: Negative z-index stacking context children
    for child in neg_z_stacking {
        paint_stacking_context(child, child_ctx, list, image_cache);
    }

    // Phase 3: In-flow non-inline non-positioned block descendants
    for child in normal_flow_blocks {
        paint_stacking_context(child, child_ctx, list, image_cache);
    }

    // Phase 4: Non-positioned floating descendants
    for child in non_positioned_floats {
        paint_stacking_context(child, child_ctx, list, image_cache);
    }

    // Phase 5: In-flow inline-level descendants
    for child in normal_flow_inlines {
        paint_stacking_context(child, child_ctx, list, image_cache);
    }

    // Phase 6: Positioned descendants with z-index: 0 / auto
    for child in auto_z_positioned {
        paint_stacking_context(child, child_ctx, list, image_cache);
    }

    // Phase 7: Positive z-index stacking context children
    for child in pos_z_stacking {
        paint_stacking_context(child, child_ctx, list, image_cache);
    }
}

/// Parse a CSS color value: named colors, #rgb, #rrggbb, rgb()/rgba()
pub fn parse_css_color(value: &str) -> Option<Rgba> {
    let v = value.trim().to_lowercase();

    // Hex forms
    if let Some(hex) = v.strip_prefix('#') {
        if !hex.is_ascii() {
            return None;
        }
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(Rgba::rgb(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                ))
            }
            6 | 8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = if hex.len() == 8 {
                    u8::from_str_radix(&hex[6..8], 16).ok()? as f32 / 255.0
                } else {
                    1.0
                };
                Some(Rgba {
                    r: r as f32 / 255.0,
                    g: g as f32 / 255.0,
                    b: b as f32 / 255.0,
                    a,
                })
            }
            _ => None,
        };
    }

    // rgb() / rgba() functional forms
    if v.starts_with("rgb(") || v.starts_with("rgba(") {
        let inner = v.split('(').nth(1)?.trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() >= 3 {
            let r = parts[0].parse::<f32>().ok()? / 255.0;
            let g = parts[1].parse::<f32>().ok()? / 255.0;
            let b = parts[2].parse::<f32>().ok()? / 255.0;
            let a = if parts.len() >= 4 {
                parts[3].parse::<f32>().unwrap_or(1.0)
            } else {
                1.0
            };
            let clamp = |x: f32| -> f32 {
                if x.is_finite() {
                    x.clamp(0.0, 1.0)
                } else {
                    1.0
                }
            };
            return Some(Rgba {
                r: clamp(r),
                g: clamp(g),
                b: clamp(b),
                a: clamp(a),
            });
        }
        return None;
    }

    // Named colors
    let (r, g, b) = match v.as_str() {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "orange" => (255, 165, 0),
        "purple" => (128, 0, 128),
        "pink" => (255, 192, 203),
        "gray" | "grey" => (128, 128, 128),
        "lightgray" | "lightgrey" => (211, 211, 211),
        "darkgray" | "darkgrey" => (169, 169, 169),
        "silver" => (192, 192, 192),
        "brown" => (165, 42, 42),
        "cyan" | "aqua" => (0, 255, 255),
        "magenta" | "fuchsia" => (255, 0, 255),
        "lime" => (0, 255, 0),
        "maroon" => (128, 0, 0),
        "navy" => (0, 0, 128),
        "olive" => (128, 128, 0),
        "teal" => (0, 128, 128),
        "gold" => (255, 215, 0),
        "coral" => (255, 127, 80),
        "salmon" => (250, 128, 114),
        "tomato" => (255, 99, 71),
        "crimson" => (220, 20, 60),
        "indigo" => (75, 0, 130),
        "violet" => (238, 130, 238),
        "khaki" => (240, 230, 140),
        "beige" => (245, 245, 220),
        "ivory" => (255, 255, 240),
        "lavender" => (230, 230, 250),
        "skyblue" => (135, 206, 235),
        "lightblue" => (173, 216, 230),
        "steelblue" => (70, 130, 180),
        "royalblue" => (65, 105, 225),
        "dodgerblue" => (30, 144, 255),
        "midnightblue" => (25, 25, 112),
        "forestgreen" => (34, 139, 34),
        "seagreen" => (46, 139, 87),
        "darkgreen" => (0, 100, 0),
        "lightgreen" => (144, 238, 144),
        "darkred" => (139, 0, 0),
        "darkblue" => (0, 0, 139),
        "darkorange" => (255, 140, 0),
        "whitesmoke" => (245, 245, 245),
        "ghostwhite" => (248, 248, 255),
        "snow" => (255, 250, 250),
        "transparent" => {
            return Some(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            })
        }
        _ => return None,
    };
    Some(Rgba::rgb(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css_parser::parse_css;
    use crate::layout::create_layout_tree;
    use crate::parser::parse_html;

    fn build(html: &str, css: &str) -> DisplayList {
        let dom = parse_html(html);
        let mut rules = parse_css(css);
        for style_elem in dom.find_all_tags("style") {
            let css_text = style_elem.text.trim();
            if !css_text.is_empty() {
                rules.append(&mut parse_css(css_text));
            }
        }
        let root = create_layout_tree(&dom, &rules, 800).expect("layout");
        build_display_list(&root)
    }

    #[test]
    fn test_parse_hex_colors() {
        assert_eq!(parse_css_color("#fff"), Some(Rgba::rgb(1.0, 1.0, 1.0)));
        assert_eq!(parse_css_color("#000000"), Some(Rgba::rgb(0.0, 0.0, 0.0)));
        let red = parse_css_color("#ff0000").unwrap();
        assert!((red.r - 1.0).abs() < 0.001 && red.g == 0.0 && red.b == 0.0);
    }

    #[test]
    fn test_parse_named_colors() {
        assert!(parse_css_color("red").is_some());
        assert!(parse_css_color("SteelBlue").is_some());
        assert!(parse_css_color("notacolor").is_none());
    }

    #[test]
    fn test_parse_color_unicode_never_panics() {
        assert_eq!(parse_css_color("#日"), None);
        assert_eq!(parse_css_color("#éa"), None);
        assert_eq!(parse_css_color("#日本語"), None);
        assert_eq!(parse_css_color("rgb(私, 0, 0)"), None);
        assert_eq!(parse_css_color("###"), None);
    }

    #[test]
    fn test_parse_rgb_functional() {
        let c = parse_css_color("rgb(255, 128, 0)").unwrap();
        assert!((c.r - 1.0).abs() < 0.001);
        assert!((c.g - 128.0 / 255.0).abs() < 0.001);
        assert_eq!(c.b, 0.0);

        let t = parse_css_color("rgba(0, 0, 0, 0.5)").unwrap();
        assert!((t.a - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_display_list_has_text() {
        let list = build("<html><body><h1>Hello Pixels</h1></body></html>", "");
        let has_text = list.items.iter().any(|i| {
            matches!(
                i, DisplayItem::TextRun { content, .. } if content.contains("Hello Pixels")
            )
        });
        assert!(has_text);
        assert!(list.width > 0.0 && list.height > 0.0);
    }

    #[test]
    fn test_heading_is_bold_and_larger() {
        let list = build("<html><body><h1>Big</h1><p>Small</p></body></html>", "");
        let h1 = list
            .items
            .iter()
            .find_map(|i| match i {
                DisplayItem::TextRun {
                    content,
                    size,
                    bold,
                    ..
                } if content == "Big" => Some((*size, *bold)),
                _ => None,
            })
            .expect("h1 text run");
        let p = list
            .items
            .iter()
            .find_map(|i| match i {
                DisplayItem::TextRun { content, size, .. } if content == "Small" => Some(*size),
                _ => None,
            })
            .expect("p text run");
        assert!(h1.1, "h1 must be bold");
        assert!(h1.0 > p, "h1 ({}) must be larger than p ({})", h1.0, p);
    }

    #[test]
    fn test_link_region_and_color() {
        let list = build(
            "<html><body><p><a href=\"https://example.com\">Click me</a></p></body></html>",
            "",
        );
        assert_eq!(list.links.len(), 1);
        assert_eq!(list.links[0].href, "https://example.com");

        let underlined = list.items.iter().any(|i| matches!(
            i, DisplayItem::TextRun { content, underline: true, .. } if content.contains("Click me")
        ));
        assert!(underlined);

        let l = &list.links[0];
        assert_eq!(
            list.link_at(l.x + 1.0, l.y + 1.0),
            Some("https://example.com")
        );
        assert_eq!(list.link_at(-100.0, -100.0), None);
    }

    #[test]
    fn test_css_colors_applied() {
        let list = build(
            "<html><head><style>p { color: red; background-color: #eeeeee; }</style></head>\
             <body><p>Colored</p></body></html>",
            "",
        );
        let red_text = list.items.iter().any(|i| {
            matches!(
                i, DisplayItem::TextRun { content, color, .. }
                    if content == "Colored" && (color.r - 1.0).abs() < 0.01 && color.g < 0.01
            )
        });
        assert!(red_text, "paragraph text should be red");

        let bg = list.items.iter().any(|i| {
            matches!(
                i, DisplayItem::Rect { color, .. } if (color.r - 0.933).abs() < 0.01
            )
        });
        assert!(bg, "paragraph background should be painted");
    }

    #[test]
    fn test_page_background_default_white() {
        let list = build("<html><body><p>x</p></body></html>", "");
        match &list.items[0] {
            DisplayItem::Rect { color, .. } => assert_eq!(*color, Rgba::WHITE),
            other => panic!("first item should be page background, got {:?}", other),
        }
    }

    #[test]
    fn test_list_item_bullet() {
        let list = build("<html><body><ul><li>Item one</li></ul></body></html>", "");
        let bullet = list.items.iter().any(|i| {
            matches!(
                i, DisplayItem::TextRun { content, .. } if content.starts_with("•")
            )
        });
        assert!(bullet);
    }

    #[test]
    fn test_rgb_channels_are_clamped() {
        let over = parse_css_color("rgb(300, 0, 0)").unwrap();
        assert_eq!(over.r, 1.0, "channel above 255 must clamp to 1.0");
        let negative = parse_css_color("rgb(-50, 255, 128)").unwrap();
        assert_eq!(negative.r, 0.0, "negative channel must clamp to 0.0");
        let huge = parse_css_color("rgb(1e400, 0, 0)");
        if let Some(c) = huge {
            assert!(c.r.is_finite(), "inf channel must be clamped");
            assert!(c.r <= 1.0);
        }
    }

    #[test]
    fn test_opacity_multiplies_alpha() {
        let list = build(
            "<html><body><p style=\"color: rgb(255, 0, 0);\">hello world padded</p></body></html>",
            "p { opacity: 0.5; }",
        );
        let run = list
            .items
            .iter()
            .find(|i| matches!(i, DisplayItem::TextRun { .. }))
            .expect("text run");
        if let DisplayItem::TextRun { color, .. } = run {
            assert!(
                (color.a - 0.5).abs() < 0.01,
                "opacity 0.5 must halve the text alpha, got {}",
                color.a
            );
        }
    }

    #[test]
    fn test_overflow_hidden_clips_children() {
        let list = build(
            r#"<html><body>
                <div style="height: 40px; overflow: hidden;">
                    <p style="margin-top: 400px;">way below</p>
                </div>
            </body></html>"#,
            "",
        );
        let clipped_runs = list
            .items
            .iter()
            .filter(|i| matches!(i, DisplayItem::TextRun { content, .. } if content.contains("way below")))
            .count();
        assert_eq!(
            clipped_runs, 0,
            "content below overflow:hidden must be clipped"
        );
    }

    #[test]
    fn test_stacking_context_z_index_order() {
        let list = build(
            r#"<html><body>
                <div style="position: relative; z-index: 10; background-color: red; width: 100px; height: 100px;">Top</div>
                <div style="position: relative; z-index: -5; background-color: blue; width: 100px; height: 100px;">Bottom</div>
            </body></html>"#,
            "",
        );
        let mut red_idx = None;
        let mut blue_idx = None;
        for (idx, item) in list.items.iter().enumerate() {
            if let DisplayItem::Rect { color, .. } = item {
                if (color.r - 1.0).abs() < 0.01 && color.g < 0.01 {
                    red_idx = Some(idx);
                }
                if color.b > 0.9 && color.r < 0.01 {
                    blue_idx = Some(idx);
                }
            }
        }
        assert!(
            blue_idx.unwrap() < red_idx.unwrap(),
            "negative z-index must paint before positive z-index"
        );
    }

    #[test]
    fn test_visible_metrics_calculation() {
        let list = build("<html><body><h1>Hello World</h1><p>Testing visual metrics calculation.</p></body></html>", "");
        let metrics = calculate_visible_metrics(&list, 800.0, 600.0);
        assert!(metrics.visible_text_characters > 20);
        assert!(metrics.meaningful_item_count >= 2);
        assert!(!metrics.has_major_blank_region);
        assert!(metrics.completeness_score > 0.1);
    }
}

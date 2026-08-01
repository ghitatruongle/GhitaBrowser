// src/paint.rs - Pixel painting: layout tree -> display list (v0.6.0)
// Turns the layout tree into a flat list of paint commands (rects, borders,
// text runs, link regions) that the GUI draws onto a real graphics canvas.
// This module is UI-framework agnostic: colors are plain RGBA floats.
#![allow(dead_code)]

use crate::layout::{effective_font_size, wrap_text, DisplayType, LayoutNode};

/// Plain RGBA color (0.0 - 1.0), independent from any GUI framework
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
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
}

/// Inherited paint state passed down the tree
#[derive(Clone, Copy)]
struct PaintContext {
    color: Rgba,
    font_size: f64,
    bold: bool,
    italic: bool,
    link: bool,
    monospace: bool,
}

/// Build the display list for a laid-out page.
/// Pages paint on a white background with black text — exactly like Chrome,
/// where the OS/browser theme never darkens actual web content.
pub fn build_display_list(root: &LayoutNode) -> DisplayList {
    let mut list = DisplayList::default();

    let doc_width = root.rect.outer_width().max(1.0) as f32;
    let doc_height = (root.rect.y + root.rect.outer_height()).max(1.0) as f32;
    list.width = doc_width;
    list.height = doc_height + 24.0; // bottom breathing room

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
    };
    paint_node(root, ctx, &mut list);

    list
}

/// Look for a background color on the root/html/body elements
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

fn paint_node(node: &LayoutNode, parent: PaintContext, list: &mut DisplayList) {
    if node.rect.display == DisplayType::None {
        return;
    }

    let tag = node.element.tag.as_str();
    let font_size = effective_font_size(&node.computed_style, tag, parent.font_size);

    // Resolve inherited text properties (UA defaults + CSS)
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

    // Text color: explicit CSS wins; otherwise inherit, with a blue default for the first <a>
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

    // Background (skip the page-level fill already painted)
    if tag != "html" && tag != "body" {
        if let Some(bg) = node
            .computed_style
            .background_color
            .as_deref()
            .and_then(parse_css_color)
        {
            list.items.push(DisplayItem::Rect {
                x,
                y,
                w,
                h,
                color: bg,
            });
        }
    }

    // Border (if styled)
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
        list.items.push(DisplayItem::Border {
            x,
            y,
            w,
            h,
            width: border_width,
            color: border_color,
        });
    }

    // Horizontal rule renders as a thin divider
    if tag == "hr" {
        list.items.push(DisplayItem::Rect {
            x,
            y: y + h / 2.0,
            w,
            h: 1.0,
            color: Rgba::rgb(0.8, 0.8, 0.8),
        });
    }

    // Text content of this element (direct text only; children paint themselves)
    let text = node.element.text.trim();
    if !text.is_empty() && tag != "title" {
        let content_x = (node.rect.x + node.rect.padding_left + node.rect.border_left) as f32;
        let content_y = (node.rect.y + node.rect.padding_top + node.rect.border_top) as f32;
        let inner_width = node.rect.content_width();
        let line_height = (font_size * 1.4) as f32;

        // List bullets, Chrome-style
        let display_text = if node.rect.display == DisplayType::ListItem {
            format!("•  {}", text)
        } else if tag == "img" {
            format!(
                "🖼 [{}]",
                node.element
                    .get_attr("alt")
                    .map(|s| s.as_str())
                    .unwrap_or("image")
            )
        } else {
            text.to_string()
        };

        let lines = wrap_text(&display_text, inner_width, font_size);
        for (i, line) in lines.iter().enumerate() {
            let line_y = content_y + i as f32 * line_height;
            list.items.push(DisplayItem::TextRun {
                x: content_x,
                y: line_y,
                size: font_size as f32,
                color,
                content: line.clone(),
                bold,
                italic,
                underline: is_link,
                monospace,
            });
        }
    }

    // Register the clickable region for links
    if tag == "a" {
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

    let ctx = PaintContext {
        color,
        font_size,
        bold,
        italic,
        link: is_link,
        monospace,
    };
    for child in &node.children {
        paint_node(child, ctx, list);
    }
}

/// Parse a CSS color value: named colors, #rgb, #rrggbb, rgb()/rgba()
pub fn parse_css_color(value: &str) -> Option<Rgba> {
    let v = value.trim().to_lowercase();

    // Hex forms
    if let Some(hex) = v.strip_prefix('#') {
        // Web CSS is untrusted input: guard against multi-byte chars so byte slicing
        // below can never panic on a char boundary (e.g. "#日").
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
            return Some(Rgba {
                r,
                g,
                b,
                a: a.clamp(0.0, 1.0),
            });
        }
        return None;
    }

    // Named colors (the common web set)
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
        // Mirror the real pipeline: also parse <style> tags embedded in the page
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
        // Untrusted CSS with multi-byte chars must return None, not panic on a byte slice
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

        // The link text must be underlined
        let underlined = list.items.iter().any(|i| matches!(
            i, DisplayItem::TextRun { content, underline: true, .. } if content.contains("Click me")
        ));
        assert!(underlined);

        // Hit-testing inside the region works
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
}

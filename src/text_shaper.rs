//! Pure-Rust shaping, system-font fallback and glyph rasterization for pages.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight, Wrap,
};

const MAX_TEXT_PIXELS: usize = 16 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct TextShapeStyle {
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub monospace: bool,
    pub color: [u8; 4],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterizedText {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub glyph_count: usize,
    pub font_count: usize,
    pub missing_glyphs: usize,
    pub contains_rtl_run: bool,
}

struct TextRasterizer {
    fonts: FontSystem,
    cache: SwashCache,
}

fn text_attrs(monospace: bool, bold: bool) -> Attrs<'static> {
    Attrs::new()
        .family(if monospace {
            Family::Monospace
        } else {
            Family::SansSerif
        })
        .weight(if bold { Weight::BOLD } else { Weight::NORMAL })
        .style(Style::Normal)
}

impl TextRasterizer {
    fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }

    /// Measure the advance width of a single un-wrapped line of text in
    /// device pixels using the real shaping engine. No rasterization is
    /// performed, so this is cheap enough to call from layout for inline box
    /// sizing. Returns an error when the request violates the shaping budget
    /// (callers should then fall back to the heuristic estimator).
    fn measure(
        &mut self,
        content: &str,
        font_size: f32,
        bold: bool,
        monospace: bool,
    ) -> Result<f64, String> {
        validate_request(content, 1, 1, font_size)?;
        if content.is_empty() {
            return Ok(0.0);
        }
        let metrics = Metrics::new(font_size, (font_size * 1.3).max(1.0));
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        let mut borrowed = buffer.borrow_with(&mut self.fonts);
        // Effectively unbounded single line: no wrapping, no raster surface.
        borrowed.set_size(1_000_000.0, 1_000.0);
        borrowed.set_wrap(Wrap::None);
        borrowed.set_text(content, text_attrs(monospace, bold), Shaping::Advanced);
        borrowed.shape_until_scroll();
        // line_w is the shaped line width in logical pixels (cosmic-text
        // 0.10 LayoutRun field), covering kerning and complex shaping for
        // the whole line; taking the max over runs is correct for BiDi.
        let mut width = 0.0_f32;
        for run in borrowed.layout_runs() {
            width = width.max(run.line_w);
        }
        Ok(f64::from(width))
    }

    fn rasterize(
        &mut self,
        content: &str,
        width: u32,
        height: u32,
        style: TextShapeStyle,
    ) -> Result<RasterizedText, String> {
        validate_request(content, width, height, style.size)?;
        let metrics = Metrics::new(style.size, (style.size * 1.3).max(1.0));
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        let mut borrowed = buffer.borrow_with(&mut self.fonts);
        borrowed.set_size(width as f32, height as f32);
        borrowed.set_wrap(Wrap::Word);
        borrowed.set_text(
            content,
            text_attrs(style.monospace, style.bold),
            Shaping::Advanced,
        );
        borrowed.shape_until_scroll();

        let mut fonts = BTreeSet::new();
        let mut glyph_count = 0_usize;
        let mut missing_glyphs = 0_usize;
        let mut contains_rtl_run = false;
        for run in borrowed.layout_runs() {
            contains_rtl_run |= run.rtl;
            for glyph in run.glyphs {
                contains_rtl_run |= glyph.level.is_rtl();
                glyph_count = glyph_count.saturating_add(1);
                fonts.insert(glyph.font_id);
                missing_glyphs += usize::from(glyph.glyph_id == 0);
            }
        }

        let mut rgba = vec![0_u8; width as usize * height as usize * 4];
        let foreground = Color::rgba(
            style.color[0],
            style.color[1],
            style.color[2],
            style.color[3],
        );
        borrowed.draw(
            &mut self.cache,
            foreground,
            |x, y, pixel_width, pixel_height, color| {
                let source = color.as_rgba();
                for offset_y in 0..pixel_height as i32 {
                    for offset_x in 0..pixel_width as i32 {
                        let target_x = x + offset_x;
                        let target_y = y + offset_y;
                        if target_x < 0
                            || target_y < 0
                            || target_x >= width as i32
                            || target_y >= height as i32
                        {
                            continue;
                        }
                        let offset = (target_y as usize * width as usize + target_x as usize) * 4;
                        blend_pixel(&mut rgba[offset..offset + 4], source);
                    }
                }
            },
        );
        Ok(RasterizedText {
            width,
            height,
            rgba,
            glyph_count,
            font_count: fonts.len(),
            missing_glyphs,
            contains_rtl_run,
        })
    }
}

pub fn rasterize_text(
    content: &str,
    width: u32,
    height: u32,
    style: TextShapeStyle,
) -> Result<RasterizedText, String> {
    static RASTERIZER: OnceLock<Mutex<TextRasterizer>> = OnceLock::new();
    let rasterizer = RASTERIZER.get_or_init(|| Mutex::new(TextRasterizer::new()));
    rasterizer
        .lock()
        .map_err(|_| "Text rasterizer lock was poisoned".to_string())?
        .rasterize(content, width, height, style)
}

/// Real glyph-advance width of a single line of text, measured by the
/// cosmic-text shaping engine shared with [`rasterize_text`]. Used by layout
/// for inline box sizing so boxes reflect true proportional advances instead
/// of the heuristic estimator. Returns an error (rather than panicking or
/// blocking) when the request violates the shaping budget; callers should
/// fall back to the heuristic in that case.
pub fn measure_text_width(
    content: &str,
    font_size: f32,
    bold: bool,
    monospace: bool,
) -> Result<f64, String> {
    static RASTERIZER: OnceLock<Mutex<TextRasterizer>> = OnceLock::new();
    let rasterizer = RASTERIZER.get_or_init(|| Mutex::new(TextRasterizer::new()));
    let mut guard = rasterizer
        .lock()
        .map_err(|_| "Text rasterizer lock was poisoned".to_string())?;
    guard.measure(content, font_size, bold, monospace)
}

fn validate_request(content: &str, width: u32, height: u32, size: f32) -> Result<(), String> {
    if content.len() > MAX_TEXT_BYTES {
        return Err("Text shaping input exceeds the 1 MB budget".to_string());
    }
    if width == 0 || height == 0 || width as usize * height as usize > MAX_TEXT_PIXELS {
        return Err("Text raster surface exceeds the pixel budget".to_string());
    }
    if !size.is_finite() || !(1.0..=512.0).contains(&size) {
        return Err("Text size is outside the shaping budget".to_string());
    }
    Ok(())
}

fn blend_pixel(target: &mut [u8], source: [u8; 4]) {
    let alpha = u16::from(source[3]);
    let inverse = 255_u16.saturating_sub(alpha);
    for channel in 0..3 {
        target[channel] = ((u16::from(source[channel]) * alpha
            + u16::from(target[channel]) * inverse)
            / 255) as u8;
    }
    target[3] = (alpha + u16::from(target[3]) * inverse / 255).min(255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advanced_shaping_rasterizes_multilingual_and_rtl_text() {
        let output = rasterize_text(
            "Tiếng Việt — العربية — 日本語",
            640,
            80,
            TextShapeStyle {
                size: 28.0,
                bold: false,
                italic: false,
                monospace: false,
                color: [255, 255, 255, 255],
            },
        )
        .unwrap();
        assert!(output.glyph_count > 10);
        assert!(output.font_count >= 1);
        assert_eq!(output.missing_glyphs, 0);
        assert!(output.contains_rtl_run);
        assert!(output.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn measure_text_width_returns_real_advances() {
        let width = measure_text_width("Hello", 16.0, false, false).unwrap();
        assert!(width > 0.0, "shaped line must have positive width");
        // Real metrics: "Hello" at 16px is ~35-45px, far from the 50.0+ the
        // old per-char heuristic (0.55-0.85 em) would give.
        assert!((30.0..=55.0).contains(&width), "measured {width}px");
    }

    #[test]
    fn measure_text_width_scales_with_font_size() {
        let small = measure_text_width("mmmmmm", 12.0, false, false).unwrap();
        let large = measure_text_width("mmmmmm", 24.0, false, false).unwrap();
        assert!(
            (large - small * 2.0).abs() < 2.0,
            "width must scale ~linearly with size: {small} vs {large}"
        );
    }

    #[test]
    fn measure_text_width_empty_and_budget_limits() {
        assert_eq!(measure_text_width("", 16.0, false, false).unwrap(), 0.0);
        let too_big = "x".repeat(1024 * 1024 + 1);
        assert!(measure_text_width(&too_big, 16.0, false, false).is_err());
    }

    #[test]
    fn shaping_rejects_unbounded_surfaces() {
        assert!(rasterize_text(
            "bounded",
            8_192,
            8_192,
            TextShapeStyle {
                size: 14.0,
                bold: false,
                italic: false,
                monospace: false,
                color: [0, 0, 0, 255],
            },
        )
        .is_err());
    }
}

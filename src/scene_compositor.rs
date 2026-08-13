//! Toolkit-independent retained scene and bounded CPU compositor.
//!
//! The scene is the stable contract between layout/paint, decoded media and a
//! future GPU adapter. CPU rendering is always available for device recovery.

use std::collections::BTreeMap;

use crate::paint::{DisplayItem, DisplayList, Rgba};

const MAX_SCENE_PRIMITIVES: usize = 200_000;
const MAX_DAMAGE_RECTS: usize = 4_096;
const MAX_SURFACE_DIMENSION: u32 = 8_192;
const MAX_SURFACE_PIXELS: usize = 64 * 1024 * 1024;
const MAX_SCENE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl SceneRect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Result<Self, String> {
        if !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width < 0.0
            || height < 0.0
        {
            return Err("Invalid scene rectangle".to_string());
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }

    fn union(self, other: Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        Self {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScenePrimitive {
    SolidRect {
        id: u64,
        bounds: SceneRect,
        color: Rgba,
    },
    Border {
        id: u64,
        bounds: SceneRect,
        width: f32,
        color: Rgba,
    },
    Text {
        id: u64,
        bounds: SceneRect,
        content: String,
        size: f32,
        color: Rgba,
        bold: bool,
        italic: bool,
        monospace: bool,
    },
    ImagePlaceholder {
        id: u64,
        bounds: SceneRect,
        ready: bool,
    },
    VideoSurface {
        id: u64,
        bounds: SceneRect,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

impl ScenePrimitive {
    pub fn id(&self) -> u64 {
        match self {
            Self::SolidRect { id, .. }
            | Self::Border { id, .. }
            | Self::Text { id, .. }
            | Self::ImagePlaceholder { id, .. }
            | Self::VideoSurface { id, .. } => *id,
        }
    }

    pub fn bounds(&self) -> SceneRect {
        match self {
            Self::SolidRect { bounds, .. }
            | Self::Border { bounds, .. }
            | Self::Text { bounds, .. }
            | Self::ImagePlaceholder { bounds, .. }
            | Self::VideoSurface { bounds, .. } => *bounds,
        }
    }

    fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Text { content, .. } => content.len(),
                Self::VideoSurface { rgba, .. } => rgba.len(),
                _ => 0,
            }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RetainedScene {
    pub revision: u64,
    pub width: f32,
    pub height: f32,
    primitives: Vec<ScenePrimitive>,
    damage: Vec<SceneRect>,
    estimated_bytes: usize,
}

impl RetainedScene {
    pub fn from_display_list(revision: u64, list: &DisplayList) -> Result<Self, String> {
        if list.items.len() > MAX_SCENE_PRIMITIVES {
            return Err("Scene primitive budget exceeded".to_string());
        }
        let mut primitives = Vec::with_capacity(list.items.len());
        for (index, item) in list.items.iter().enumerate() {
            let id = index as u64 + 1;
            let primitive = match item {
                DisplayItem::Rect { x, y, w, h, color } => ScenePrimitive::SolidRect {
                    id,
                    bounds: SceneRect::new(*x, *y, *w, *h)?,
                    color: *color,
                },
                DisplayItem::Border {
                    x,
                    y,
                    w,
                    h,
                    width,
                    color,
                } => ScenePrimitive::Border {
                    id,
                    bounds: SceneRect::new(*x, *y, *w, *h)?,
                    width: *width,
                    color: *color,
                },
                DisplayItem::TextRun {
                    x,
                    y,
                    size,
                    color,
                    content,
                    bold,
                    italic,
                    monospace,
                    ..
                } => ScenePrimitive::Text {
                    id,
                    bounds: SceneRect::new(
                        *x,
                        *y,
                        (content.chars().count() as f32 * *size * 0.65).max(1.0),
                        (*size * 1.3).max(1.0),
                    )?,
                    content: content.clone(),
                    size: *size,
                    color: *color,
                    bold: *bold,
                    italic: *italic,
                    monospace: *monospace,
                },
                DisplayItem::VectorShape(shape) => ScenePrimitive::SolidRect {
                    id,
                    bounds: SceneRect::new(shape.x, shape.y, shape.w, shape.h)?,
                    color: shape.fill.unwrap_or(crate::paint::Rgba::rgb(0.0, 0.0, 0.0)),
                },
                DisplayItem::Image { x, y, w, h, .. } => ScenePrimitive::ImagePlaceholder {
                    id,
                    bounds: SceneRect::new(*x, *y, *w, *h)?,
                    ready: true,
                },
                DisplayItem::PendingImage { x, y, w, h, .. } => ScenePrimitive::ImagePlaceholder {
                    id,
                    bounds: SceneRect::new(*x, *y, *w, *h)?,
                    ready: false,
                },
            };
            primitives.push(primitive);
        }
        let estimated_bytes = primitives.iter().map(ScenePrimitive::estimated_bytes).sum();
        if estimated_bytes > MAX_SCENE_BYTES {
            return Err("Scene byte budget exceeded".to_string());
        }
        Ok(Self {
            revision,
            width: list.width,
            height: list.height,
            primitives,
            damage: vec![SceneRect::new(0.0, 0.0, list.width, list.height)?],
            estimated_bytes,
        })
    }

    pub fn primitives(&self) -> &[ScenePrimitive] {
        &self.primitives
    }

    pub fn damage(&self) -> &[SceneRect] {
        &self.damage
    }

    pub fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    pub fn upsert_video_surface(
        &mut self,
        id: u64,
        bounds: SceneRect,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), String> {
        validate_surface(width, height, &rgba)?;
        let replacement = ScenePrimitive::VideoSurface {
            id,
            bounds,
            width,
            height,
            rgba,
        };
        let previous_bytes = self
            .primitives
            .iter()
            .find(|primitive| primitive.id() == id)
            .map(ScenePrimitive::estimated_bytes)
            .unwrap_or_default();
        let projected = self
            .estimated_bytes
            .saturating_sub(previous_bytes)
            .saturating_add(replacement.estimated_bytes());
        if projected > MAX_SCENE_BYTES {
            return Err("Scene byte budget exceeded".to_string());
        }
        if let Some(index) = self
            .primitives
            .iter()
            .position(|primitive| primitive.id() == id)
        {
            let previous = self.primitives[index].bounds();
            self.estimated_bytes = projected;
            self.primitives[index] = replacement;
            self.push_damage(previous.union(bounds));
        } else {
            if self.primitives.len() >= MAX_SCENE_PRIMITIVES {
                return Err("Scene primitive budget exceeded".to_string());
            }
            self.estimated_bytes = projected;
            self.primitives.push(replacement);
            self.push_damage(bounds);
        }
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn update_from(&mut self, next: RetainedScene) {
        let previous = self
            .primitives
            .iter()
            .map(|primitive| (primitive.id(), primitive))
            .collect::<BTreeMap<_, _>>();
        let current = next
            .primitives
            .iter()
            .map(|primitive| (primitive.id(), primitive))
            .collect::<BTreeMap<_, _>>();
        let mut damage = Vec::new();
        for (id, primitive) in &previous {
            match current.get(id) {
                Some(next_primitive) if *primitive == *next_primitive => {}
                Some(next_primitive) => {
                    damage.push(primitive.bounds().union(next_primitive.bounds()))
                }
                None => damage.push(primitive.bounds()),
            }
        }
        for (id, primitive) in &current {
            if !previous.contains_key(id) {
                damage.push(primitive.bounds());
            }
        }
        *self = next;
        self.damage.clear();
        for rect in damage {
            self.push_damage(rect);
        }
    }

    pub fn clear_damage(&mut self) {
        self.damage.clear();
    }

    fn push_damage(&mut self, rect: SceneRect) {
        if self.damage.iter().any(|existing| existing.intersects(rect)) {
            if let Some(existing) = self
                .damage
                .iter_mut()
                .find(|existing| existing.intersects(rect))
            {
                *existing = existing.union(rect);
            }
        } else if self.damage.len() < MAX_DAMAGE_RECTS {
            self.damage.push(rect);
        } else if let Some(first) = self.damage.first_mut() {
            *first = first.union(rect);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub scene_revision: u64,
    pub used_cpu_fallback: bool,
}

#[derive(Debug, Default)]
pub struct CpuCompositor;

impl CpuCompositor {
    pub fn render(
        &self,
        scene: &RetainedScene,
        width: u32,
        height: u32,
    ) -> Result<CompositedFrame, String> {
        let pixels = validate_dimensions(width, height)?;
        let mut rgba = vec![0u8; pixels * 4];
        for primitive in &scene.primitives {
            match primitive {
                ScenePrimitive::SolidRect { bounds, color, .. } => {
                    fill_rect(&mut rgba, width, height, *bounds, *color)
                }
                ScenePrimitive::Border {
                    bounds,
                    width: border,
                    color,
                    ..
                } => draw_border(&mut rgba, width, height, *bounds, *border, *color),
                ScenePrimitive::Text {
                    bounds,
                    content,
                    size,
                    color,
                    bold,
                    italic,
                    monospace,
                    ..
                } => {
                    if !draw_shaped_text(
                        &mut rgba, width, height, *bounds, content, *size, *color, *bold, *italic,
                        *monospace,
                    ) {
                        draw_reference_text(&mut rgba, width, height, *bounds, content, *color);
                    }
                }
                ScenePrimitive::ImagePlaceholder { bounds, ready, .. } => fill_rect(
                    &mut rgba,
                    width,
                    height,
                    *bounds,
                    if *ready {
                        Rgba::rgb(0.75, 0.75, 0.75)
                    } else {
                        Rgba::rgb(0.45, 0.45, 0.45)
                    },
                ),
                ScenePrimitive::VideoSurface {
                    bounds,
                    width: source_width,
                    height: source_height,
                    rgba: source,
                    ..
                } => blit_surface(
                    &mut rgba,
                    width,
                    height,
                    *bounds,
                    *source_width,
                    *source_height,
                    source,
                ),
            }
        }
        Ok(CompositedFrame {
            width,
            height,
            rgba,
            scene_revision: scene.revision,
            used_cpu_fallback: false,
        })
    }
}

pub trait OptionalGpuAdapter {
    fn render_gpu(
        &mut self,
        scene: &RetainedScene,
        width: u32,
        height: u32,
    ) -> Result<CompositedFrame, String>;
    fn recover_device(&mut self) -> Result<(), String>;
}

pub struct ResilientCompositor<G: OptionalGpuAdapter> {
    gpu: Option<G>,
    cpu: CpuCompositor,
    device_losses: u32,
}

impl<G: OptionalGpuAdapter> ResilientCompositor<G> {
    pub fn new(gpu: Option<G>) -> Self {
        Self {
            gpu,
            cpu: CpuCompositor,
            device_losses: 0,
        }
    }

    pub fn render(
        &mut self,
        scene: &RetainedScene,
        width: u32,
        height: u32,
    ) -> Result<CompositedFrame, String> {
        if let Some(gpu) = self.gpu.as_mut() {
            match gpu.render_gpu(scene, width, height) {
                Ok(frame) => return Ok(frame),
                Err(_) => {
                    self.device_losses = self.device_losses.saturating_add(1);
                    let _ = gpu.recover_device();
                }
            }
        }
        let mut frame = self.cpu.render(scene, width, height)?;
        frame.used_cpu_fallback = true;
        Ok(frame)
    }

    pub fn device_losses(&self) -> u32 {
        self.device_losses
    }
}

pub fn percentile_95_ms(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut samples = samples.to_vec();
    samples.sort_unstable();
    Some(samples[(samples.len() * 95).div_ceil(100).saturating_sub(1)])
}

fn validate_dimensions(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 || width > MAX_SURFACE_DIMENSION || height > MAX_SURFACE_DIMENSION
    {
        return Err("Compositor surface dimensions exceed the budget".to_string());
    }
    let pixels = width as usize * height as usize;
    if pixels > MAX_SURFACE_PIXELS {
        return Err("Compositor surface pixel budget exceeded".to_string());
    }
    Ok(pixels)
}

fn validate_surface(width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let pixels = validate_dimensions(width, height)?;
    if rgba.len() != pixels.saturating_mul(4) {
        return Err("Video surface RGBA payload has the wrong size".to_string());
    }
    Ok(())
}

fn fill_rect(target: &mut [u8], width: u32, height: u32, rect: SceneRect, color: Rgba) {
    let left = rect.x.floor().max(0.0) as u32;
    let top = rect.y.floor().max(0.0) as u32;
    let right = (rect.x + rect.width).ceil().clamp(0.0, width as f32) as u32;
    let bottom = (rect.y + rect.height).ceil().clamp(0.0, height as f32) as u32;
    let pixel = [
        (color.r.clamp(0.0, 1.0) * 255.0) as u8,
        (color.g.clamp(0.0, 1.0) * 255.0) as u8,
        (color.b.clamp(0.0, 1.0) * 255.0) as u8,
        (color.a.clamp(0.0, 1.0) * 255.0) as u8,
    ];
    for y in top..bottom {
        for x in left..right {
            let offset = (y as usize * width as usize + x as usize) * 4;
            target[offset..offset + 4].copy_from_slice(&pixel);
        }
    }
}

fn draw_border(
    target: &mut [u8],
    width: u32,
    height: u32,
    rect: SceneRect,
    border: f32,
    color: Rgba,
) {
    let border = border.max(1.0).min(rect.width.min(rect.height) / 2.0);
    let strips = [
        SceneRect {
            height: border,
            ..rect
        },
        SceneRect {
            y: rect.y + rect.height - border,
            height: border,
            ..rect
        },
        SceneRect {
            width: border,
            ..rect
        },
        SceneRect {
            x: rect.x + rect.width - border,
            width: border,
            ..rect
        },
    ];
    for strip in strips {
        fill_rect(target, width, height, strip, color);
    }
}

fn draw_reference_text(
    target: &mut [u8],
    width: u32,
    height: u32,
    bounds: SceneRect,
    content: &str,
    color: Rgba,
) {
    let count = content.chars().count().max(1) as f32;
    let cell = (bounds.width / count).max(1.0);
    for (index, character) in content.chars().enumerate() {
        if character.is_whitespace() {
            continue;
        }
        fill_rect(
            target,
            width,
            height,
            SceneRect {
                x: bounds.x + index as f32 * cell,
                y: bounds.y + bounds.height * 0.2,
                width: (cell * 0.65).max(1.0),
                height: (bounds.height * 0.65).max(1.0),
            },
            color,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_shaped_text(
    target: &mut [u8],
    target_width: u32,
    target_height: u32,
    bounds: SceneRect,
    content: &str,
    size: f32,
    color: Rgba,
    bold: bool,
    italic: bool,
    monospace: bool,
) -> bool {
    let width = bounds.width.ceil().clamp(1.0, 8_192.0) as u32;
    let height = bounds.height.ceil().clamp(1.0, 8_192.0) as u32;
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    let Ok(shaped) = crate::text_shaper::rasterize_text(
        content,
        width,
        height,
        crate::text_shaper::TextShapeStyle {
            size,
            bold,
            italic,
            monospace,
            color: [byte(color.r), byte(color.g), byte(color.b), byte(color.a)],
        },
    ) else {
        return false;
    };
    blit_alpha_surface(
        target,
        target_width,
        target_height,
        bounds.x.floor() as i32,
        bounds.y.floor() as i32,
        shaped.width,
        shaped.height,
        &shaped.rgba,
    );
    true
}

#[allow(clippy::too_many_arguments)]
fn blit_alpha_surface(
    target: &mut [u8],
    target_width: u32,
    target_height: u32,
    left: i32,
    top: i32,
    source_width: u32,
    source_height: u32,
    source: &[u8],
) {
    for source_y in 0..source_height {
        let target_y = top + source_y as i32;
        if target_y < 0 || target_y >= target_height as i32 {
            continue;
        }
        for source_x in 0..source_width {
            let target_x = left + source_x as i32;
            if target_x < 0 || target_x >= target_width as i32 {
                continue;
            }
            let source_offset = (source_y as usize * source_width as usize + source_x as usize) * 4;
            let target_offset = (target_y as usize * target_width as usize + target_x as usize) * 4;
            let alpha = u16::from(source[source_offset + 3]);
            if alpha == 0 {
                continue;
            }
            let inverse = 255_u16.saturating_sub(alpha);
            for channel in 0..3 {
                target[target_offset + channel] = ((u16::from(source[source_offset + channel])
                    * alpha
                    + u16::from(target[target_offset + channel]) * inverse)
                    / 255) as u8;
            }
            target[target_offset + 3] =
                (alpha + u16::from(target[target_offset + 3]) * inverse / 255).min(255) as u8;
        }
    }
}

fn blit_surface(
    target: &mut [u8],
    target_width: u32,
    target_height: u32,
    bounds: SceneRect,
    source_width: u32,
    source_height: u32,
    source: &[u8],
) {
    let left = bounds.x.floor().max(0.0) as u32;
    let top = bounds.y.floor().max(0.0) as u32;
    let out_width = bounds.width.ceil().max(1.0) as u32;
    let out_height = bounds.height.ceil().max(1.0) as u32;
    for y in 0..out_height {
        let target_y = top.saturating_add(y);
        if target_y >= target_height {
            break;
        }
        let source_y = (y as u64 * source_height as u64 / out_height as u64) as u32;
        for x in 0..out_width {
            let target_x = left.saturating_add(x);
            if target_x >= target_width {
                break;
            }
            let source_x = (x as u64 * source_width as u64 / out_width as u64) as u32;
            let source_offset = (source_y as usize * source_width as usize + source_x as usize) * 4;
            let target_offset = (target_y as usize * target_width as usize + target_x as usize) * 4;
            target[target_offset..target_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LossyGpu {
        fail_once: bool,
    }

    impl OptionalGpuAdapter for LossyGpu {
        fn render_gpu(
            &mut self,
            scene: &RetainedScene,
            width: u32,
            height: u32,
        ) -> Result<CompositedFrame, String> {
            if self.fail_once {
                self.fail_once = false;
                return Err("device lost".to_string());
            }
            CpuCompositor.render(scene, width, height)
        }

        fn recover_device(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn scene_retains_damage_video_and_cpu_pixels() {
        let list = DisplayList {
            items: vec![DisplayItem::Rect {
                x: 0.0,
                y: 0.0,
                w: 8.0,
                h: 8.0,
                color: Rgba::rgb(1.0, 0.0, 0.0),
            }],
            width: 8.0,
            height: 8.0,
            ..Default::default()
        };
        let mut scene = RetainedScene::from_display_list(1, &list).unwrap();
        scene.clear_damage();
        scene
            .upsert_video_surface(
                99,
                SceneRect::new(2.0, 2.0, 2.0, 2.0).unwrap(),
                1,
                1,
                vec![0, 255, 0, 255],
            )
            .unwrap();
        assert_eq!(scene.damage().len(), 1);
        let frame = CpuCompositor.render(&scene, 8, 8).unwrap();
        let green = (2usize * 8 + 2) * 4;
        assert_eq!(&frame.rgba[green..green + 4], &[0, 255, 0, 255]);
    }

    #[test]
    fn device_loss_falls_back_and_next_gpu_frame_matches_reference() {
        let list = DisplayList {
            items: vec![DisplayItem::Rect {
                x: 0.0,
                y: 0.0,
                w: 4.0,
                h: 4.0,
                color: Rgba::rgb(0.0, 0.0, 1.0),
            }],
            width: 4.0,
            height: 4.0,
            ..Default::default()
        };
        let scene = RetainedScene::from_display_list(1, &list).unwrap();
        let reference = CpuCompositor.render(&scene, 4, 4).unwrap();
        let mut compositor = ResilientCompositor::new(Some(LossyGpu { fail_once: true }));
        let fallback = compositor.render(&scene, 4, 4).unwrap();
        assert!(fallback.used_cpu_fallback);
        assert_eq!(fallback.rgba, reference.rgba);
        let recovered = compositor.render(&scene, 4, 4).unwrap();
        assert!(!recovered.used_cpu_fallback);
        assert_eq!(recovered.rgba, reference.rgba);
        assert_eq!(compositor.device_losses(), 1);
        assert_eq!(percentile_95_ms(&[4, 5, 6, 7, 8]), Some(8));
    }
}

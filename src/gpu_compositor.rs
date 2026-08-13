//! Optional DX12 compute compositor for the browser-owned retained scene.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::paint::Rgba;
use crate::scene_compositor::{CompositedFrame, OptionalGpuAdapter, RetainedScene, ScenePrimitive};

const MAX_GPU_SOURCE_BYTES: usize = 512 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuPrimitive {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    kind: u32,
    color: u32,
    border: u32,
    source_offset: u32,
    source_width: u32,
    source_height: u32,
    reserved0: u32,
    reserved1: u32,
}

struct GpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    adapter_name: String,
    faulted: Arc<AtomicBool>,
}

pub struct WgpuCompositor {
    state: Option<GpuState>,
}

impl WgpuCompositor {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            state: Some(create_state()?),
        })
    }

    pub fn adapter_name(&self) -> Option<&str> {
        self.state.as_ref().map(|state| state.adapter_name.as_str())
    }

    #[cfg(test)]
    fn inject_device_loss(&mut self) {
        self.state = None;
    }
}

impl OptionalGpuAdapter for WgpuCompositor {
    fn render_gpu(
        &mut self,
        scene: &RetainedScene,
        width: u32,
        height: u32,
    ) -> Result<CompositedFrame, String> {
        let state = self
            .state
            .as_ref()
            .ok_or_else(|| "GPU device is unavailable".to_string())?;
        if state.faulted.load(Ordering::Acquire) {
            return Err("GPU device reported an uncaptured error".to_string());
        }
        let pixel_count = width as usize * height as usize;
        if width == 0 || height == 0 || pixel_count > 64 * 1024 * 1024 {
            return Err("GPU output surface exceeds the compositor budget".to_string());
        }
        let (mut primitives, mut sources) = prepare_scene(scene, width, height)?;
        let primitive_count = primitives.len() as u32;
        if primitives.is_empty() {
            primitives.push(GpuPrimitive::zeroed());
        }
        if sources.is_empty() {
            sources.push(0);
        }
        let output_size = (pixel_count * 4) as u64;
        let primitive_buffer = state
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ghita-gpu-primitives"),
                contents: bytemuck::cast_slice(&primitives),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let source_buffer = state
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ghita-gpu-sources"),
                contents: bytemuck::cast_slice(&sources),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let output_buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ghita-gpu-output"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ghita-gpu-readback"),
            size: output_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let config = [width, height, primitive_count, 0_u32];
        let config_buffer = state
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ghita-gpu-config"),
                contents: bytemuck::cast_slice(&config),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ghita-gpu-bind-group"),
            layout: &state.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: primitive_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: source_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: config_buffer.as_entire_binding(),
                },
            ],
        });
        let mut encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ghita-gpu-encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ghita-gpu-composite-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&state.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&output_buffer, 0, &readback, 0, output_size);
        let submission = state.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        let _ = state
            .device
            .poll(wgpu::Maintain::WaitForSubmissionIndex(submission));
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "GPU readback timed out".to_string())??;
        let rgba = slice.get_mapped_range().to_vec();
        readback.unmap();
        if state.faulted.load(Ordering::Acquire) {
            return Err("GPU device faulted while compositing".to_string());
        }
        Ok(CompositedFrame {
            width,
            height,
            rgba,
            scene_revision: scene.revision,
            used_cpu_fallback: false,
        })
    }

    fn recover_device(&mut self) -> Result<(), String> {
        self.state = Some(create_state()?);
        Ok(())
    }
}

fn create_state() -> Result<GpuState, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .ok_or_else(|| "No compatible DX12 adapter".to_string())?;
    let adapter_name = adapter.get_info().name;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("GhitaBrowser retained compositor"),
            ..Default::default()
        },
        None,
    ))
    .map_err(|error| format!("Cannot create GPU device: {error}"))?;
    let faulted = Arc::new(AtomicBool::new(false));
    let error_flag = Arc::clone(&faulted);
    device.on_uncaptured_error(Box::new(move |_error| {
        error_flag.store(true, Ordering::Release);
    }));
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("GhitaBrowser compositor shader"),
        source: wgpu::ShaderSource::Wgsl(GPU_SHADER.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("GhitaBrowser compositor pipeline"),
        layout: None,
        module: &shader,
        entry_point: "main",
    });
    Ok(GpuState {
        device,
        queue,
        pipeline,
        adapter_name,
        faulted,
    })
}

fn prepare_scene(
    scene: &RetainedScene,
    target_width: u32,
    target_height: u32,
) -> Result<(Vec<GpuPrimitive>, Vec<u32>), String> {
    let mut primitives = Vec::with_capacity(scene.primitives().len());
    let mut sources = Vec::new();
    for primitive in scene.primitives() {
        let bounds = primitive.bounds();
        let mut metadata = GpuPrimitive {
            left: bounds.x.floor() as i32,
            top: bounds.y.floor() as i32,
            right: (bounds.x + bounds.width).ceil() as i32,
            bottom: (bounds.y + bounds.height).ceil() as i32,
            ..GpuPrimitive::zeroed()
        };
        match primitive {
            ScenePrimitive::SolidRect { color, .. } => {
                metadata.kind = 0;
                metadata.color = pack_color(*color);
                clip_metadata(&mut metadata, target_width, target_height);
            }
            ScenePrimitive::Border {
                width,
                color,
                bounds,
                ..
            } => {
                metadata.kind = 1;
                metadata.color = pack_color(*color);
                metadata.border = width
                    .max(1.0)
                    .min(bounds.width.min(bounds.height) / 2.0)
                    .ceil() as u32;
                clip_metadata(&mut metadata, target_width, target_height);
            }
            ScenePrimitive::Text {
                content,
                size,
                color,
                bold,
                italic,
                monospace,
                ..
            } => {
                let source_width = bounds.width.ceil().clamp(1.0, 8_192.0) as u32;
                let source_height = bounds.height.ceil().clamp(1.0, 8_192.0) as u32;
                let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                let shaped = crate::text_shaper::rasterize_text(
                    content,
                    source_width,
                    source_height,
                    crate::text_shaper::TextShapeStyle {
                        size: *size,
                        bold: *bold,
                        italic: *italic,
                        monospace: *monospace,
                        color: [byte(color.r), byte(color.g), byte(color.b), byte(color.a)],
                    },
                )?;
                metadata.kind = 2;
                metadata.right = metadata.left.saturating_add(source_width as i32);
                metadata.bottom = metadata.top.saturating_add(source_height as i32);
                append_source(
                    &mut metadata,
                    &mut sources,
                    &shaped.rgba,
                    source_width,
                    source_height,
                )?;
            }
            ScenePrimitive::ImagePlaceholder { ready, .. } => {
                metadata.kind = 0;
                metadata.color = pack_color(if *ready {
                    Rgba::rgb(0.75, 0.75, 0.75)
                } else {
                    Rgba::rgb(0.45, 0.45, 0.45)
                });
                clip_metadata(&mut metadata, target_width, target_height);
            }
            ScenePrimitive::VideoSurface {
                width,
                height,
                rgba,
                ..
            } => {
                metadata.kind = 4;
                metadata.left = metadata.left.max(0);
                metadata.top = metadata.top.max(0);
                metadata.right = metadata
                    .left
                    .saturating_add(bounds.width.ceil().max(1.0) as i32);
                metadata.bottom = metadata
                    .top
                    .saturating_add(bounds.height.ceil().max(1.0) as i32);
                append_source(&mut metadata, &mut sources, rgba, *width, *height)?;
            }
        }
        primitives.push(metadata);
    }
    Ok((primitives, sources))
}

fn append_source(
    metadata: &mut GpuPrimitive,
    sources: &mut Vec<u32>,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(), String> {
    if !rgba.len().is_multiple_of(4) || rgba.len() > MAX_GPU_SOURCE_BYTES {
        return Err("GPU source surface exceeds the byte budget".to_string());
    }
    let projected = sources
        .len()
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(rgba.len()))
        .ok_or_else(|| "GPU source byte count overflow".to_string())?;
    if projected > MAX_GPU_SOURCE_BYTES {
        return Err("GPU scene sources exceed the byte budget".to_string());
    }
    metadata.source_offset =
        u32::try_from(sources.len()).map_err(|_| "GPU source offset overflow".to_string())?;
    metadata.source_width = width;
    metadata.source_height = height;
    sources.extend(rgba.chunks_exact(4).map(|pixel| {
        u32::from(pixel[0])
            | (u32::from(pixel[1]) << 8)
            | (u32::from(pixel[2]) << 16)
            | (u32::from(pixel[3]) << 24)
    }));
    Ok(())
}

fn clip_metadata(metadata: &mut GpuPrimitive, width: u32, height: u32) {
    metadata.left = metadata.left.clamp(0, width as i32);
    metadata.top = metadata.top.clamp(0, height as i32);
    metadata.right = metadata.right.clamp(0, width as i32);
    metadata.bottom = metadata.bottom.clamp(0, height as i32);
}

fn pack_color(color: Rgba) -> u32 {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0) as u32;
    byte(color.r) | (byte(color.g) << 8) | (byte(color.b) << 16) | (byte(color.a) << 24)
}

const GPU_SHADER: &str = r#"
struct Primitive {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    kind: u32,
    color: u32,
    border: u32,
    source_offset: u32,
    source_width: u32,
    source_height: u32,
    reserved0: u32,
    reserved1: u32,
};

struct Config {
    width: u32,
    height: u32,
    primitive_count: u32,
    reserved: u32,
};

@group(0) @binding(0) var<storage, read> primitives: array<Primitive>;
@group(0) @binding(1) var<storage, read> sources: array<u32>;
@group(0) @binding(2) var<storage, read_write> output_pixels: array<u32>;
@group(0) @binding(3) var<uniform> config: Config;

fn channel(color: u32, shift: u32) -> u32 {
    return (color >> shift) & 255u;
}

fn blend(destination: u32, source: u32) -> u32 {
    let alpha = channel(source, 24u);
    if (alpha == 0u) { return destination; }
    if (alpha == 255u) { return source; }
    let inverse = 255u - alpha;
    let red = (channel(source, 0u) * alpha + channel(destination, 0u) * inverse) / 255u;
    let green = (channel(source, 8u) * alpha + channel(destination, 8u) * inverse) / 255u;
    let blue = (channel(source, 16u) * alpha + channel(destination, 16u) * inverse) / 255u;
    let out_alpha = min(255u, alpha + channel(destination, 24u) * inverse / 255u);
    return red | (green << 8u) | (blue << 16u) | (out_alpha << 24u);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) global: vec3<u32>) {
    if (global.x >= config.width || global.y >= config.height) { return; }
    let x = i32(global.x);
    let y = i32(global.y);
    var result = 0u;
    for (var index = 0u; index < config.primitive_count; index = index + 1u) {
        let primitive = primitives[index];
        if (x < primitive.left || x >= primitive.right || y < primitive.top || y >= primitive.bottom) {
            continue;
        }
        var source = 0u;
        if (primitive.kind == 0u) {
            source = primitive.color;
        } else if (primitive.kind == 1u) {
            let border = i32(primitive.border);
            if (x < primitive.left + border || x >= primitive.right - border || y < primitive.top + border || y >= primitive.bottom - border) {
                source = primitive.color;
            }
        } else if (primitive.kind == 2u) {
            let source_x = u32(x - primitive.left);
            let source_y = u32(y - primitive.top);
            if (source_x < primitive.source_width && source_y < primitive.source_height) {
                source = sources[primitive.source_offset + source_y * primitive.source_width + source_x];
            }
        } else if (primitive.kind == 4u) {
            let output_width = u32(max(1, primitive.right - primitive.left));
            let output_height = u32(max(1, primitive.bottom - primitive.top));
            let source_x = u32(x - primitive.left) * primitive.source_width / output_width;
            let source_y = u32(y - primitive.top) * primitive.source_height / output_height;
            source = sources[primitive.source_offset + source_y * primitive.source_width + source_x];
        }
        result = blend(result, source);
    }
    output_pixels[global.y * config.width + global.x] = result;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::{DisplayItem, DisplayList};
    use crate::scene_compositor::CpuCompositor;

    fn reference_scene() -> RetainedScene {
        RetainedScene::from_display_list(
            7,
            &DisplayList {
                items: vec![
                    DisplayItem::Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 32.0,
                        h: 32.0,
                        color: Rgba::rgb(0.1, 0.2, 0.3),
                    },
                    DisplayItem::Border {
                        x: 4.0,
                        y: 4.0,
                        w: 20.0,
                        h: 20.0,
                        width: 2.0,
                        color: Rgba::rgb(1.0, 0.0, 0.0),
                    },
                    DisplayItem::TextRun {
                        x: 5.0,
                        y: 6.0,
                        size: 8.0,
                        color: Rgba::WHITE,
                        content: "Xin chào".to_string(),
                        bold: false,
                        italic: false,
                        underline: false,
                        monospace: false,
                    },
                ],
                width: 32.0,
                height: 32.0,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn dx12_output_is_pixel_stable_and_recovers_after_injected_loss() {
        let mut gpu = WgpuCompositor::new()
            .expect("Phase 20 Windows gate requires an available DX12 compositor adapter");
        let scene = reference_scene();
        let cpu = CpuCompositor.render(&scene, 32, 32).unwrap();
        let frame = gpu.render_gpu(&scene, 32, 32).unwrap();
        assert_eq!(frame.rgba, cpu.rgba);

        gpu.inject_device_loss();
        assert!(gpu.render_gpu(&scene, 32, 32).is_err());
        gpu.recover_device().unwrap();
        let mut frame_times = Vec::new();
        for _ in 0..120 {
            let started = std::time::Instant::now();
            let recovered = gpu.render_gpu(&scene, 32, 32).unwrap();
            assert_eq!(recovered.rgba, cpu.rgba);
            frame_times.push(started.elapsed().as_millis() as u64);
        }
        let p95 = crate::scene_compositor::percentile_95_ms(&frame_times).unwrap();
        assert!(p95 <= 32, "GPU animation p95 regressed to {p95}ms");
    }
}

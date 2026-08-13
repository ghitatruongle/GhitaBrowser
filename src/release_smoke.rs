//! Headless smoke gate executed against an installed release binary.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

pub fn run(installed_executable: &Path) -> Result<Value, String> {
    let worker = installed_executable.with_file_name(format!(
        "ghita-renderer-worker{}",
        std::env::consts::EXE_SUFFIX
    ));
    if !worker.is_file() {
        return Err(format!(
            "Installed renderer worker is missing: {}",
            worker.display()
        ));
    }
    let request = crate::worker::PreparationRequest {
        html: "<html><head><title>Release smoke</title></head><body><h1>HTML worker ready</h1></body></html>".to_string(),
        fallback_title: "smoke".to_string(),
        base_rules: Vec::new(),
        viewport_width: 640,
        viewport_height: 360,
    };
    let prepared = crate::worker::prepare_with_program(&worker, &request, Duration::from_secs(15))
        .map_err(|error| error.to_string())?;
    if prepared.title != "Release smoke" || !prepared.rendered_text.contains("HTML worker ready") {
        return Err("Installed renderer worker returned invalid document output".to_string());
    }

    let live = crate::document::prepare_live_document(
        "<main><h1>Xin chào العربية</h1><script>let node=document.querySelector('h1');node.textContent='Runtime ready العربية';</script></main>",
        "https://release-smoke.test/",
        &[],
        640,
    );
    let render = live.render_state();
    let scene = crate::scene_compositor::RetainedScene::from_display_list(
        render.revision,
        &render.display_list,
    )?;
    let page_frame = crate::scene_compositor::CpuCompositor.render(&scene, 640, 360)?;
    if !page_frame.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0) {
        return Err("Installed page compositor produced an empty frame".to_string());
    }

    #[cfg(windows)]
    let gpu_adapter = {
        use crate::scene_compositor::OptionalGpuAdapter;

        match crate::gpu_compositor::WgpuCompositor::new() {
            Ok(mut gpu) => {
                let adapter = gpu
                    .adapter_name()
                    .unwrap_or("unknown DX12 adapter")
                    .to_string();
                let gpu_frame = gpu.render_gpu(&scene, 640, 360)?;
                if gpu_frame.rgba != page_frame.rgba {
                    return Err("Installed CPU/GPU compositor pixels diverged".to_string());
                }
                Some(adapter)
            }
            Err(_) => None,
        }
    };

    #[cfg(windows)]
    let decoded = crate::media_backend::decode_clear_content_bytes(include_bytes!(
        "../tests/fixtures/media/clear-avc-aac.mp4"
    ))?;
    #[cfg(windows)]
    {
        use crate::audio_output::{AudioSink, MemoryAudioSink};
        use crate::html_media::MediaControlAction;
        use crate::media_runtime::{MediaRuntimeLimits, PageMediaRuntime};
        use crate::runtime_core::{RuntimeLimits, RuntimeRealm};

        let video_frames = decoded.video_frames.len();
        let audio_frames = decoded.audio_frames.len();
        if video_frames == 0 || audio_frames == 0 {
            return Err("Installed Media Foundation path produced no A/V output".to_string());
        }
        let audio_sample_rate = decoded.audio_frames[0].sample_rate_hz;
        let audio_channels = decoded.audio_frames[0].channels;
        let realm = RuntimeRealm::new(0x0052_454c_4541_5345, RuntimeLimits::default())?;
        let mut media = PageMediaRuntime::new(realm, MediaRuntimeLimits::default());
        let element = media.create_media_element()?;
        media.attach_decoded_output(element.id, decoded)?;
        media.apply_control(element.id, MediaControlAction::TogglePlayback)?;
        let tick = media.tick(element.id, 20)?;
        let mut sink = MemoryAudioSink::new(audio_sample_rate, audio_channels, 2)?;
        let written = media
            .output_mut(element.id)
            .ok_or_else(|| "Installed media output disappeared".to_string())?
            .write_audio_to(&mut sink)?;
        if !tick.video_frame_presented
            || tick.audio_frames_emitted == 0
            || written == 0
            || sink.queued_samples() == 0
        {
            return Err("Installed media output did not present synchronized A/V".to_string());
        }
        sink.flush()?;
        media.teardown();
        if media.live_binding_count() != 0 || sink.queued_samples() != 0 {
            return Err("Installed media smoke teardown leaked state".to_string());
        }
        Ok(json!({
            "passed": true,
            "version": crate::VERSION,
            "worker": true,
            "runtime": true,
            "scene": true,
            "gpu_adapter": gpu_adapter,
            "video_frames": video_frames,
            "audio_frames": audio_frames,
            "media_tick_audio": tick.audio_frames_emitted,
            "media_tick_video": tick.video_frame_presented,
            "teardown_bindings": media.live_binding_count()
        }))
    }

    #[cfg(not(windows))]
    Ok(json!({
        "passed": true,
        "version": crate::VERSION,
        "worker": true,
        "runtime": true,
        "scene": true,
        "media": "windows-only"
    }))
}

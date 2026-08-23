//! Page-owned media bindings and bounded decoded audio/video output.
//!
//! This module bridges runtime host objects, the HTML media state machine,
//! decoded platform frames and the toolkit-independent retained scene. It does
//! not decode compressed SourceBuffer samples by itself; decoder backends feed
//! `DecodedMediaAsset` into `MediaOutputPipeline`.

use std::collections::{BTreeMap, VecDeque};

use crate::audio_output::AudioSink;
use crate::html_media::{HtmlMediaElement, MediaControlAction, MediaEvent};
use crate::media_backend::{decode_clear_content_bytes, DecodedMediaAsset, DecoderCapabilities};
use crate::media_core::{
    video_sync_action, AudioClock, BoundedFrameQueue, DecodedAudioFrame, DecodedFrame,
    DecodedVideoFrame, VideoSyncAction,
};
use crate::mse::{AppendReport, MediaSource};
use crate::runtime_core::{HeapHandle, HostObjectKind, RuntimeRealm, RuntimeValue};
use crate::scene_compositor::{RetainedScene, SceneRect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaRuntimeLimits {
    pub max_elements: usize,
    pub max_sources: usize,
    pub max_events: usize,
    pub max_video_frames: usize,
    pub max_audio_frames: usize,
    pub max_decoded_bytes: usize,
    pub sync_tolerance_us: u64,
}

impl Default for MediaRuntimeLimits {
    fn default() -> Self {
        Self {
            max_elements: 32,
            max_sources: 32,
            max_events: 2_048,
            max_video_frames: 1_200,
            max_audio_frames: 16_384,
            max_decoded_bytes: 64 * 1024 * 1024,
            sync_tolerance_us: 40_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaElementBinding {
    pub id: u64,
    pub heap_handle: HeapHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaSourceBinding {
    pub id: u64,
    pub heap_handle: HeapHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaOutputTick {
    pub audio_frames_emitted: usize,
    pub video_frame_presented: bool,
    pub video_frames_dropped: usize,
    pub sync_action: VideoSyncAction,
}

#[derive(Debug, Clone)]
pub struct MediaOutputPipeline {
    video: BoundedFrameQueue,
    audio: BoundedFrameQueue,
    clock: AudioClock,
    sample_rate_hz: u32,
    tolerance_us: u64,
    current_video: Option<DecodedVideoFrame>,
    emitted_audio: VecDeque<DecodedAudioFrame>,
    max_emitted_audio: usize,
}

#[derive(Debug, Clone)]
pub struct BoundedStreamingDecoder {
    bytes: Vec<u8>,
    max_input_bytes: usize,
    finalized: bool,
}

impl BoundedStreamingDecoder {
    pub fn new(max_input_bytes: usize) -> Result<Self, String> {
        if max_input_bytes == 0 || max_input_bytes > 64 * 1024 * 1024 {
            return Err("Streaming decoder input budget is invalid".to_string());
        }
        Ok(Self {
            bytes: Vec::new(),
            max_input_bytes,
            finalized: false,
        })
    }

    pub fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.finalized {
            return Err("Streaming decoder is already finalized".to_string());
        }
        let projected = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| "Streaming decoder byte count overflow".to_string())?;
        if projected > self.max_input_bytes {
            return Err("Streaming decoder input budget exceeded".to_string());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub fn finalize(&mut self, limits: MediaRuntimeLimits) -> Result<MediaOutputPipeline, String> {
        if self.finalized {
            return Err("Streaming decoder is already finalized".to_string());
        }
        self.finalized = true;
        let decoded = decode_clear_content_bytes(&self.bytes)?;
        self.bytes.clear();
        self.bytes.shrink_to_fit();
        MediaOutputPipeline::from_asset(decoded, limits)
    }

    pub fn buffered_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub fn cancel(&mut self) {
        self.bytes.clear();
        self.bytes.shrink_to_fit();
        self.finalized = true;
    }
}

impl MediaOutputPipeline {
    pub fn from_asset(
        asset: DecodedMediaAsset,
        limits: MediaRuntimeLimits,
    ) -> Result<Self, String> {
        if asset.video_frames.is_empty() && asset.audio_frames.is_empty() {
            return Err("Decoded media asset has no output frames".to_string());
        }
        let sample_rate_hz = asset
            .audio_frames
            .first()
            .map(|frame| frame.sample_rate_hz)
            .unwrap_or(48_000);
        let video_budget = limits.max_decoded_bytes.saturating_mul(3) / 4;
        let audio_budget = limits.max_decoded_bytes.saturating_sub(video_budget);
        let mut video = BoundedFrameQueue::new(limits.max_video_frames, video_budget);
        let mut audio = BoundedFrameQueue::new(limits.max_audio_frames, audio_budget);
        for frame in asset.video_frames {
            validate_video_frame(&frame)?;
            video.push(DecodedFrame::Video(frame))?;
        }
        for frame in asset.audio_frames {
            validate_audio_frame(&frame, sample_rate_hz)?;
            audio.push(DecodedFrame::Audio(frame))?;
        }
        let mut clock = AudioClock::new(sample_rate_hz)?;
        clock.seek(0)?;
        Ok(Self {
            video,
            audio,
            clock,
            sample_rate_hz,
            tolerance_us: limits.sync_tolerance_us,
            current_video: None,
            emitted_audio: VecDeque::new(),
            max_emitted_audio: limits.max_audio_frames,
        })
    }

    pub fn play(&mut self) {
        self.clock.start();
    }

    pub fn pause(&mut self) {
        self.clock.pause();
    }

    pub fn seek(&mut self, timestamp_us: i64) -> Result<(), String> {
        if timestamp_us < self.clock.position_us() {
            return Err("Backward output seek requires a decoder flush and refill".to_string());
        }
        self.clock.seek(timestamp_us)?;
        self.current_video = None;
        self.emitted_audio.clear();
        discard_before(&mut self.video, timestamp_us);
        discard_before(&mut self.audio, timestamp_us);
        Ok(())
    }

    pub fn tick(&mut self, elapsed_ms: u64) -> Result<MediaOutputTick, String> {
        if self.clock.is_running() {
            let sample_rate = u64::from(self.sample_rate_hz);
            let frames = elapsed_ms.saturating_mul(sample_rate) / 1_000;
            self.clock.advance_frames(frames)?;
        }
        let position = self.clock.position_us();
        let ready_limit = position.saturating_add(self.tolerance_us as i64);
        let mut audio_frames_emitted = 0;
        while self
            .audio
            .front()
            .is_some_and(|frame| frame.timestamp_us() <= ready_limit)
        {
            let Some(DecodedFrame::Audio(frame)) = self.audio.pop() else {
                break;
            };
            if self.emitted_audio.len() >= self.max_emitted_audio {
                self.emitted_audio.pop_front();
            }
            self.emitted_audio.push_back(frame);
            audio_frames_emitted += 1;
        }

        let mut video_frame_presented = false;
        let mut video_frames_dropped = 0;
        let mut last_action = VideoSyncAction::Hold;
        while let Some(timestamp) = self.video.front().map(DecodedFrame::timestamp_us) {
            let action = video_sync_action(position, timestamp, self.tolerance_us);
            last_action = action;
            match action {
                VideoSyncAction::Hold => break,
                VideoSyncAction::Drop => {
                    let _ = self.video.pop();
                    video_frames_dropped += 1;
                }
                VideoSyncAction::Present => {
                    let Some(DecodedFrame::Video(frame)) = self.video.pop() else {
                        break;
                    };
                    self.current_video = Some(frame);
                    video_frame_presented = true;
                    break;
                }
            }
        }
        Ok(MediaOutputTick {
            audio_frames_emitted,
            video_frame_presented,
            video_frames_dropped,
            sync_action: last_action,
        })
    }

    pub fn current_video_frame(&self) -> Option<&DecodedVideoFrame> {
        self.current_video.as_ref()
    }

    pub fn drain_audio_frames(&mut self) -> Vec<DecodedAudioFrame> {
        self.emitted_audio.drain(..).collect()
    }

    pub fn write_audio_to(&mut self, sink: &mut dyn AudioSink) -> Result<usize, String> {
        let mut written = 0usize;
        while let Some(frame) = self.emitted_audio.pop_front() {
            sink.enqueue(frame)?;
            written = written.saturating_add(1);
        }
        Ok(written)
    }

    pub fn write_video_surface(
        &self,
        scene: &mut RetainedScene,
        primitive_id: u64,
        rect: SceneRect,
    ) -> Result<bool, String> {
        let Some(frame) = self.current_video.as_ref() else {
            return Ok(false);
        };
        scene.upsert_video_surface(
            primitive_id,
            rect,
            frame.width,
            frame.height,
            frame.rgba.clone(),
        )?;
        Ok(true)
    }

    pub fn queued_frames(&self) -> usize {
        self.video.len().saturating_add(self.audio.len())
    }

    pub fn queued_bytes(&self) -> usize {
        self.video
            .queued_bytes()
            .saturating_add(self.audio.queued_bytes())
    }

    pub fn clear(&mut self) {
        self.video.clear();
        self.audio.clear();
        self.emitted_audio.clear();
        self.current_video = None;
        self.clock.pause();
    }
}

#[derive(Debug)]
struct BoundElement {
    element: HtmlMediaElement,
    output: Option<MediaOutputPipeline>,
}

#[derive(Debug)]
struct BoundSource {
    source: Option<MediaSource>,
}

#[derive(Debug)]
pub struct PageMediaRuntime {
    pub realm: RuntimeRealm,
    limits: MediaRuntimeLimits,
    next_id: u64,
    elements: BTreeMap<u64, BoundElement>,
    sources: BTreeMap<u64, BoundSource>,
    events: VecDeque<(u64, MediaEvent)>,
}

impl PageMediaRuntime {
    pub fn new(realm: RuntimeRealm, limits: MediaRuntimeLimits) -> Self {
        Self {
            realm,
            limits,
            next_id: 1,
            elements: BTreeMap::new(),
            sources: BTreeMap::new(),
            events: VecDeque::new(),
        }
    }

    pub fn create_media_element(&mut self) -> Result<MediaElementBinding, String> {
        if self.elements.len() >= self.limits.max_elements {
            return Err("Page media element budget exceeded".to_string());
        }
        let (id, handle) = self.allocate_binding(HostObjectKind::MediaElement, "mediaElement")?;
        self.elements.insert(
            id,
            BoundElement {
                element: HtmlMediaElement::new(),
                output: None,
            },
        );
        Ok(MediaElementBinding {
            id,
            heap_handle: handle,
        })
    }

    pub fn create_media_source(&mut self) -> Result<MediaSourceBinding, String> {
        if self.sources.len() >= self.limits.max_sources {
            return Err("Page MediaSource budget exceeded".to_string());
        }
        let (id, handle) = self.allocate_binding(HostObjectKind::MediaSource, "mediaSource")?;
        let mut source = MediaSource::new();
        source.open()?;
        self.sources.insert(
            id,
            BoundSource {
                source: Some(source),
            },
        );
        Ok(MediaSourceBinding {
            id,
            heap_handle: handle,
        })
    }

    pub fn add_source_buffer(
        &mut self,
        source_id: u64,
        content_type: &str,
        capabilities: &DecoderCapabilities,
    ) -> Result<u32, String> {
        self.source_mut(source_id)?
            .add_source_buffer(content_type, capabilities)
    }

    pub fn append_buffer(
        &mut self,
        source_id: u64,
        buffer_id: u32,
        bytes: &[u8],
    ) -> Result<AppendReport, String> {
        self.source_mut(source_id)?.append_buffer(buffer_id, bytes)
    }

    pub fn set_source_duration(&mut self, source_id: u64, duration_us: i64) -> Result<(), String> {
        self.source_mut(source_id)?.set_duration(duration_us)
    }

    pub fn end_source_stream(&mut self, source_id: u64) -> Result<(), String> {
        self.source_mut(source_id)?.end_of_stream()
    }

    pub fn attach_source(&mut self, element_id: u64, source_id: u64) -> Result<(), String> {
        let source = self
            .sources
            .get_mut(&source_id)
            .and_then(|binding| binding.source.take())
            .ok_or_else(|| "MediaSource binding is stale or already attached".to_string())?;
        let element = self
            .elements
            .get_mut(&element_id)
            .ok_or_else(|| "Media element binding is stale".to_string())?;
        if let Err(error) = element.element.attach_media_source(source.clone()) {
            if let Some(binding) = self.sources.get_mut(&source_id) {
                binding.source = Some(source);
            }
            return Err(error);
        }
        self.collect_element_events(element_id);
        Ok(())
    }

    pub fn attach_decoded_output(
        &mut self,
        element_id: u64,
        asset: DecodedMediaAsset,
    ) -> Result<(), String> {
        let duration_us = asset
            .video_frames
            .iter()
            .map(|frame| {
                frame
                    .timestamp_us
                    .saturating_add(i64::try_from(frame.duration_us).unwrap_or(i64::MAX))
            })
            .chain(asset.audio_frames.iter().map(|frame| {
                frame
                    .timestamp_us
                    .saturating_add(i64::try_from(frame.duration_us).unwrap_or(i64::MAX))
            }))
            .max()
            .ok_or_else(|| "Decoded media asset has no duration".to_string())?;
        let output = MediaOutputPipeline::from_asset(asset, self.limits)?;
        let element = self
            .elements
            .get_mut(&element_id)
            .ok_or_else(|| "Media element binding is stale".to_string())?;
        element.element.attach_decoded_stream(duration_us)?;
        element.output = Some(output);
        self.collect_element_events(element_id);
        Ok(())
    }

    pub fn apply_control(
        &mut self,
        element_id: u64,
        action: MediaControlAction,
    ) -> Result<(), String> {
        let element = self
            .elements
            .get_mut(&element_id)
            .ok_or_else(|| "Media element binding is stale".to_string())?;
        let was_paused = element.element.paused();
        // Validate fallible output operations BEFORE mutating the element;
        // mutating first left the UI showing the new time while the pipeline
        // kept playing from the old position on rejected (backward) seeks.
        let seek_target_us = match action {
            MediaControlAction::SeekTo(seconds) => Some((seconds * 1_000_000.0) as i64),
            MediaControlAction::SeekBy(delta) => Some(
                (element.element.current_time_seconds() * 1_000_000.0) as i64
                    + (delta * 1_000_000.0) as i64,
            ),
            _ => None,
        };
        if let (Some(target), Some(output)) = (seek_target_us, element.output.as_ref()) {
            if target < output.clock.position_us() {
                return Err(
                    "NotSupportedError: backward seeking is not supported by the decoded pipeline"
                        .to_string(),
                );
            }
        }
        element.element.apply_control(action)?;
        if let Some(output) = element.output.as_mut() {
            match action {
                MediaControlAction::TogglePlayback if was_paused => output.play(),
                MediaControlAction::TogglePlayback => output.pause(),
                MediaControlAction::SeekTo(_) | MediaControlAction::SeekBy(_) => {
                    output.seek((element.element.current_time_seconds() * 1_000_000.0) as i64)?
                }
                _ => {}
            }
        }
        self.collect_element_events(element_id);
        Ok(())
    }

    pub fn tick(&mut self, element_id: u64, elapsed_ms: u64) -> Result<MediaOutputTick, String> {
        let element = self
            .elements
            .get_mut(&element_id)
            .ok_or_else(|| "Media element binding is stale".to_string())?;
        element.element.tick(elapsed_ms);
        let tick = element
            .output
            .as_mut()
            .ok_or_else(|| "Media element has no decoded output".to_string())?
            .tick(elapsed_ms)?;
        self.collect_element_events(element_id);
        Ok(tick)
    }

    pub fn output(&self, element_id: u64) -> Option<&MediaOutputPipeline> {
        self.elements
            .get(&element_id)
            .and_then(|element| element.output.as_ref())
    }

    pub fn output_mut(&mut self, element_id: u64) -> Option<&mut MediaOutputPipeline> {
        self.elements
            .get_mut(&element_id)
            .and_then(|element| element.output.as_mut())
    }

    pub fn drain_events(&mut self) -> Vec<(u64, MediaEvent)> {
        self.events.drain(..).collect()
    }

    pub fn teardown(&mut self) {
        for element in self.elements.values_mut() {
            if let Some(output) = element.output.as_mut() {
                output.clear();
            }
        }
        let element_ids = self.elements.keys().copied().collect::<Vec<_>>();
        self.elements.clear();
        for source in self.sources.values_mut() {
            if let Some(source) = source.source.as_mut() {
                source.close();
            }
        }
        let source_ids = self.sources.keys().copied().collect::<Vec<_>>();
        self.sources.clear();
        self.events.clear();
        for id in element_ids {
            let _ = self
                .realm
                .heap
                .remove_property(self.realm.document, &format!("mediaElement{id}"));
        }
        for id in source_ids {
            let _ = self
                .realm
                .heap
                .remove_property(self.realm.document, &format!("mediaSource{id}"));
        }
        let _ = self.realm.collect_garbage();
    }

    pub fn live_binding_count(&self) -> usize {
        self.elements.len().saturating_add(self.sources.len())
    }

    fn source_mut(&mut self, source_id: u64) -> Result<&mut MediaSource, String> {
        self.sources
            .get_mut(&source_id)
            .and_then(|binding| binding.source.as_mut())
            .ok_or_else(|| "MediaSource binding is stale or already attached".to_string())
    }

    fn allocate_binding(
        &mut self,
        kind: HostObjectKind,
        prefix: &str,
    ) -> Result<(u64, HeapHandle), String> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "Page media binding id space exhausted".to_string())?;
        let handle = self.realm.heap.allocate(kind)?;
        self.realm.heap.set_property(
            self.realm.document,
            &format!("{prefix}{id}"),
            RuntimeValue::Object(handle),
        )?;
        Ok((id, handle))
    }

    fn collect_element_events(&mut self, element_id: u64) {
        let Some(element) = self.elements.get_mut(&element_id) else {
            return;
        };
        for event in element.element.drain_events() {
            if self.events.len() >= self.limits.max_events {
                self.events.pop_front();
            }
            self.events.push_back((element_id, event));
        }
    }
}

impl Drop for PageMediaRuntime {
    fn drop(&mut self) {
        self.teardown();
    }
}

fn discard_before(queue: &mut BoundedFrameQueue, timestamp_us: i64) {
    while queue
        .front()
        .is_some_and(|frame| frame.timestamp_us() < timestamp_us)
    {
        let _ = queue.pop();
    }
}

fn validate_video_frame(frame: &DecodedVideoFrame) -> Result<(), String> {
    if frame.width == 0 || frame.height == 0 || frame.width > 4_096 || frame.height > 4_096 {
        return Err("Decoded video dimensions exceed output limits".to_string());
    }
    let expected = usize::try_from(frame.width)
        .ok()
        .and_then(|width| {
            usize::try_from(frame.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Decoded video dimensions overflow".to_string())?;
    if frame.rgba.len() != expected {
        return Err("Decoded video frame has an invalid RGBA buffer".to_string());
    }
    Ok(())
}

fn validate_audio_frame(frame: &DecodedAudioFrame, sample_rate_hz: u32) -> Result<(), String> {
    if frame.sample_rate_hz != sample_rate_hz || !(1..=8).contains(&frame.channels) {
        return Err("Decoded audio format changes require an output flush".to_string());
    }
    if !frame
        .interleaved_samples
        .len()
        .is_multiple_of(usize::from(frame.channels))
    {
        return Err("Decoded audio frame is not channel aligned".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_output::{AudioSink, MemoryAudioSink};
    use crate::runtime_core::RuntimeLimits;
    use crate::scene_compositor::CpuCompositor;

    fn decoded_asset() -> DecodedMediaAsset {
        DecodedMediaAsset {
            video_frames: vec![DecodedVideoFrame {
                timestamp_us: 0,
                duration_us: 40_000,
                width: 2,
                height: 1,
                rgba: vec![10, 20, 30, 255, 40, 50, 60, 255],
            }],
            audio_frames: vec![DecodedAudioFrame {
                timestamp_us: 0,
                duration_us: 20_000,
                sample_rate_hz: 48_000,
                channels: 2,
                interleaved_samples: vec![0; 1_920],
            }],
        }
    }

    #[test]
    fn default_media_budget_is_64_mb() {
        assert_eq!(
            MediaRuntimeLimits::default().max_decoded_bytes,
            64 * 1024 * 1024
        );
    }

    #[test]
    fn decoded_output_is_bounded_clocked_and_composited() {
        let mut output =
            MediaOutputPipeline::from_asset(decoded_asset(), MediaRuntimeLimits::default())
                .unwrap();
        output.play();
        let tick = output.tick(1).unwrap();
        assert_eq!(tick.audio_frames_emitted, 1);
        assert!(tick.video_frame_presented);
        let mut sink = MemoryAudioSink::new(48_000, 2, 1).unwrap();
        assert_eq!(output.write_audio_to(&mut sink).unwrap(), 1);
        assert_eq!(sink.queued_samples(), 1_920);
        let mut scene = RetainedScene::default();
        assert!(output
            .write_video_surface(
                &mut scene,
                7,
                SceneRect {
                    x: 0.0,
                    y: 0.0,
                    width: 2.0,
                    height: 1.0,
                },
            )
            .unwrap());
        let frame = CpuCompositor.render(&scene, 2, 1).unwrap();
        assert_eq!(&frame.rgba[..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn page_bindings_have_host_kinds_and_teardown_output() {
        let realm = RuntimeRealm::new(44, RuntimeLimits::default()).unwrap();
        let mut runtime = PageMediaRuntime::new(realm, MediaRuntimeLimits::default());
        let element = runtime.create_media_element().unwrap();
        let source = runtime.create_media_source().unwrap();
        assert_eq!(
            runtime.realm.heap.get(element.heap_handle).unwrap().kind,
            HostObjectKind::MediaElement
        );
        assert_eq!(
            runtime.realm.heap.get(source.heap_handle).unwrap().kind,
            HostObjectKind::MediaSource
        );
        runtime
            .attach_decoded_output(element.id, decoded_asset())
            .unwrap();
        assert!(runtime.output(element.id).unwrap().queued_bytes() > 0);
        runtime.teardown();
        assert_eq!(runtime.live_binding_count(), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn fragmented_byte_stream_decodes_to_bounded_output_without_a_file_path() {
        let bytes = include_bytes!("../tests/fixtures/media/clear-avc-aac.mp4");
        let mut decoder = BoundedStreamingDecoder::new(1024 * 1024).unwrap();
        for chunk in bytes.chunks(257) {
            decoder.append(chunk).unwrap();
        }
        assert_eq!(decoder.buffered_bytes(), bytes.len());
        let mut output = decoder.finalize(MediaRuntimeLimits::default()).unwrap();
        assert!(output.queued_frames() > 0);
        output.play();
        let tick = output.tick(20).unwrap();
        assert!(tick.audio_frames_emitted > 0);
        assert!(tick.video_frame_presented);
        assert_eq!(decoder.buffered_bytes(), 0);
    }
}

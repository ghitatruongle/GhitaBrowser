//! Bounded Media Source Extensions subset.
//!
//! This module owns SourceBuffer state and coded-frame processing. Compressed
//! samples remain opaque here and are handed to the selected decoder backend
//! only after container parsing and memory/timestamp validation.

use crate::iso_bmff::{parse_init_segment, parse_media_segment, TrackInfo};
use crate::media_backend::DecoderCapabilities;
use crate::media_core::{parse_media_type, EncodedSample, MediaContainer, ParsedMediaType};

const MAX_SOURCE_BUFFERS: usize = 8;
const MAX_SAMPLES_PER_BUFFER: usize = 100_000;
const MAX_BYTES_PER_BUFFER: usize = 128 * 1024 * 1024;
const MAX_TOTAL_MEDIA_BYTES: usize = 256 * 1024 * 1024;
const MAX_MEDIA_TIME_US: i64 = 24 * 60 * 60 * 1_000_000;
const RANGE_GAP_TOLERANCE_US: i64 = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaSourceReadyState {
    Closed,
    Open,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaSourceEvent {
    SourceOpen,
    SourceEnded,
    SourceClose,
    UpdateStart,
    Update,
    UpdateEnd,
    Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub start_us: i64,
    pub end_us: i64,
}

impl TimeRange {
    pub fn new(start_us: i64, end_us: i64) -> Result<Self, String> {
        if start_us < 0 || end_us <= start_us || end_us > MAX_MEDIA_TIME_US {
            return Err("Invalid media time range".to_string());
        }
        Ok(Self { start_us, end_us })
    }

    pub fn contains(self, timestamp_us: i64) -> bool {
        timestamp_us >= self.start_us && timestamp_us < self.end_us
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppendReport {
    pub initialization_segment: bool,
    pub appended_samples: usize,
    pub dropped_samples: usize,
    pub evicted_samples: usize,
    pub buffered: Vec<TimeRange>,
}

#[derive(Debug, Clone)]
pub struct SourceBuffer {
    id: u32,
    media_type: ParsedMediaType,
    tracks: Vec<TrackInfo>,
    samples: Vec<EncodedSample>,
    queued_bytes: usize,
    updating: bool,
    timestamp_offset_us: i64,
    append_window: TimeRange,
    events: Vec<MediaSourceEvent>,
}

impl SourceBuffer {
    fn new(id: u32, media_type: ParsedMediaType) -> Self {
        Self {
            id,
            media_type,
            tracks: Vec::new(),
            samples: Vec::new(),
            queued_bytes: 0,
            updating: false,
            timestamp_offset_us: 0,
            append_window: TimeRange {
                start_us: 0,
                end_us: MAX_MEDIA_TIME_US,
            },
            events: Vec::new(),
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn media_type(&self) -> &ParsedMediaType {
        &self.media_type
    }

    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    pub fn samples(&self) -> &[EncodedSample] {
        &self.samples
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn updating(&self) -> bool {
        self.updating
    }

    pub fn timestamp_offset_us(&self) -> i64 {
        self.timestamp_offset_us
    }

    pub fn set_timestamp_offset(&mut self, offset_us: i64) -> Result<(), String> {
        if self.updating || offset_us.unsigned_abs() > MAX_MEDIA_TIME_US as u64 {
            return Err("Invalid SourceBuffer timestamp offset".to_string());
        }
        self.timestamp_offset_us = offset_us;
        Ok(())
    }

    pub fn set_append_window(&mut self, range: TimeRange) -> Result<(), String> {
        if self.updating {
            return Err("Cannot change append window while updating".to_string());
        }
        self.append_window = range;
        Ok(())
    }

    pub fn append_buffer(&mut self, bytes: &[u8]) -> Result<AppendReport, String> {
        if self.updating {
            return Err("SourceBuffer is already updating".to_string());
        }
        let rollback = self.clone();
        self.updating = true;
        self.events.push(MediaSourceEvent::UpdateStart);
        let result = self.append_buffer_inner(bytes);
        self.updating = false;
        match result {
            Ok(report) => {
                self.events.push(MediaSourceEvent::Update);
                self.events.push(MediaSourceEvent::UpdateEnd);
                Ok(report)
            }
            Err(error) => {
                *self = rollback;
                self.events.push(MediaSourceEvent::UpdateStart);
                self.events.push(MediaSourceEvent::Abort);
                self.events.push(MediaSourceEvent::UpdateEnd);
                Err(error)
            }
        }
    }

    fn append_buffer_inner(&mut self, bytes: &[u8]) -> Result<AppendReport, String> {
        if self.media_type.container != MediaContainer::IsoBmff {
            return Err("This SourceBuffer currently requires ISO-BMFF segments".to_string());
        }
        if self.tracks.is_empty() {
            let tracks = parse_init_segment(bytes)?;
            if !self.media_type.codecs.is_empty()
                && tracks
                    .iter()
                    .any(|track| !self.media_type.codecs.contains(&track.codec))
            {
                return Err(
                    "Initialization segment codec does not match SourceBuffer type".to_string(),
                );
            }
            self.tracks = tracks;
            return Ok(AppendReport {
                initialization_segment: true,
                buffered: self.buffered(),
                ..Default::default()
            });
        }
        let parsed = parse_media_segment(bytes, &self.tracks)?;
        let mut report = AppendReport::default();
        for mut sample in parsed {
            sample.decode_timestamp_us = sample
                .decode_timestamp_us
                .checked_add(self.timestamp_offset_us)
                .ok_or_else(|| "SourceBuffer decode timestamp overflow".to_string())?;
            sample.presentation_timestamp_us = sample
                .presentation_timestamp_us
                .checked_add(self.timestamp_offset_us)
                .ok_or_else(|| "SourceBuffer presentation timestamp overflow".to_string())?;
            let end = sample
                .presentation_timestamp_us
                .checked_add(i64::try_from(sample.duration_us).unwrap_or(i64::MAX))
                .ok_or_else(|| "SourceBuffer sample end timestamp overflow".to_string())?;
            if !self
                .append_window
                .contains(sample.presentation_timestamp_us)
                || end > self.append_window.end_us
            {
                report.dropped_samples += 1;
                continue;
            }
            let projected = self
                .queued_bytes
                .checked_add(sample.data.len())
                .ok_or_else(|| "SourceBuffer byte count overflow".to_string())?;
            if projected > MAX_BYTES_PER_BUFFER || self.samples.len() >= MAX_SAMPLES_PER_BUFFER {
                return Err("SourceBuffer memory budget exceeded".to_string());
            }
            let before = self.samples.len();
            self.samples.retain(|existing| {
                if existing.track_id != sample.track_id {
                    return true;
                }
                let existing_end = existing
                    .presentation_timestamp_us
                    .saturating_add(existing.duration_us as i64);
                existing_end <= sample.presentation_timestamp_us
                    || existing.presentation_timestamp_us >= end
            });
            report.evicted_samples += before - self.samples.len();
            self.queued_bytes = self.samples.iter().map(|item| item.data.len()).sum();
            self.queued_bytes = self.queued_bytes.saturating_add(sample.data.len());
            self.samples.push(sample);
            report.appended_samples += 1;
        }
        self.samples
            .sort_by_key(|sample| (sample.presentation_timestamp_us, sample.track_id));
        report.buffered = self.buffered();
        Ok(report)
    }

    pub fn remove(&mut self, range: TimeRange) -> Result<usize, String> {
        if self.updating {
            return Err("Cannot remove while SourceBuffer is updating".to_string());
        }
        let before = self.samples.len();
        self.samples.retain(|sample| {
            let end = sample
                .presentation_timestamp_us
                .saturating_add(sample.duration_us as i64);
            end <= range.start_us || sample.presentation_timestamp_us >= range.end_us
        });
        self.queued_bytes = self.samples.iter().map(|item| item.data.len()).sum();
        Ok(before - self.samples.len())
    }

    pub fn evict_before(&mut self, timestamp_us: i64) -> usize {
        let before = self.samples.len();
        self.samples.retain(|sample| {
            sample
                .presentation_timestamp_us
                .saturating_add(sample.duration_us as i64)
                > timestamp_us
        });
        self.queued_bytes = self.samples.iter().map(|item| item.data.len()).sum();
        before - self.samples.len()
    }

    pub fn abort(&mut self) {
        self.updating = false;
        self.events.push(MediaSourceEvent::Abort);
    }

    pub fn buffered(&self) -> Vec<TimeRange> {
        let mut intervals = self
            .samples
            .iter()
            .filter_map(|sample| {
                let end = sample
                    .presentation_timestamp_us
                    .checked_add(i64::try_from(sample.duration_us).ok()?)?;
                TimeRange::new(sample.presentation_timestamp_us.max(0), end).ok()
            })
            .collect::<Vec<_>>();
        intervals.sort_by_key(|range| range.start_us);
        let mut merged: Vec<TimeRange> = Vec::new();
        for range in intervals {
            if let Some(last) = merged.last_mut() {
                if range.start_us <= last.end_us.saturating_add(RANGE_GAP_TOLERANCE_US) {
                    last.end_us = last.end_us.max(range.end_us);
                    continue;
                }
            }
            merged.push(range);
        }
        merged
    }

    pub fn drain_events(&mut self) -> Vec<MediaSourceEvent> {
        std::mem::take(&mut self.events)
    }
}

#[derive(Debug, Clone)]
pub struct MediaSource {
    ready_state: MediaSourceReadyState,
    duration_us: Option<i64>,
    source_buffers: Vec<SourceBuffer>,
    next_buffer_id: u32,
    events: Vec<MediaSourceEvent>,
}

impl Default for MediaSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaSource {
    pub fn new() -> Self {
        Self {
            ready_state: MediaSourceReadyState::Closed,
            duration_us: None,
            source_buffers: Vec::new(),
            next_buffer_id: 1,
            events: Vec::new(),
        }
    }

    pub fn open(&mut self) -> Result<(), String> {
        if self.ready_state != MediaSourceReadyState::Closed {
            return Err("MediaSource is already open".to_string());
        }
        self.ready_state = MediaSourceReadyState::Open;
        self.events.push(MediaSourceEvent::SourceOpen);
        Ok(())
    }

    pub fn ready_state(&self) -> MediaSourceReadyState {
        self.ready_state
    }

    pub fn duration_us(&self) -> Option<i64> {
        self.duration_us
    }

    pub fn set_duration(&mut self, duration_us: i64) -> Result<(), String> {
        self.require_open()?;
        if duration_us <= 0 || duration_us > MAX_MEDIA_TIME_US {
            return Err("MediaSource duration is outside the supported range".to_string());
        }
        self.duration_us = Some(duration_us);
        for buffer in &mut self.source_buffers {
            let _ = buffer.remove(TimeRange {
                start_us: duration_us,
                end_us: MAX_MEDIA_TIME_US,
            })?;
        }
        Ok(())
    }

    pub fn add_source_buffer(
        &mut self,
        content_type: &str,
        capabilities: &DecoderCapabilities,
    ) -> Result<u32, String> {
        self.require_open()?;
        if self.source_buffers.len() >= MAX_SOURCE_BUFFERS {
            return Err("MediaSource SourceBuffer budget exceeded".to_string());
        }
        let media_type = parse_media_type(content_type)?;
        if !is_type_supported(&media_type, capabilities) {
            return Err("MediaSource type or decoder is unsupported".to_string());
        }
        let id = self.next_buffer_id;
        self.next_buffer_id = self.next_buffer_id.saturating_add(1);
        self.source_buffers.push(SourceBuffer::new(id, media_type));
        Ok(id)
    }

    /// Merged buffered time ranges across every SourceBuffer, exposed to the
    /// page as `video.buffered` / `sourceBuffer.buffered` (Phase 17 player
    /// operations). Ranges are returned in microseconds, bounded by the
    /// media time cap of each buffer.
    pub fn buffered_ranges(&self) -> Vec<TimeRange> {
        let mut merged: Vec<TimeRange> = Vec::new();
        for buffer in &self.source_buffers {
            for range in buffer.buffered() {
                merged.push(range);
            }
        }
        merged.sort_by_key(|range| range.start_us);
        let mut combined: Vec<TimeRange> = Vec::new();
        for range in merged {
            if let Some(last) = combined.last_mut() {
                if range.start_us <= last.end_us {
                    last.end_us = last.end_us.max(range.end_us);
                    continue;
                }
            }
            combined.push(range);
        }
        combined
    }

    pub fn source_buffer(&self, id: u32) -> Option<&SourceBuffer> {
        self.source_buffers.iter().find(|buffer| buffer.id == id)
    }

    pub fn source_buffer_mut(&mut self, id: u32) -> Option<&mut SourceBuffer> {
        self.source_buffers
            .iter_mut()
            .find(|buffer| buffer.id == id)
    }

    pub fn append_buffer(&mut self, id: u32, bytes: &[u8]) -> Result<AppendReport, String> {
        self.require_open()?;
        let index = self
            .source_buffers
            .iter()
            .position(|buffer| buffer.id == id)
            .ok_or_else(|| "Unknown MediaSource SourceBuffer".to_string())?;
        let original = self.source_buffers[index].clone();
        let report = self.source_buffers[index].append_buffer(bytes)?;
        if self.total_queued_bytes() > MAX_TOTAL_MEDIA_BYTES {
            self.source_buffers[index] = original;
            return Err("MediaSource total byte budget exceeded".to_string());
        }
        Ok(report)
    }

    pub fn end_of_stream(&mut self) -> Result<(), String> {
        self.require_open()?;
        if self.source_buffers.iter().any(SourceBuffer::updating) {
            return Err("Cannot end MediaSource while a SourceBuffer is updating".to_string());
        }
        if self.source_buffers.is_empty()
            || self
                .source_buffers
                .iter()
                .all(|buffer| buffer.samples.is_empty())
        {
            return Err("Cannot end an empty MediaSource".to_string());
        }
        self.ready_state = MediaSourceReadyState::Ended;
        self.events.push(MediaSourceEvent::SourceEnded);
        Ok(())
    }

    pub fn close(&mut self) {
        self.ready_state = MediaSourceReadyState::Closed;
        self.source_buffers.clear();
        self.duration_us = None;
        self.events.push(MediaSourceEvent::SourceClose);
    }

    pub fn total_queued_bytes(&self) -> usize {
        self.source_buffers
            .iter()
            .map(SourceBuffer::queued_bytes)
            .sum()
    }

    pub fn buffered(&self) -> Vec<TimeRange> {
        let mut ranges = self
            .source_buffers
            .iter()
            .flat_map(SourceBuffer::buffered)
            .collect::<Vec<_>>();
        ranges.sort_by_key(|range| range.start_us);
        ranges
    }

    pub fn drain_events(&mut self) -> Vec<MediaSourceEvent> {
        std::mem::take(&mut self.events)
    }

    fn require_open(&self) -> Result<(), String> {
        if self.ready_state != MediaSourceReadyState::Open {
            return Err("MediaSource is not open".to_string());
        }
        Ok(())
    }
}

pub fn is_type_supported(media_type: &ParsedMediaType, capabilities: &DecoderCapabilities) -> bool {
    media_type.container == MediaContainer::IsoBmff
        && !media_type.codecs.is_empty()
        && media_type
            .codecs
            .iter()
            .all(|codec| capabilities.supports(codec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso_bmff::fixture;
    use crate::media_backend::{
        BrowserPcmBackend, CodecCapability, DecoderBackend, DecoderProvider,
    };
    use crate::media_core::MediaCodec;

    fn avc_capabilities() -> DecoderCapabilities {
        DecoderCapabilities {
            codecs: vec![CodecCapability {
                codec: MediaCodec::Avc,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            }],
            ..BrowserPcmBackend.capabilities()
        }
    }

    #[test]
    fn source_buffer_processes_fragmented_mp4_and_replaces_overlap() {
        let mut source = MediaSource::new();
        source.open().unwrap();
        let id = source
            .add_source_buffer("video/mp4; codecs=\"avc1.640028\"", &avc_capabilities())
            .unwrap();
        assert!(
            source
                .append_buffer(id, &fixture::init(1, 1_000, b"vide", b"avc1"))
                .unwrap()
                .initialization_segment
        );
        let segment = fixture::media(1, 0, 40, &[b"a", b"b"]);
        assert_eq!(
            source.append_buffer(id, &segment).unwrap().appended_samples,
            2
        );
        let replacement = fixture::media(1, 40, 40, &[b"c"]);
        let report = source.append_buffer(id, &replacement).unwrap();
        assert_eq!(report.evicted_samples, 1);
        assert_eq!(source.source_buffer(id).unwrap().samples().len(), 2);
        source.end_of_stream().unwrap();
        assert_eq!(source.ready_state(), MediaSourceReadyState::Ended);
    }

    #[test]
    fn source_buffer_append_window_and_remove_are_bounded() {
        let mut source = MediaSource::new();
        source.open().unwrap();
        let id = source
            .add_source_buffer("video/mp4; codecs=\"avc1\"", &avc_capabilities())
            .unwrap();
        source
            .append_buffer(id, &fixture::init(1, 1_000, b"vide", b"avc1"))
            .unwrap();
        source
            .source_buffer_mut(id)
            .unwrap()
            .set_append_window(TimeRange::new(40_000, 120_000).unwrap())
            .unwrap();
        let report = source
            .append_buffer(id, &fixture::media(1, 0, 40, &[b"a", b"b", b"c", b"d"]))
            .unwrap();
        assert_eq!(report.appended_samples, 2);
        assert_eq!(report.dropped_samples, 2);
        let removed = source
            .source_buffer_mut(id)
            .unwrap()
            .remove(TimeRange::new(40_000, 80_000).unwrap())
            .unwrap();
        assert_eq!(removed, 1);
    }

    #[test]
    fn failed_append_is_atomic_and_emits_abort_sequence() {
        let mut source = MediaSource::new();
        source.open().unwrap();
        let id = source
            .add_source_buffer("video/mp4; codecs=\"avc1\"", &avc_capabilities())
            .unwrap();
        source
            .append_buffer(id, &fixture::init(1, 1_000, b"vide", b"avc1"))
            .unwrap();
        source
            .append_buffer(id, &fixture::media(1, 0, 40, &[b"a"]))
            .unwrap();
        let buffer = source.source_buffer_mut(id).unwrap();
        let samples_before = buffer.samples().to_vec();
        let bytes_before = buffer.queued_bytes();
        let _ = buffer.drain_events();
        assert!(buffer.append_buffer(b"not-an-mp4-segment").is_err());
        assert_eq!(buffer.samples(), samples_before.as_slice());
        assert_eq!(buffer.queued_bytes(), bytes_before);
        assert_eq!(
            buffer.drain_events(),
            vec![
                MediaSourceEvent::UpdateStart,
                MediaSourceEvent::Abort,
                MediaSourceEvent::UpdateEnd,
            ]
        );
    }
}

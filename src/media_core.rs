//! Independent, bounded media-stack primitives.
//!
//! This module intentionally contains no platform decoder and no borrowed
//! browser implementation. It defines the validated contracts that future
//! ISO-BMFF/WebM demuxers and audited codec backends must satisfy.

use std::collections::{BTreeMap, VecDeque};

const MAX_CONTENT_TYPE_BYTES: usize = 1_024;
const MAX_SAMPLE_DURATION_US: u64 = 10 * 60 * 1_000_000;
const MAX_ABSOLUTE_TIMESTAMP_US: i64 = 7 * 24 * 60 * 60 * 1_000_000;
const MAX_WAVE_BYTES: usize = 128 * 1024 * 1024;
const PCM_FRAMES_PER_SAMPLE: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaContainer {
    IsoBmff,
    WebM,
    Ogg,
    Wave,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaCodec {
    Avc,
    Hevc,
    Vp8,
    Vp9,
    Av1,
    Aac,
    Opus,
    Vorbis,
    Pcm,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMediaType {
    pub essence: String,
    pub container: MediaContainer,
    pub codecs: Vec<MediaCodec>,
}

pub fn parse_media_type(value: &str) -> Result<ParsedMediaType, String> {
    if value.is_empty() || value.len() > MAX_CONTENT_TYPE_BYTES || !value.is_ascii() {
        return Err("Invalid media Content-Type".to_string());
    }
    let mut parts = value.split(';');
    let essence = parts
        .next()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .filter(|value| value.contains('/'))
        .ok_or_else(|| "Media Content-Type has no valid essence".to_string())?;
    let container = match essence.as_str() {
        "video/mp4" | "audio/mp4" | "application/mp4" => MediaContainer::IsoBmff,
        "video/webm" | "audio/webm" => MediaContainer::WebM,
        "video/ogg" | "audio/ogg" | "application/ogg" => MediaContainer::Ogg,
        "audio/wav" | "audio/wave" | "audio/x-wav" => MediaContainer::Wave,
        _ => MediaContainer::Unknown,
    };

    let mut codecs = Vec::new();
    for parameter in parts {
        let Some((name, raw_value)) = parameter.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("codecs") {
            continue;
        }
        let codec_list = raw_value.trim().trim_matches('"');
        for codec in codec_list
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if codecs.len() >= 16 {
                return Err("Media codec list budget exceeded".to_string());
            }
            codecs.push(parse_codec(codec));
        }
    }
    Ok(ParsedMediaType {
        essence,
        container,
        codecs,
    })
}

fn parse_codec(codec: &str) -> MediaCodec {
    let normalized = codec.to_ascii_lowercase();
    let prefix = normalized.split('.').next().unwrap_or_default();
    match prefix {
        "avc1" | "avc2" => MediaCodec::Avc,
        "hev1" | "hvc1" => MediaCodec::Hevc,
        "vp8" | "vp08" => MediaCodec::Vp8,
        "vp9" | "vp09" => MediaCodec::Vp9,
        "av01" => MediaCodec::Av1,
        "mp4a" => MediaCodec::Aac,
        "opus" => MediaCodec::Opus,
        "vorbis" => MediaCodec::Vorbis,
        "pcm" | "lpcm" => MediaCodec::Pcm,
        _ => MediaCodec::Unknown(normalized),
    }
}

pub fn sniff_media_container(prefix: &[u8]) -> MediaContainer {
    if prefix.len() >= 12 && &prefix[4..8] == b"ftyp" {
        return MediaContainer::IsoBmff;
    }
    if prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return MediaContainer::WebM;
    }
    if prefix.starts_with(b"OggS") {
        return MediaContainer::Ogg;
    }
    if prefix.len() >= 12 && prefix.starts_with(b"RIFF") && &prefix[8..12] == b"WAVE" {
        return MediaContainer::Wave;
    }
    MediaContainer::Unknown
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end_inclusive: u64,
}

impl ByteRange {
    pub fn new(start: u64, end_inclusive: u64) -> Result<Self, String> {
        if start > end_inclusive {
            return Err("Invalid media byte range".to_string());
        }
        Ok(Self {
            start,
            end_inclusive,
        })
    }

    pub fn len(self) -> Result<usize, String> {
        usize::try_from(
            self.end_inclusive
                .checked_sub(self.start)
                .and_then(|length| length.checked_add(1))
                .ok_or_else(|| "Media byte range overflow".to_string())?,
        )
        .map_err(|_| "Media byte range exceeds address space".to_string())
    }

    pub fn is_empty(self) -> bool {
        false
    }
}

pub fn bounded_range_slice(
    bytes: &[u8],
    range: ByteRange,
    max_bytes: usize,
) -> Result<&[u8], String> {
    let length = range.len()?;
    if length > max_bytes {
        return Err("Media byte range budget exceeded".to_string());
    }
    let start = usize::try_from(range.start)
        .map_err(|_| "Media byte range exceeds address space".to_string())?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| "Media byte range overflow".to_string())?;
    bytes
        .get(start..end)
        .ok_or_else(|| "Media byte range is outside the resource".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSample {
    pub track_id: u32,
    pub decode_timestamp_us: i64,
    pub presentation_timestamp_us: i64,
    pub duration_us: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

pub trait MediaDemuxer {
    fn container(&self) -> MediaContainer;
    fn push_bytes(
        &mut self,
        bytes: &[u8],
        end_of_stream: bool,
    ) -> Result<Vec<EncodedSample>, String>;
}

pub trait MediaDecoder {
    type Output;

    fn codec(&self) -> MediaCodec;
    fn decode(&mut self, sample: EncodedSample) -> Result<Vec<Self::Output>, String>;
    fn flush(&mut self) -> Result<Vec<Self::Output>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl PcmFormat {
    fn validate(self) -> Result<Self, String> {
        if !(8_000..=384_000).contains(&self.sample_rate_hz) {
            return Err("Unsupported PCM sample rate".to_string());
        }
        if !(1..=8).contains(&self.channels) || self.bits_per_sample != 16 {
            return Err("Only bounded 16-bit PCM audio is supported".to_string());
        }
        Ok(self)
    }

    pub fn block_align(self) -> usize {
        usize::from(self.channels) * usize::from(self.bits_per_sample / 8)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedAudioFrame {
    pub timestamp_us: i64,
    pub duration_us: u64,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub interleaved_samples: Vec<i16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedVideoFrame {
    pub timestamp_us: i64,
    pub duration_us: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedFrame {
    Audio(DecodedAudioFrame),
    Video(DecodedVideoFrame),
}

impl DecodedFrame {
    pub fn timestamp_us(&self) -> i64 {
        match self {
            Self::Audio(frame) => frame.timestamp_us,
            Self::Video(frame) => frame.timestamp_us,
        }
    }

    pub fn estimated_bytes(&self) -> usize {
        match self {
            Self::Audio(frame) => frame
                .interleaved_samples
                .len()
                .saturating_mul(std::mem::size_of::<i16>()),
            Self::Video(frame) => frame.rgba.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BoundedFrameQueue {
    max_frames: usize,
    max_bytes: usize,
    queued_bytes: usize,
    frames: VecDeque<DecodedFrame>,
}

impl BoundedFrameQueue {
    pub fn new(max_frames: usize, max_bytes: usize) -> Self {
        Self {
            max_frames,
            max_bytes,
            queued_bytes: 0,
            frames: VecDeque::new(),
        }
    }

    pub fn push(&mut self, frame: DecodedFrame) -> Result<(), String> {
        if self.frames.len() >= self.max_frames {
            return Err("Decoded frame count budget exceeded".to_string());
        }
        let bytes = frame.estimated_bytes();
        let projected = self
            .queued_bytes
            .checked_add(bytes)
            .ok_or_else(|| "Decoded frame byte count overflow".to_string())?;
        if projected > self.max_bytes {
            return Err("Decoded frame byte budget exceeded".to_string());
        }
        if self
            .frames
            .back()
            .is_some_and(|last| frame.timestamp_us() < last.timestamp_us())
        {
            return Err("Non-monotonic decoded frame timestamp".to_string());
        }
        self.frames.push_back(frame);
        self.queued_bytes = projected;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<DecodedFrame> {
        let frame = self.frames.pop_front()?;
        self.queued_bytes = self.queued_bytes.saturating_sub(frame.estimated_bytes());
        Some(frame)
    }

    pub fn front(&self) -> Option<&DecodedFrame> {
        self.frames.front()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.queued_bytes = 0;
    }
}

/// A bounded RIFF/WAVE PCM demuxer. This is a real clear-content path used by
/// the Phase 15 gate and does not depend on a platform codec.
#[derive(Debug, Default)]
pub struct WavePcmDemuxer {
    buffered: Vec<u8>,
    format: Option<PcmFormat>,
    emitted: bool,
}

impl WavePcmDemuxer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn format(&self) -> Option<PcmFormat> {
        self.format
    }
}

impl MediaDemuxer for WavePcmDemuxer {
    fn container(&self) -> MediaContainer {
        MediaContainer::Wave
    }

    fn push_bytes(
        &mut self,
        bytes: &[u8],
        end_of_stream: bool,
    ) -> Result<Vec<EncodedSample>, String> {
        if self.emitted {
            return Err("WAVE stream already reached end of stream".to_string());
        }
        let projected = self
            .buffered
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| "WAVE buffer size overflow".to_string())?;
        if projected > MAX_WAVE_BYTES {
            return Err("WAVE buffer budget exceeded".to_string());
        }
        self.buffered.extend_from_slice(bytes);
        if !end_of_stream {
            return Ok(Vec::new());
        }
        let (format, data) = parse_wave_pcm(&self.buffered)?;
        self.format = Some(format);
        self.emitted = true;
        pcm_bytes_to_samples(format, data)
    }
}

#[derive(Debug, Clone)]
pub struct Pcm16Decoder {
    format: PcmFormat,
}

impl Pcm16Decoder {
    pub fn new(format: PcmFormat) -> Result<Self, String> {
        Ok(Self {
            format: format.validate()?,
        })
    }
}

impl MediaDecoder for Pcm16Decoder {
    type Output = DecodedAudioFrame;

    fn codec(&self) -> MediaCodec {
        MediaCodec::Pcm
    }

    fn decode(&mut self, sample: EncodedSample) -> Result<Vec<Self::Output>, String> {
        validate_sample(&sample)?;
        let block_align = self.format.block_align();
        if !sample.data.len().is_multiple_of(block_align) || !sample.data.len().is_multiple_of(2) {
            return Err("PCM sample is not aligned to complete frames".to_string());
        }
        let interleaved_samples = sample
            .data
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
            .collect();
        Ok(vec![DecodedAudioFrame {
            timestamp_us: sample.presentation_timestamp_us,
            duration_us: sample.duration_us,
            sample_rate_hz: self.format.sample_rate_hz,
            channels: self.format.channels,
            interleaved_samples,
        }])
    }

    fn flush(&mut self) -> Result<Vec<Self::Output>, String> {
        Ok(Vec::new())
    }
}

fn parse_wave_pcm(bytes: &[u8]) -> Result<(PcmFormat, &[u8]), String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Invalid RIFF/WAVE header".to_string());
    }
    let declared = read_le_u32(bytes, 4)? as usize;
    if declared.saturating_add(8) > bytes.len() {
        return Err("Truncated RIFF/WAVE resource".to_string());
    }
    let mut offset = 12usize;
    let mut format = None;
    let mut data = None;
    let mut chunks = 0usize;
    while offset.saturating_add(8) <= bytes.len() {
        chunks += 1;
        if chunks > 1_024 {
            return Err("WAVE chunk count budget exceeded".to_string());
        }
        let id = &bytes[offset..offset + 4];
        let length = read_le_u32(bytes, offset + 4)? as usize;
        let start = offset
            .checked_add(8)
            .ok_or_else(|| "WAVE chunk offset overflow".to_string())?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| "WAVE chunk size overflow".to_string())?;
        let chunk = bytes
            .get(start..end)
            .ok_or_else(|| "Truncated WAVE chunk".to_string())?;
        if id == b"fmt " {
            if chunk.len() < 16 || read_le_u16(chunk, 0)? != 1 {
                return Err("Only uncompressed PCM WAVE is supported".to_string());
            }
            let parsed = PcmFormat {
                channels: read_le_u16(chunk, 2)?,
                sample_rate_hz: read_le_u32(chunk, 4)?,
                bits_per_sample: read_le_u16(chunk, 14)?,
            }
            .validate()?;
            let declared_align = usize::from(read_le_u16(chunk, 12)?);
            if declared_align != parsed.block_align() {
                return Err("Invalid WAVE block alignment".to_string());
            }
            format = Some(parsed);
        } else if id == b"data" {
            data = Some(chunk);
        }
        offset = end.saturating_add(length % 2);
    }
    let format = format.ok_or_else(|| "WAVE format chunk is missing".to_string())?;
    let data = data.ok_or_else(|| "WAVE data chunk is missing".to_string())?;
    if data.is_empty() || data.len() % format.block_align() != 0 {
        return Err("WAVE data is empty or misaligned".to_string());
    }
    Ok((format, data))
}

fn pcm_bytes_to_samples(format: PcmFormat, data: &[u8]) -> Result<Vec<EncodedSample>, String> {
    let bytes_per_frame = format.block_align();
    let bytes_per_sample = bytes_per_frame.saturating_mul(PCM_FRAMES_PER_SAMPLE);
    let mut samples = Vec::new();
    let mut frame_offset = 0u64;
    for chunk in data.chunks(bytes_per_sample.max(bytes_per_frame)) {
        let frames = chunk.len() / bytes_per_frame;
        let timestamp_us = frame_offset
            .saturating_mul(1_000_000)
            .checked_div(u64::from(format.sample_rate_hz))
            .unwrap_or_default();
        let duration_us = (frames as u64)
            .saturating_mul(1_000_000)
            .checked_div(u64::from(format.sample_rate_hz))
            .unwrap_or_default()
            .max(1);
        samples.push(EncodedSample {
            track_id: 1,
            decode_timestamp_us: i64::try_from(timestamp_us).unwrap_or(i64::MAX),
            presentation_timestamp_us: i64::try_from(timestamp_us).unwrap_or(i64::MAX),
            duration_us,
            keyframe: true,
            data: chunk.to_vec(),
        });
        frame_offset = frame_offset.saturating_add(frames as u64);
    }
    Ok(samples)
}

fn read_le_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| "Truncated little-endian u16".to_string())?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| "Truncated little-endian u32".to_string())?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[derive(Debug)]
pub struct BoundedSampleQueue {
    max_samples: usize,
    max_bytes: usize,
    queued_bytes: usize,
    samples: VecDeque<EncodedSample>,
    last_decode_timestamp: BTreeMap<u32, i64>,
}

impl BoundedSampleQueue {
    pub fn new(max_samples: usize, max_bytes: usize) -> Self {
        Self {
            max_samples,
            max_bytes,
            queued_bytes: 0,
            samples: VecDeque::new(),
            last_decode_timestamp: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, sample: EncodedSample) -> Result<(), String> {
        validate_sample(&sample)?;
        if self.samples.len() >= self.max_samples {
            return Err("Encoded sample count budget exceeded".to_string());
        }
        let projected = self
            .queued_bytes
            .checked_add(sample.data.len())
            .ok_or_else(|| "Encoded sample byte count overflow".to_string())?;
        if projected > self.max_bytes {
            return Err("Encoded sample byte budget exceeded".to_string());
        }
        if self
            .last_decode_timestamp
            .get(&sample.track_id)
            .is_some_and(|last| sample.decode_timestamp_us < *last)
        {
            return Err("Non-monotonic media decode timestamp".to_string());
        }
        self.last_decode_timestamp
            .insert(sample.track_id, sample.decode_timestamp_us);
        self.queued_bytes = projected;
        self.samples.push_back(sample);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<EncodedSample> {
        let sample = self.samples.pop_front()?;
        self.queued_bytes = self.queued_bytes.saturating_sub(sample.data.len());
        Some(sample)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.last_decode_timestamp.clear();
        self.queued_bytes = 0;
    }
}

fn validate_sample(sample: &EncodedSample) -> Result<(), String> {
    if sample.track_id == 0 || sample.data.is_empty() {
        return Err("Invalid encoded media sample".to_string());
    }
    if sample.duration_us == 0 || sample.duration_us > MAX_SAMPLE_DURATION_US {
        return Err("Invalid encoded sample duration".to_string());
    }
    if sample.decode_timestamp_us.unsigned_abs() > MAX_ABSOLUTE_TIMESTAMP_US as u64
        || sample.presentation_timestamp_us.unsigned_abs() > MAX_ABSOLUTE_TIMESTAMP_US as u64
    {
        return Err("Encoded sample timestamp budget exceeded".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSyncAction {
    Present,
    Hold,
    Drop,
}

pub fn video_sync_action(
    audio_position_us: i64,
    video_timestamp_us: i64,
    tolerance_us: u64,
) -> VideoSyncAction {
    let tolerance = i64::try_from(tolerance_us).unwrap_or(i64::MAX);
    let delta = video_timestamp_us.saturating_sub(audio_position_us);
    if delta > tolerance {
        VideoSyncAction::Hold
    } else if delta < -tolerance {
        VideoSyncAction::Drop
    } else {
        VideoSyncAction::Present
    }
}

#[derive(Debug, Clone)]
pub struct AudioClock {
    sample_rate_hz: u32,
    anchor_us: i64,
    frames_played: u64,
    running: bool,
}

impl AudioClock {
    pub fn new(sample_rate_hz: u32) -> Result<Self, String> {
        if !(8_000..=384_000).contains(&sample_rate_hz) {
            return Err("Unsupported audio sample rate".to_string());
        }
        Ok(Self {
            sample_rate_hz,
            anchor_us: 0,
            frames_played: 0,
            running: false,
        })
    }

    pub fn start(&mut self) {
        self.running = true;
    }

    pub fn pause(&mut self) {
        self.running = false;
    }

    pub fn seek(&mut self, timestamp_us: i64) -> Result<(), String> {
        if timestamp_us.unsigned_abs() > MAX_ABSOLUTE_TIMESTAMP_US as u64 {
            return Err("Audio clock timestamp budget exceeded".to_string());
        }
        self.anchor_us = timestamp_us;
        self.frames_played = 0;
        Ok(())
    }

    pub fn advance_frames(&mut self, frames: u64) -> Result<(), String> {
        if self.running {
            self.frames_played = self
                .frames_played
                .checked_add(frames)
                .ok_or_else(|| "Audio clock frame counter overflow".to_string())?;
        }
        Ok(())
    }

    pub fn position_us(&self) -> i64 {
        let elapsed = self
            .frames_played
            .saturating_mul(1_000_000)
            .checked_div(u64::from(self.sample_rate_hz))
            .unwrap_or_default();
        self.anchor_us
            .saturating_add(i64::try_from(elapsed).unwrap_or(i64::MAX))
    }

    pub fn is_running(&self) -> bool {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(track_id: u32, dts: i64, bytes: usize) -> EncodedSample {
        EncodedSample {
            track_id,
            decode_timestamp_us: dts,
            presentation_timestamp_us: dts,
            duration_us: 20_000,
            keyframe: dts == 0,
            data: vec![0; bytes],
        }
    }

    #[test]
    fn media_type_parses_container_and_codec_parameters() {
        let parsed = parse_media_type("video/mp4; codecs=\"avc1.640028, mp4a.40.2\"").unwrap();
        assert_eq!(parsed.essence, "video/mp4");
        assert_eq!(parsed.container, MediaContainer::IsoBmff);
        assert_eq!(parsed.codecs, vec![MediaCodec::Avc, MediaCodec::Aac]);
    }

    #[test]
    fn sniffing_uses_bounded_magic_bytes() {
        assert_eq!(
            sniff_media_container(b"\0\0\0\x18ftypisom"),
            MediaContainer::IsoBmff
        );
        assert_eq!(
            sniff_media_container(&[0x1a, 0x45, 0xdf, 0xa3]),
            MediaContainer::WebM
        );
        assert_eq!(sniff_media_container(b"OggS"), MediaContainer::Ogg);
        assert_eq!(
            sniff_media_container(b"RIFF\0\0\0\0WAVE"),
            MediaContainer::Wave
        );
    }

    #[test]
    fn byte_ranges_reject_overflow_out_of_bounds_and_excess_size() {
        let bytes = b"0123456789";
        let range = ByteRange::new(2, 5).unwrap();
        assert_eq!(bounded_range_slice(bytes, range, 4).unwrap(), b"2345");
        assert!(bounded_range_slice(bytes, range, 3).is_err());
        assert!(bounded_range_slice(bytes, ByteRange::new(8, 12).unwrap(), 8).is_err());
        assert!(ByteRange::new(5, 4).is_err());
    }

    #[test]
    fn sample_queue_enforces_order_count_and_byte_budgets() {
        let mut queue = BoundedSampleQueue::new(2, 6);
        queue.push(sample(1, 0, 3)).unwrap();
        queue.push(sample(1, 20_000, 3)).unwrap();
        assert_eq!(queue.queued_bytes(), 6);
        assert!(queue.push(sample(1, 40_000, 1)).is_err());
        assert_eq!(queue.pop().unwrap().decode_timestamp_us, 0);
        assert!(queue.push(sample(1, 10_000, 1)).is_err());
        queue.clear();
        queue.push(sample(1, 10_000, 6)).unwrap();
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn extreme_timestamps_fail_without_panicking() {
        let mut queue = BoundedSampleQueue::new(1, 4);
        assert!(queue.push(sample(1, i64::MIN, 1)).is_err());
        let mut clock = AudioClock::new(48_000).unwrap();
        assert!(clock.seek(i64::MIN).is_err());
    }

    #[test]
    fn audio_clock_drives_video_sync_decisions() {
        let mut clock = AudioClock::new(48_000).unwrap();
        clock.start();
        clock.advance_frames(48_000).unwrap();
        assert_eq!(clock.position_us(), 1_000_000);
        assert_eq!(
            video_sync_action(clock.position_us(), 1_010_000, 20_000),
            VideoSyncAction::Present
        );
        assert_eq!(
            video_sync_action(clock.position_us(), 1_100_000, 20_000),
            VideoSyncAction::Hold
        );
        assert_eq!(
            video_sync_action(clock.position_us(), 900_000, 20_000),
            VideoSyncAction::Drop
        );
        clock.pause();
        clock.advance_frames(48_000).unwrap();
        assert_eq!(clock.position_us(), 1_000_000);
    }

    fn pcm_wave(frames: usize) -> Vec<u8> {
        let channels = 2u16;
        let sample_rate = 48_000u32;
        let bits = 16u16;
        let block_align = channels * bits / 8;
        let byte_rate = sample_rate * u32::from(block_align);
        let mut data = Vec::with_capacity(frames * usize::from(block_align));
        for frame in 0..frames {
            let value = ((frame as i32 % 200) - 100) as i16;
            data.extend_from_slice(&value.to_le_bytes());
            data.extend_from_slice(&(-value).to_le_bytes());
        }
        let riff_size = 36u32 + data.len() as u32;
        let mut wave = Vec::new();
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&riff_size.to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16u32.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes());
        wave.extend_from_slice(&channels.to_le_bytes());
        wave.extend_from_slice(&sample_rate.to_le_bytes());
        wave.extend_from_slice(&byte_rate.to_le_bytes());
        wave.extend_from_slice(&block_align.to_le_bytes());
        wave.extend_from_slice(&bits.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wave.extend_from_slice(&data);
        wave
    }

    #[test]
    fn wave_pcm_fixture_is_demuxed_decoded_and_memory_bounded() {
        let wave = pcm_wave(2_400);
        let split = wave.len() / 2;
        let mut demuxer = WavePcmDemuxer::new();
        assert!(demuxer
            .push_bytes(&wave[..split], false)
            .unwrap()
            .is_empty());
        let samples = demuxer.push_bytes(&wave[split..], true).unwrap();
        let format = demuxer.format().unwrap();
        assert_eq!(format.sample_rate_hz, 48_000);
        assert_eq!(format.channels, 2);
        assert_eq!(samples.len(), 3);

        let mut decoder = Pcm16Decoder::new(format).unwrap();
        let mut queue = BoundedFrameQueue::new(8, 32 * 1024);
        for sample in samples {
            for frame in decoder.decode(sample).unwrap() {
                queue.push(DecodedFrame::Audio(frame)).unwrap();
            }
        }
        assert_eq!(queue.len(), 3);
        assert!(queue.queued_bytes() <= 32 * 1024);
        let mut previous = -1;
        while let Some(frame) = queue.pop() {
            assert!(frame.timestamp_us() > previous);
            previous = frame.timestamp_us();
        }
        assert_eq!(queue.queued_bytes(), 0);
    }

    #[test]
    fn wave_parser_rejects_compressed_and_truncated_resources() {
        let mut wave = pcm_wave(32);
        wave[20] = 3;
        let mut demuxer = WavePcmDemuxer::new();
        assert!(demuxer.push_bytes(&wave, true).is_err());

        let wave = pcm_wave(32);
        let mut demuxer = WavePcmDemuxer::new();
        assert!(demuxer.push_bytes(&wave[..wave.len() - 3], true).is_err());
    }
}

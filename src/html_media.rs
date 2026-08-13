//! Bounded HTML audio/video element state machine.
//!
//! The state machine is independent of the UI toolkit and decoder backend. It
//! owns playback controls, event ordering, seek, rate, underflow recovery and
//! the MediaSource attachment used by application code.

use std::collections::VecDeque;

use crate::mse::{MediaSource, MediaSourceReadyState, TimeRange};

const MAX_MEDIA_EVENTS: usize = 512;
const MAX_PLAYBACK_RATE: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadyState {
    HaveNothing,
    HaveMetadata,
    HaveCurrentData,
    HaveFutureData,
    HaveEnoughData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    Empty,
    Idle,
    Loading,
    NoSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaEvent {
    LoadStart,
    DurationChange,
    LoadedMetadata,
    LoadedData,
    CanPlay,
    CanPlayThrough,
    Play,
    Playing,
    Pause,
    Seeking,
    Seeked,
    TimeUpdate,
    Progress,
    Waiting,
    Stalled,
    RateChange,
    VolumeChange,
    Ended,
    Error,
    Emptied,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaControlAction {
    TogglePlayback,
    SeekTo(f64),
    SeekBy(f64),
    SetVolume(f64),
    ToggleMute,
    SetPlaybackRate(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaControlsState {
    pub paused: bool,
    pub muted: bool,
    pub seeking: bool,
    pub ended: bool,
    pub current_time_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub volume: f64,
    pub playback_rate: f64,
    pub buffered: Vec<TimeRange>,
}

#[derive(Debug, Clone)]
pub struct HtmlMediaElement {
    network_state: NetworkState,
    ready_state: ReadyState,
    paused: bool,
    seeking: bool,
    ended: bool,
    current_time_us: i64,
    duration_us: Option<i64>,
    volume: f64,
    muted: bool,
    playback_rate: f64,
    loop_enabled: bool,
    autoplay: bool,
    source_url: Option<String>,
    media_source: Option<MediaSource>,
    direct_buffered_range: Option<TimeRange>,
    events: VecDeque<MediaEvent>,
    error: Option<String>,
}

impl Default for HtmlMediaElement {
    fn default() -> Self {
        Self::new()
    }
}

impl HtmlMediaElement {
    pub fn new() -> Self {
        Self {
            network_state: NetworkState::Empty,
            ready_state: ReadyState::HaveNothing,
            paused: true,
            seeking: false,
            ended: false,
            current_time_us: 0,
            duration_us: None,
            volume: 1.0,
            muted: false,
            playback_rate: 1.0,
            loop_enabled: false,
            autoplay: false,
            source_url: None,
            media_source: None,
            direct_buffered_range: None,
            events: VecDeque::new(),
            error: None,
        }
    }

    pub fn attach_media_source(&mut self, mut source: MediaSource) -> Result<(), String> {
        self.reset_resource();
        if source.ready_state() == MediaSourceReadyState::Closed {
            source.open()?;
        }
        self.media_source = Some(source);
        self.network_state = NetworkState::Loading;
        self.push_event(MediaEvent::LoadStart);
        self.synchronize_source_state();
        Ok(())
    }

    pub fn load_url(&mut self, url: &str) -> Result<(), String> {
        let parsed = url::Url::parse(url).map_err(|_| "Invalid media URL".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https" | "file") {
            return Err("Unsupported media URL scheme".to_string());
        }
        self.reset_resource();
        self.source_url = Some(parsed.into());
        self.network_state = NetworkState::Loading;
        self.push_event(MediaEvent::LoadStart);
        Ok(())
    }

    /// Attach output that has already been demuxed and decoded by the media
    /// backend. This keeps direct-file playback on the same HTML state machine
    /// as MediaSource playback instead of bypassing play/seek/readiness.
    pub fn attach_decoded_stream(&mut self, duration_us: i64) -> Result<(), String> {
        if duration_us <= 0 {
            return Err("Decoded media duration must be positive".to_string());
        }
        self.reset_resource();
        self.duration_us = Some(duration_us);
        self.direct_buffered_range = Some(TimeRange {
            start_us: 0,
            end_us: duration_us,
        });
        self.network_state = NetworkState::Idle;
        self.ready_state = ReadyState::HaveEnoughData;
        self.push_event(MediaEvent::LoadStart);
        self.push_event(MediaEvent::DurationChange);
        self.push_event(MediaEvent::LoadedMetadata);
        self.push_event(MediaEvent::LoadedData);
        self.push_event(MediaEvent::CanPlay);
        self.push_event(MediaEvent::CanPlayThrough);
        self.push_event(MediaEvent::Progress);
        Ok(())
    }

    pub fn media_source(&self) -> Option<&MediaSource> {
        self.media_source.as_ref()
    }

    pub fn media_source_mut(&mut self) -> Option<&mut MediaSource> {
        self.media_source.as_mut()
    }

    pub fn synchronize_source_state(&mut self) {
        let Some(source) = self.media_source.as_ref() else {
            return;
        };
        let source_duration = source
            .duration_us()
            .or_else(|| source.buffered().iter().map(|range| range.end_us).max());
        let source_ready_state = source.ready_state();
        let old_duration = self.duration_us;
        self.duration_us = source_duration;
        if self.duration_us != old_duration && self.duration_us.is_some() {
            self.push_event(MediaEvent::DurationChange);
        }
        if self.duration_us.is_some() && self.ready_state == ReadyState::HaveNothing {
            self.ready_state = ReadyState::HaveMetadata;
            self.push_event(MediaEvent::LoadedMetadata);
        }
        let ahead = self.buffered_ahead_us();
        let next_ready = if ahead >= 5_000_000 {
            ReadyState::HaveEnoughData
        } else if ahead > 250_000 {
            ReadyState::HaveFutureData
        } else if ahead > 0 {
            ReadyState::HaveCurrentData
        } else if self.duration_us.is_some() {
            ReadyState::HaveMetadata
        } else {
            ReadyState::HaveNothing
        };
        if next_ready > self.ready_state {
            if self.ready_state < ReadyState::HaveCurrentData
                && next_ready >= ReadyState::HaveCurrentData
            {
                self.push_event(MediaEvent::LoadedData);
            }
            if self.ready_state < ReadyState::HaveFutureData
                && next_ready >= ReadyState::HaveFutureData
            {
                self.push_event(MediaEvent::CanPlay);
            }
            if next_ready == ReadyState::HaveEnoughData {
                self.push_event(MediaEvent::CanPlayThrough);
            }
        }
        self.ready_state = next_ready;
        self.network_state = if source_ready_state == MediaSourceReadyState::Ended {
            NetworkState::Idle
        } else {
            NetworkState::Loading
        };
        self.push_event(MediaEvent::Progress);
        if self.autoplay && self.paused && self.ready_state >= ReadyState::HaveFutureData {
            let _ = self.play();
        }
    }

    pub fn play(&mut self) -> Result<(), String> {
        if self.error.is_some() || self.ready_state < ReadyState::HaveCurrentData {
            return Err("Media does not have playable data".to_string());
        }
        if self.ended {
            self.current_time_us = 0;
            self.ended = false;
        }
        if self.paused {
            self.paused = false;
            self.push_event(MediaEvent::Play);
            self.push_event(MediaEvent::Playing);
        }
        Ok(())
    }

    pub fn pause(&mut self) {
        if !self.paused {
            self.paused = true;
            self.push_event(MediaEvent::Pause);
        }
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err("Invalid media seek target".to_string());
        }
        let target = (seconds * 1_000_000.0) as i64;
        let duration = self
            .duration_us
            .ok_or_else(|| "Media duration is not available".to_string())?;
        self.seeking = true;
        self.push_event(MediaEvent::Seeking);
        self.current_time_us = target.min(duration);
        self.ended = false;
        self.synchronize_source_state();
        self.seeking = false;
        self.push_event(MediaEvent::TimeUpdate);
        self.push_event(MediaEvent::Seeked);
        Ok(())
    }

    pub fn set_volume(&mut self, volume: f64) -> Result<(), String> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err("Media volume must be between zero and one".to_string());
        }
        if self.volume != volume {
            self.volume = volume;
            self.push_event(MediaEvent::VolumeChange);
        }
        Ok(())
    }

    pub fn set_muted(&mut self, muted: bool) {
        if self.muted != muted {
            self.muted = muted;
            self.push_event(MediaEvent::VolumeChange);
        }
    }

    pub fn set_playback_rate(&mut self, rate: f64) -> Result<(), String> {
        if !rate.is_finite() || !(0.25..=MAX_PLAYBACK_RATE).contains(&rate) {
            return Err("Unsupported media playback rate".to_string());
        }
        if self.playback_rate != rate {
            self.playback_rate = rate;
            self.push_event(MediaEvent::RateChange);
        }
        Ok(())
    }

    pub fn set_loop(&mut self, enabled: bool) {
        self.loop_enabled = enabled;
    }

    pub fn set_autoplay(&mut self, enabled: bool) {
        self.autoplay = enabled;
    }

    pub fn tick(&mut self, elapsed_ms: u64) {
        if self.paused || self.seeking || elapsed_ms == 0 {
            return;
        }
        self.synchronize_source_state();
        if self.buffered_ahead_us() == 0 {
            self.ready_state = self
                .duration_us
                .map(|_| ReadyState::HaveMetadata)
                .unwrap_or(ReadyState::HaveNothing);
            self.push_event(MediaEvent::Waiting);
            return;
        }
        let advance = (elapsed_ms as f64 * 1_000.0 * self.playback_rate) as i64;
        self.current_time_us = self.current_time_us.saturating_add(advance);
        if let Some(duration) = self.duration_us {
            if self.current_time_us >= duration {
                if self.loop_enabled {
                    self.current_time_us = 0;
                    self.push_event(MediaEvent::TimeUpdate);
                    return;
                }
                self.current_time_us = duration;
                self.ended = true;
                self.paused = true;
                self.push_event(MediaEvent::TimeUpdate);
                self.push_event(MediaEvent::Ended);
                return;
            }
        }
        self.push_event(MediaEvent::TimeUpdate);
    }

    pub fn apply_control(&mut self, action: MediaControlAction) -> Result<(), String> {
        match action {
            MediaControlAction::TogglePlayback => {
                if self.paused {
                    self.play()
                } else {
                    self.pause();
                    Ok(())
                }
            }
            MediaControlAction::SeekTo(seconds) => self.seek(seconds),
            MediaControlAction::SeekBy(seconds) => {
                self.seek((self.current_time_seconds() + seconds).max(0.0))
            }
            MediaControlAction::SetVolume(volume) => self.set_volume(volume),
            MediaControlAction::ToggleMute => {
                self.set_muted(!self.muted);
                Ok(())
            }
            MediaControlAction::SetPlaybackRate(rate) => self.set_playback_rate(rate),
        }
    }

    pub fn controls_state(&self) -> MediaControlsState {
        MediaControlsState {
            paused: self.paused,
            muted: self.muted,
            seeking: self.seeking,
            ended: self.ended,
            current_time_seconds: self.current_time_seconds(),
            duration_seconds: self.duration_us.map(|value| value as f64 / 1_000_000.0),
            volume: self.volume,
            playback_rate: self.playback_rate,
            buffered: self.buffered(),
        }
    }

    pub fn network_state(&self) -> NetworkState {
        self.network_state
    }

    pub fn ready_state(&self) -> ReadyState {
        self.ready_state
    }

    pub fn current_time_seconds(&self) -> f64 {
        self.current_time_us as f64 / 1_000_000.0
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn fail(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.paused = true;
        self.network_state = NetworkState::NoSource;
        self.push_event(MediaEvent::Error);
    }

    pub fn buffered(&self) -> Vec<TimeRange> {
        self.media_source
            .as_ref()
            .map(MediaSource::buffered)
            .or_else(|| self.direct_buffered_range.map(|range| vec![range]))
            .unwrap_or_default()
    }

    pub fn drain_events(&mut self) -> Vec<MediaEvent> {
        self.events.drain(..).collect()
    }

    fn buffered_ahead_us(&self) -> i64 {
        self.buffered()
            .into_iter()
            .find(|range| range.contains(self.current_time_us))
            .map(|range| range.end_us.saturating_sub(self.current_time_us))
            .unwrap_or_default()
    }

    fn reset_resource(&mut self) {
        if self.network_state != NetworkState::Empty {
            self.push_event(MediaEvent::Emptied);
        }
        self.network_state = NetworkState::Empty;
        self.ready_state = ReadyState::HaveNothing;
        self.paused = true;
        self.seeking = false;
        self.ended = false;
        self.current_time_us = 0;
        self.duration_us = None;
        self.source_url = None;
        self.media_source = None;
        self.direct_buffered_range = None;
        self.error = None;
    }

    fn push_event(&mut self, event: MediaEvent) {
        if self.events.len() >= MAX_MEDIA_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso_bmff::fixture;
    use crate::media_backend::{CodecCapability, DecoderCapabilities, DecoderProvider};
    use crate::media_core::MediaCodec;

    fn playable_source(duration_ms: u32, samples: usize) -> MediaSource {
        let capabilities = DecoderCapabilities {
            codecs: vec![CodecCapability {
                codec: MediaCodec::Avc,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            }],
            probe_error: None,
        };
        let mut source = MediaSource::new();
        source.open().unwrap();
        let id = source
            .add_source_buffer("video/mp4; codecs=\"avc1\"", &capabilities)
            .unwrap();
        source
            .append_buffer(id, &fixture::init(1, 1_000, b"vide", b"avc1"))
            .unwrap();
        let payload = vec![1u8];
        let payloads = (0..samples).map(|_| payload.as_slice()).collect::<Vec<_>>();
        source
            .append_buffer(id, &fixture::media(1, 0, duration_ms, &payloads))
            .unwrap();
        source
            .set_duration(i64::from(duration_ms) * samples as i64 * 1_000)
            .unwrap();
        source
    }

    #[test]
    fn media_element_play_pause_seek_rate_volume_and_end_are_ordered() {
        let source = playable_source(1_000, 8);
        let mut media = HtmlMediaElement::new();
        media.attach_media_source(source).unwrap();
        assert!(media.ready_state() >= ReadyState::HaveFutureData);
        media.play().unwrap();
        media.tick(1_000);
        assert_eq!(media.current_time_seconds(), 1.0);
        media.pause();
        media.tick(500);
        assert_eq!(media.current_time_seconds(), 1.0);
        media.seek(4.0).unwrap();
        media.set_playback_rate(2.0).unwrap();
        media.set_volume(0.5).unwrap();
        media.play().unwrap();
        media.tick(2_000);
        assert_eq!(media.current_time_seconds(), 8.0);
        assert!(media.controls_state().ended);
        let events = media.drain_events();
        assert!(events.contains(&MediaEvent::Seeking));
        assert!(events.contains(&MediaEvent::Seeked));
        assert!(events.contains(&MediaEvent::Ended));
    }

    #[test]
    fn media_element_waits_on_underflow_and_recovers_after_append() {
        let source = playable_source(200, 2);
        let mut media = HtmlMediaElement::new();
        media.attach_media_source(source).unwrap();
        media.play().unwrap();
        media.tick(400);
        assert!(media.controls_state().ended);
        media.seek(0.0).unwrap();
        media.play().unwrap();
        media.tick(100);
        assert!(!media.paused());
    }
}

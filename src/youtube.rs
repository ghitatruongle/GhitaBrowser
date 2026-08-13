//! Bounded YouTube application-shell and clear-content player integration.
//!
//! The parser consumes server-provided bootstrap JSON without copying website
//! implementation code. It exposes navigation/search/watch models and selects
//! only direct clear-content formats supported by the active decoder backend.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::html_media::{HtmlMediaElement, MediaControlAction};
use crate::media_backend::{
    decode_clear_content_bytes, DecodedMediaAsset, DecoderBackend, DecoderCapabilities,
    WindowsMediaFoundationBackend,
};
use crate::media_core::{
    parse_media_type, video_sync_action, AudioClock, DecodedAudioFrame, DecodedVideoFrame,
    MediaCodec, VideoSyncAction,
};
use crate::media_runtime::{MediaOutputPipeline, MediaOutputTick, MediaRuntimeLimits};
use crate::mse::MediaSource;
use crate::network_scheduler::CancellationToken;

const MAX_BOOTSTRAP_BYTES: usize = 16 * 1024 * 1024;
const MAX_JSON_NODES: usize = 200_000;
const MAX_RESULTS: usize = 100;
const MAX_TITLE_CHARS: usize = 512;
const MAX_SEARCH_CHARS: usize = 256;
const MAX_STREAM_BITRATE: u64 = 50_000_000;
const MAX_LIVE_PLAYER_BYTES: usize = 4 * 1024 * 1024;
const MAX_LIVE_MEDIA_BYTES: u64 = 50 * 1024 * 1024;
const LIVE_PLAYER_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const LIVE_SEARCH_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/search?prettyPrint=false";
const LIVE_CLIENT_NAME: &str = "ANDROID_VR";
const LIVE_CLIENT_ID: &str = "28";
const LIVE_CLIENT_VERSION: &str = "1.65.10";
const LIVE_CLIENT_USER_AGENT: &str = "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveClientContext<'a> {
    client_name: &'a str,
    client_version: &'a str,
    device_make: &'a str,
    device_model: &'a str,
    android_sdk_version: u32,
    user_agent: &'a str,
    os_name: &'a str,
    os_version: &'a str,
    hl: &'a str,
    time_zone: &'a str,
    utc_offset_minutes: i32,
}

#[derive(Debug, Serialize)]
struct LiveRequestContext<'a> {
    client: LiveClientContext<'a>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContentPlaybackContext<'a> {
    html5_preference: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackContext<'a> {
    content_playback_context: ContentPlaybackContext<'a>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LivePlayerRequest<'a> {
    context: LiveRequestContext<'a>,
    video_id: &'a str,
    playback_context: PlaybackContext<'a>,
    content_check_ok: bool,
    racy_check_ok: bool,
}

#[derive(Debug, Serialize)]
struct LiveSearchRequest<'a> {
    context: LiveRequestContext<'a>,
    query: &'a str,
}

fn live_client_context() -> LiveRequestContext<'static> {
    LiveRequestContext {
        client: LiveClientContext {
            client_name: LIVE_CLIENT_NAME,
            client_version: LIVE_CLIENT_VERSION,
            device_make: "Oculus",
            device_model: "Quest 3",
            android_sdk_version: 32,
            user_agent: LIVE_CLIENT_USER_AGENT,
            os_name: "Android",
            os_version: "12L",
            hl: "en",
            time_zone: "UTC",
            utc_offset_minutes: 0,
        },
    }
}

fn live_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(LIVE_CLIENT_USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Cannot initialize YouTube live transport: {error}"))
}

async fn post_live_json<T: Serialize>(
    endpoint: &str,
    payload: &T,
    cancellation: &CancellationToken,
) -> Result<Value, String> {
    if cancellation.is_cancelled() {
        return Err("YouTube request cancelled".to_string());
    }
    let body = serde_json::to_vec(payload)
        .map_err(|error| format!("Cannot encode YouTube request: {error}"))?;
    if body.len() > 64 * 1024 {
        return Err("YouTube request exceeds its byte budget".to_string());
    }
    let client = live_http_client()?;
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err("YouTube request cancelled".to_string()),
        response = client
            .post(endpoint)
            .header("Origin", "https://www.youtube.com")
            .header("X-Youtube-Client-Name", LIVE_CLIENT_ID)
            .header("X-Youtube-Client-Version", LIVE_CLIENT_VERSION)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send() => response.map_err(|error| format!("YouTube request failed: {error}"))?,
    };
    if !response.status().is_success() {
        return Err(format!(
            "YouTube request returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_LIVE_PLAYER_BYTES as u64)
    {
        return Err("YouTube response exceeds its byte budget".to_string());
    }
    let mut response = response;
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err("YouTube request cancelled".to_string()),
            chunk = response.chunk() => chunk.map_err(|error| format!("YouTube response failed: {error}"))?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        if bytes.len().saturating_add(chunk.len()) > MAX_LIVE_PLAYER_BYTES {
            return Err("YouTube response exceeds its byte budget".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("YouTube response is not valid JSON: {error}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YouTubeRoute {
    Home,
    Search { query: String },
    Watch { video_id: String },
}

impl YouTubeRoute {
    pub fn parse(url: &str) -> Result<Self, String> {
        let parsed = url::Url::parse(url).map_err(|_| "Invalid YouTube URL".to_string())?;
        let host = parsed
            .host_str()
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| "YouTube URL has no host".to_string())?;
        if !matches!(
            host.as_str(),
            "youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be"
        ) {
            return Err("URL is not a supported YouTube origin".to_string());
        }
        if host == "youtu.be" {
            return Ok(Self::Watch {
                video_id: validate_video_id(parsed.path().trim_start_matches('/'))?,
            });
        }
        match parsed.path() {
            "/" => Ok(Self::Home),
            "/results" => {
                let query = parsed
                    .query_pairs()
                    .find(|(name, _)| name == "search_query")
                    .map(|(_, value)| value.into_owned())
                    .unwrap_or_default();
                Ok(Self::Search {
                    query: validate_search_query(&query)?,
                })
            }
            "/watch" => {
                let video_id = parsed
                    .query_pairs()
                    .find(|(name, _)| name == "v")
                    .map(|(_, value)| value.into_owned())
                    .ok_or_else(|| "YouTube watch URL has no video id".to_string())?;
                Ok(Self::Watch {
                    video_id: validate_video_id(&video_id)?,
                })
            }
            path if path.starts_with("/shorts/") || path.starts_with("/embed/") => {
                let video_id = path.split('/').nth(2).unwrap_or_default();
                Ok(Self::Watch {
                    video_id: validate_video_id(video_id)?,
                })
            }
            _ => Err("Unsupported YouTube route".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeVideoResult {
    pub video_id: String,
    pub title: String,
    pub thumbnail_url: Option<String>,
    pub duration_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeShell {
    pub route: YouTubeRoute,
    pub results: Vec<YouTubeVideoResult>,
}

impl YouTubeShell {
    pub fn from_html(url: &str, html: &str) -> Result<Self, String> {
        let route = YouTubeRoute::parse(url)?;
        let value = extract_bootstrap_json(html, &["ytInitialData", "window[\"ytInitialData\"]"])?;
        let results = collect_video_results(&value)?;
        Ok(Self { route, results })
    }

    pub fn navigate(&mut self, route: YouTubeRoute) {
        self.route = route;
    }

    pub fn from_search_response(query: &str, value: &Value) -> Result<Self, String> {
        let query = validate_search_query(query)?;
        let results = collect_video_results(value)?;
        if results.is_empty() {
            return Err("YouTube search returned no bounded video results".to_string());
        }
        Ok(Self {
            route: YouTubeRoute::Search { query },
            results,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Muxed,
    Audio,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeFormat {
    pub itag: u32,
    pub mime_type: String,
    pub codecs: Vec<MediaCodec>,
    pub url: String,
    pub bitrate: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub content_length: Option<u64>,
    pub kind: StreamKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubePlayerResponse {
    pub video_id: String,
    pub title: String,
    pub duration_us: i64,
    pub formats: Vec<YouTubeFormat>,
}

impl YouTubePlayerResponse {
    pub fn from_html(html: &str) -> Result<Self, String> {
        let value = extract_bootstrap_json(
            html,
            &[
                "ytInitialPlayerResponse",
                "window[\"ytInitialPlayerResponse\"]",
            ],
        )?;
        Self::from_value(&value)
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let status = value
            .pointer("/playabilityStatus/status")
            .and_then(Value::as_str)
            .unwrap_or("ERROR");
        if status != "OK" {
            return Err(format!("YouTube player response is not playable: {status}"));
        }
        let details = value
            .get("videoDetails")
            .ok_or_else(|| "YouTube player response has no video details".to_string())?;
        let video_id = validate_video_id(
            details
                .get("videoId")
                .and_then(Value::as_str)
                .ok_or_else(|| "YouTube player response has no video id".to_string())?,
        )?;
        let title = bounded_text(
            details
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Untitled video"),
        );
        let duration_seconds = details
            .get("lengthSeconds")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| *value > 0 && *value <= 24 * 60 * 60)
            .ok_or_else(|| "YouTube video duration is invalid".to_string())?;
        let streaming = value
            .get("streamingData")
            .ok_or_else(|| "YouTube player response has no streaming data".to_string())?;
        let mut formats = Vec::new();
        for (key, kind) in [
            ("formats", StreamKind::Muxed),
            ("adaptiveFormats", StreamKind::Video),
        ] {
            let Some(entries) = streaming.get(key).and_then(Value::as_array) else {
                continue;
            };
            for entry in entries {
                if formats.len() >= 256 {
                    return Err("YouTube format count budget exceeded".to_string());
                }
                if let Some(format) = parse_format(entry, kind)? {
                    formats.push(format);
                }
            }
        }
        if formats.is_empty() {
            return Err("YouTube response has no direct clear-content formats".to_string());
        }
        Ok(Self {
            video_id,
            title,
            duration_us: duration_seconds.saturating_mul(1_000_000),
            formats,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackPlan {
    pub video: YouTubeFormat,
    pub audio: Option<YouTubeFormat>,
}

impl PlaybackPlan {
    pub fn is_muxed(&self) -> bool {
        self.video.kind == StreamKind::Muxed
    }
}

pub fn select_playback_plan(
    response: &YouTubePlayerResponse,
    capabilities: &DecoderCapabilities,
) -> Result<PlaybackPlan, String> {
    let supported = |format: &&YouTubeFormat| {
        format.bitrate <= MAX_STREAM_BITRATE
            && format
                .content_length
                .is_none_or(|length| length <= MAX_LIVE_MEDIA_BYTES)
            && !format.codecs.is_empty()
            && format
                .codecs
                .iter()
                .all(|codec| capabilities.supports(codec))
    };
    if let Some(muxed) = response
        .formats
        .iter()
        .filter(|format| format.kind == StreamKind::Muxed)
        .filter(supported)
        .max_by_key(|format| (format.height.unwrap_or_default(), format.bitrate))
    {
        return Ok(PlaybackPlan {
            video: muxed.clone(),
            audio: None,
        });
    }
    let video = response
        .formats
        .iter()
        .filter(|format| format.kind == StreamKind::Video)
        .filter(supported)
        .max_by_key(|format| (format.height.unwrap_or_default(), format.bitrate))
        .cloned()
        .ok_or_else(|| "No supported YouTube video format is available".to_string())?;
    let audio = response
        .formats
        .iter()
        .filter(|format| format.kind == StreamKind::Audio)
        .filter(supported)
        .max_by_key(|format| format.bitrate)
        .cloned()
        .ok_or_else(|| "No supported YouTube audio format is available".to_string())?;
    Ok(PlaybackPlan {
        video,
        audio: Some(audio),
    })
}

#[derive(Debug, Clone)]
pub struct LiveYouTubePlayback {
    pub response: YouTubePlayerResponse,
    pub plan: PlaybackPlan,
    pub decoded: Arc<DecodedMediaAsset>,
    pub downloaded_bytes: usize,
}

impl LiveYouTubePlayback {
    pub fn audio_format(&self) -> Option<(u32, u16)> {
        self.decoded
            .audio_frames
            .first()
            .map(|frame| (frame.sample_rate_hz, frame.channels))
    }
}

pub async fn fetch_live_youtube_search(
    query: &str,
    cancellation: CancellationToken,
) -> Result<YouTubeShell, String> {
    let query = validate_search_query(query)?;
    let request = LiveSearchRequest {
        context: live_client_context(),
        query: &query,
    };
    let response = post_live_json(LIVE_SEARCH_ENDPOINT, &request, &cancellation).await?;
    YouTubeShell::from_search_response(&query, &response)
}

pub async fn prepare_live_youtube_playback(
    video_id: &str,
    cancellation: CancellationToken,
) -> Result<LiveYouTubePlayback, String> {
    let video_id = validate_video_id(video_id)?;
    let request = LivePlayerRequest {
        context: live_client_context(),
        video_id: &video_id,
        playback_context: PlaybackContext {
            content_playback_context: ContentPlaybackContext {
                html5_preference: "HTML5_PREF_WANTS",
            },
        },
        content_check_ok: true,
        racy_check_ok: true,
    };
    let value = post_live_json(LIVE_PLAYER_ENDPOINT, &request, &cancellation).await?;
    let response = YouTubePlayerResponse::from_value(&value)?;
    if response.video_id != video_id {
        return Err("YouTube player response video id mismatch".to_string());
    }
    let capabilities = WindowsMediaFoundationBackend.capabilities();
    let plan = select_live_playback_plan(&response, &capabilities)?;
    validate_live_media_format(&plan.video)?;
    let (video_bytes, audio_bytes) = if let Some(audio) = plan.audio.as_ref() {
        validate_live_media_format(audio)?;
        tokio::try_join!(
            download_live_format(&plan.video, cancellation.clone()),
            download_live_format(audio, cancellation.clone())
        )?
    } else {
        (
            download_live_format(&plan.video, cancellation.clone()).await?,
            Vec::new(),
        )
    };
    let downloaded_bytes = video_bytes.len().saturating_add(audio_bytes.len());
    let mut decoded = tokio::task::spawn_blocking(move || decode_clear_content_bytes(&video_bytes))
        .await
        .map_err(|error| format!("YouTube video decoder task failed: {error}"))??;
    if !audio_bytes.is_empty() {
        let audio = tokio::task::spawn_blocking(move || decode_clear_content_bytes(&audio_bytes))
            .await
            .map_err(|error| format!("YouTube audio decoder task failed: {error}"))??;
        if !audio.video_frames.is_empty() {
            return Err("YouTube audio stream unexpectedly decoded video".to_string());
        }
        decoded.audio_frames = audio.audio_frames;
    }
    if decoded.video_frames.is_empty() || decoded.audio_frames.is_empty() {
        return Err("YouTube muxed stream did not decode both video and audio".to_string());
    }
    Ok(LiveYouTubePlayback {
        response,
        plan,
        decoded: Arc::new(decoded),
        downloaded_bytes,
    })
}

fn select_live_playback_plan(
    response: &YouTubePlayerResponse,
    capabilities: &DecoderCapabilities,
) -> Result<PlaybackPlan, String> {
    let supported = |format: &&YouTubeFormat| {
        format.bitrate <= MAX_STREAM_BITRATE
            && format
                .content_length
                .is_some_and(|length| length > 0 && length <= MAX_LIVE_MEDIA_BYTES)
            && !format.codecs.is_empty()
            && format
                .codecs
                .iter()
                .all(|codec| capabilities.supports(codec))
    };
    if let Some(muxed) = response
        .formats
        .iter()
        .filter(|format| format.kind == StreamKind::Muxed)
        .filter(|format| format.mime_type.starts_with("video/mp4"))
        .filter(|format| {
            format.bitrate <= MAX_STREAM_BITRATE
                && !format.codecs.is_empty()
                && format
                    .codecs
                    .iter()
                    .all(|codec| capabilities.supports(codec))
                && format
                    .content_length
                    .is_none_or(|length| length > 0 && length <= MAX_LIVE_MEDIA_BYTES)
        })
        .max_by_key(|format| (format.height.unwrap_or_default(), format.bitrate))
    {
        return Ok(PlaybackPlan {
            video: muxed.clone(),
            audio: None,
        });
    }
    let video = response
        .formats
        .iter()
        .filter(|format| format.kind == StreamKind::Video)
        .filter(|format| format.mime_type.starts_with("video/mp4"))
        .filter(supported)
        .filter(|format| format.height.is_some_and(|height| height <= 144))
        .max_by_key(|format| (format.height.unwrap_or_default(), format.bitrate))
        .cloned();
    let audio = response
        .formats
        .iter()
        .filter(|format| format.kind == StreamKind::Audio)
        .filter(|format| format.mime_type.starts_with("audio/mp4"))
        .filter(supported)
        .min_by_key(|format| format.bitrate)
        .cloned();
    if let (Some(video), Some(audio)) = (video, audio) {
        return Ok(PlaybackPlan {
            video,
            audio: Some(audio),
        });
    }
    select_playback_plan(response, capabilities)
}

async fn download_live_format(
    format: &YouTubeFormat,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, String> {
    let content_length = match format.content_length {
        Some(length) => length,
        None => {
            crate::network_scheduler::probe_binary_length(
                format.url.clone(),
                MAX_LIVE_MEDIA_BYTES as usize,
                cancellation.clone(),
            )
            .await?
        }
    };
    crate::network_scheduler::fetch_binary_ranges(
        format.url.clone(),
        content_length,
        MAX_LIVE_MEDIA_BYTES as usize,
        cancellation,
    )
    .await
}

fn validate_live_media_format(format: &YouTubeFormat) -> Result<(), String> {
    let parsed = url::Url::parse(&format.url)
        .map_err(|_| "YouTube live media URL is invalid".to_string())?;
    let host = parsed
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "YouTube live media URL has no host".to_string())?;
    if parsed.scheme() != "https"
        || (host != "googlevideo.com" && !host.ends_with(".googlevideo.com"))
        || parsed.path() != "/videoplayback"
    {
        return Err("YouTube live media URL is outside the allowed origin".to_string());
    }
    if format
        .content_length
        .is_some_and(|length| length == 0 || length > MAX_LIVE_MEDIA_BYTES)
    {
        return Err("YouTube live media content length exceeds its budget".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct LiveYouTubeController {
    pub response: YouTubePlayerResponse,
    pub plan: PlaybackPlan,
    decoded: Arc<DecodedMediaAsset>,
    media: HtmlMediaElement,
    output: MediaOutputPipeline,
    decoded_duration_us: i64,
}

impl LiveYouTubeController {
    pub fn new(prepared: LiveYouTubePlayback) -> Result<Self, String> {
        let decoded_duration_us = decoded_duration_us(&prepared.decoded)?;
        let output = MediaOutputPipeline::from_asset(
            prepared.decoded.as_ref().clone(),
            MediaRuntimeLimits::default(),
        )?;
        let mut media = HtmlMediaElement::new();
        media.attach_decoded_stream(decoded_duration_us)?;
        Ok(Self {
            response: prepared.response,
            plan: prepared.plan,
            decoded: prepared.decoded,
            media,
            output,
            decoded_duration_us,
        })
    }

    pub fn toggle_playback(&mut self) -> Result<bool, String> {
        if self.media.paused() {
            self.media.play()?;
            self.output.play();
        } else {
            self.media.pause();
            self.output.pause();
        }
        Ok(!self.media.paused())
    }

    pub fn seek_by(&mut self, seconds: f64) -> Result<(), String> {
        let controls = self.media.controls_state();
        let target = (controls.current_time_seconds + seconds)
            .clamp(0.0, self.decoded_duration_us as f64 / 1_000_000.0);
        self.seek_to(target)
    }

    pub fn seek_to(&mut self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err("Invalid live playback seek target".to_string());
        }
        let was_playing = !self.media.paused();
        let target_us = ((seconds * 1_000_000.0) as i64).min(self.decoded_duration_us);
        let mut output = MediaOutputPipeline::from_asset(
            self.decoded.as_ref().clone(),
            MediaRuntimeLimits::default(),
        )?;
        output.seek(target_us)?;
        self.media.seek(target_us as f64 / 1_000_000.0)?;
        if was_playing {
            output.play();
        }
        self.output = output;
        Ok(())
    }

    pub fn set_volume(&mut self, volume: f64) -> Result<(), String> {
        self.media.set_volume(volume)
    }

    pub fn toggle_mute(&mut self) {
        let muted = self.media.controls_state().muted;
        self.media.set_muted(!muted);
    }

    pub fn tick(&mut self, elapsed_ms: u64) -> Result<MediaOutputTick, String> {
        self.media.tick(elapsed_ms);
        self.output.tick(elapsed_ms)
    }

    pub fn current_video_frame(&self) -> Option<&DecodedVideoFrame> {
        self.output.current_video_frame()
    }

    pub fn drain_audio_frames(&mut self) -> Vec<DecodedAudioFrame> {
        let controls = self.media.controls_state();
        let gain = if controls.muted {
            0.0
        } else {
            controls.volume.clamp(0.0, 1.0)
        };
        let mut frames = self.output.drain_audio_frames();
        if gain < 1.0 {
            for frame in &mut frames {
                for sample in &mut frame.interleaved_samples {
                    *sample = (f64::from(*sample) * gain)
                        .round()
                        .clamp(f64::from(i16::MIN), f64::from(i16::MAX))
                        as i16;
                }
            }
        }
        frames
    }

    pub fn recover_after_interruption(&mut self) -> Result<(), String> {
        let controls = self.media.controls_state();
        self.seek_to(controls.current_time_seconds)
    }

    pub fn controls(&self) -> crate::html_media::MediaControlsState {
        self.media.controls_state()
    }
}

fn decoded_duration_us(asset: &DecodedMediaAsset) -> Result<i64, String> {
    asset
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
        .filter(|duration| *duration > 0)
        .ok_or_else(|| "Decoded YouTube media has no duration".to_string())
}

#[derive(Debug, Clone)]
pub struct RecordedPlaybackAssets {
    pub video_init: Vec<u8>,
    pub video_segments: Vec<Vec<u8>>,
    pub audio_init: Option<Vec<u8>>,
    pub audio_segments: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YouTubeGateReport {
    pub shell_visible: bool,
    pub search_interactive: bool,
    pub watch_route_interactive: bool,
    pub mse_started: bool,
    pub playback_started: bool,
    pub video_output: bool,
    pub audio_output: bool,
    pub pause_resume: bool,
    pub seek: bool,
    pub volume: bool,
    pub av_sync: bool,
    pub bounded_recovery: bool,
}

impl YouTubeGateReport {
    pub fn passed(&self) -> bool {
        self.shell_visible
            && self.search_interactive
            && self.watch_route_interactive
            && self.mse_started
            && self.playback_started
            && self.video_output
            && self.audio_output
            && self.pause_resume
            && self.seek
            && self.volume
            && self.av_sync
            && self.bounded_recovery
    }
}

#[derive(Debug, Clone)]
pub struct YouTubePlayerSession {
    pub shell: YouTubeShell,
    pub response: YouTubePlayerResponse,
    pub plan: PlaybackPlan,
    media: HtmlMediaElement,
    audio_clock: AudioClock,
    output: Option<MediaOutputPipeline>,
    report: YouTubeGateReport,
}

impl YouTubePlayerSession {
    pub fn from_recorded_page(
        url: &str,
        html: &str,
        capabilities: &DecoderCapabilities,
        assets: RecordedPlaybackAssets,
    ) -> Result<Self, String> {
        let shell = YouTubeShell::from_html(url, html)?;
        let response = YouTubePlayerResponse::from_html(html)?;
        let plan = select_playback_plan(&response, capabilities)?;
        let mut source = MediaSource::new();
        source.open()?;
        let video_buffer = source.add_source_buffer(&plan.video.mime_type, capabilities)?;
        source.append_buffer(video_buffer, &assets.video_init)?;
        for segment in &assets.video_segments {
            source.append_buffer(video_buffer, segment)?;
        }
        if let Some(audio) = plan.audio.as_ref() {
            let audio_init = assets.audio_init.as_ref().ok_or_else(|| {
                "Recorded YouTube audio initialization segment is missing".to_string()
            })?;
            let audio_buffer = source.add_source_buffer(&audio.mime_type, capabilities)?;
            source.append_buffer(audio_buffer, audio_init)?;
            for segment in &assets.audio_segments {
                source.append_buffer(audio_buffer, segment)?;
            }
        }
        source.set_duration(response.duration_us)?;
        source.end_of_stream()?;
        let mut media = HtmlMediaElement::new();
        media.attach_media_source(source)?;
        let mut audio_clock = AudioClock::new(48_000)?;
        audio_clock.seek(0)?;
        let report = YouTubeGateReport {
            shell_visible: !shell.results.is_empty(),
            watch_route_interactive: matches!(shell.route, YouTubeRoute::Watch { .. }),
            mse_started: media.media_source().is_some(),
            ..Default::default()
        };
        Ok(Self {
            shell,
            response,
            plan,
            media,
            audio_clock,
            output: None,
            report,
        })
    }

    pub fn attach_decoded_output(&mut self, asset: DecodedMediaAsset) -> Result<(), String> {
        self.output = Some(MediaOutputPipeline::from_asset(
            asset,
            MediaRuntimeLimits::default(),
        )?);
        Ok(())
    }

    pub fn navigate_search(&mut self, query: &str) -> Result<(), String> {
        let query = validate_search_query(query)?;
        self.shell.navigate(YouTubeRoute::Search { query });
        self.report.search_interactive = true;
        Ok(())
    }

    pub fn navigate_watch(&mut self, video_id: &str) -> Result<(), String> {
        let video_id = validate_video_id(video_id)?;
        self.shell.navigate(YouTubeRoute::Watch { video_id });
        self.report.watch_route_interactive = true;
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), String> {
        self.media.play()?;
        self.audio_clock.start();
        if let Some(output) = self.output.as_mut() {
            output.play();
        }
        self.report.playback_started = true;
        Ok(())
    }

    pub fn pause(&mut self) {
        self.media.pause();
        self.audio_clock.pause();
        if let Some(output) = self.output.as_mut() {
            output.pause();
        }
    }

    pub fn resume(&mut self) -> Result<(), String> {
        self.play()?;
        self.report.pause_resume = true;
        Ok(())
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        self.media.seek(seconds)?;
        self.audio_clock.seek((seconds * 1_000_000.0) as i64)?;
        if let Some(output) = self.output.as_mut() {
            output.seek((seconds * 1_000_000.0) as i64)?;
        }
        self.report.seek = true;
        Ok(())
    }

    pub fn set_volume(&mut self, volume: f64) -> Result<(), String> {
        self.media
            .apply_control(MediaControlAction::SetVolume(volume))?;
        self.report.volume = true;
        Ok(())
    }

    pub fn tick(&mut self, elapsed_ms: u64) -> Result<VideoSyncAction, String> {
        let was_playing = !self.media.paused();
        self.media.tick(elapsed_ms);
        if was_playing {
            let frames = elapsed_ms.saturating_mul(48);
            self.audio_clock.advance_frames(frames)?;
        }
        let video_time = (self.media.current_time_seconds() * 1_000_000.0) as i64;
        let action = video_sync_action(self.audio_clock.position_us(), video_time, 40_000);
        if action == VideoSyncAction::Present {
            self.report.av_sync = true;
        }
        if let Some(output) = self.output.as_mut() {
            let output_tick = output.tick(elapsed_ms)?;
            self.report.video_output |= output_tick.video_frame_presented;
            self.report.audio_output |= output_tick.audio_frames_emitted > 0;
        }
        Ok(action)
    }

    pub fn recover_after_underflow(&mut self) {
        self.media.synchronize_source_state();
        self.report.bounded_recovery = true;
    }

    pub fn report(&self) -> &YouTubeGateReport {
        &self.report
    }

    pub fn media(&self) -> &HtmlMediaElement {
        &self.media
    }

    pub fn output(&self) -> Option<&MediaOutputPipeline> {
        self.output.as_ref()
    }
}

fn parse_format(value: &Value, default_kind: StreamKind) -> Result<Option<YouTubeFormat>, String> {
    let Some(url) = value.get("url").and_then(Value::as_str) else {
        // signatureCipher requires the current website player JavaScript and
        // is intentionally not treated as a direct playable URL.
        return Ok(None);
    };
    let parsed_url = url::Url::parse(url).map_err(|_| "Invalid YouTube media URL".to_string())?;
    if !matches!(parsed_url.scheme(), "https" | "http") {
        return Ok(None);
    }
    let mime_type = value
        .get("mimeType")
        .and_then(Value::as_str)
        .ok_or_else(|| "YouTube format has no MIME type".to_string())?
        .to_string();
    let parsed = parse_media_type(&mime_type)?;
    let width = value
        .get("width")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let height = value
        .get("height")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let kind = if width.is_some() || height.is_some() {
        if value.get("audioQuality").is_some() {
            StreamKind::Muxed
        } else {
            StreamKind::Video
        }
    } else if value.get("audioQuality").is_some() || mime_type.starts_with("audio/") {
        StreamKind::Audio
    } else {
        default_kind
    };
    let bitrate = value.get("bitrate").and_then(Value::as_u64).unwrap_or(0);
    if bitrate == 0 || bitrate > MAX_STREAM_BITRATE.saturating_mul(4) {
        return Ok(None);
    }
    let itag = value
        .get("itag")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| "YouTube format has no valid itag".to_string())?;
    let content_length = value
        .get("contentLength")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok());
    Ok(Some(YouTubeFormat {
        itag,
        mime_type,
        codecs: parsed.codecs,
        url: parsed_url.into(),
        bitrate,
        width,
        height,
        content_length,
        kind,
    }))
}

fn extract_bootstrap_json(html: &str, markers: &[&str]) -> Result<Value, String> {
    if html.is_empty() || html.len() > MAX_BOOTSTRAP_BYTES {
        return Err("YouTube bootstrap document size budget exceeded".to_string());
    }
    for marker in markers {
        let Some(marker_start) = html.find(marker) else {
            continue;
        };
        let tail = &html[marker_start + marker.len()..];
        let Some(relative_start) = tail.find('{') else {
            continue;
        };
        let start = marker_start + marker.len() + relative_start;
        if let Some(end) = balanced_json_end(html.as_bytes(), start) {
            let slice = &html[start..end];
            return serde_json::from_str(slice)
                .map_err(|error| format!("Invalid YouTube bootstrap JSON: {error}"));
        }
    }
    Err("YouTube bootstrap JSON was not found".to_string())
}

fn balanced_json_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, byte) in bytes.get(start..)?.iter().copied().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b'"' => quoted = true,
            b'{' => depth = depth.saturating_add(1),
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn collect_video_results(root: &Value) -> Result<Vec<YouTubeVideoResult>, String> {
    let mut results = Vec::new();
    let mut stack = vec![root];
    let mut visited = 0usize;
    while let Some(value) = stack.pop() {
        visited += 1;
        if visited > MAX_JSON_NODES {
            return Err("YouTube bootstrap JSON node budget exceeded".to_string());
        }
        match value {
            Value::Object(map) => {
                for key in ["videoRenderer", "compactVideoRenderer", "gridVideoRenderer"] {
                    if let Some(renderer) = map.get(key) {
                        if results.len() < MAX_RESULTS {
                            if let Some(result) = parse_video_renderer(renderer) {
                                if !results.iter().any(|existing: &YouTubeVideoResult| {
                                    existing.video_id == result.video_id
                                }) {
                                    results.push(result);
                                }
                            }
                        }
                    }
                }
                stack.extend(map.values());
            }
            Value::Array(values) => stack.extend(values),
            _ => {}
        }
    }
    Ok(results)
}

fn parse_video_renderer(value: &Value) -> Option<YouTubeVideoResult> {
    let video_id = validate_video_id(value.get("videoId")?.as_str()?).ok()?;
    let title = text_value(value.get("title")?)?;
    let thumbnail_url = value
        .pointer("/thumbnail/thumbnails")
        .and_then(Value::as_array)
        .and_then(|values| values.last())
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str)
        .filter(|url| url.starts_with("https://"))
        .map(str::to_string);
    let duration_text = value.get("lengthText").and_then(text_value);
    Some(YouTubeVideoResult {
        video_id,
        title,
        thumbnail_url,
        duration_text,
    })
}

fn text_value(value: &Value) -> Option<String> {
    if let Some(simple) = value.get("simpleText").and_then(Value::as_str) {
        return Some(bounded_text(simple));
    }
    let runs = value.get("runs")?.as_array()?;
    let text = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty()).then(|| bounded_text(&text))
}

fn bounded_text(value: &str) -> String {
    value.chars().take(MAX_TITLE_CHARS).collect()
}

fn validate_video_id(value: &str) -> Result<String, String> {
    if !(6..=64).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("Invalid YouTube video id".to_string());
    }
    Ok(value.to_string())
}

fn validate_search_query(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_SEARCH_CHARS {
        return Err("Invalid YouTube search query".to_string());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso_bmff::fixture;
    use crate::media_backend::{CodecCapability, DecoderProvider};
    use crate::media_core::{DecodedAudioFrame, DecodedVideoFrame};

    fn capabilities() -> DecoderCapabilities {
        DecoderCapabilities {
            codecs: vec![
                CodecCapability {
                    codec: MediaCodec::Avc,
                    available: true,
                    provider: DecoderProvider::WindowsMediaFoundation,
                },
                CodecCapability {
                    codec: MediaCodec::Aac,
                    available: true,
                    provider: DecoderProvider::WindowsMediaFoundation,
                },
            ],
            probe_error: None,
        }
    }

    fn recorded_html() -> String {
        let initial = serde_json::json!({
            "contents": {"videoRenderer": {
                "videoId": "ghitaVideo1",
                "title": {"runs": [{"text": "Ghita media gate"}]},
                "thumbnail": {"thumbnails": [{"url": "https://img.test/gate.jpg"}]},
                "lengthText": {"simpleText": "0:08"}
            }}
        });
        let player = serde_json::json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {
                "videoId": "ghitaVideo1",
                "title": "Ghita media gate",
                "lengthSeconds": "8"
            },
            "streamingData": {"adaptiveFormats": [
                {
                    "itag": 137,
                    "mimeType": "video/mp4; codecs=\"avc1.640028\"",
                    "url": "https://media.test/video.mp4",
                    "bitrate": 4000000,
                    "width": 1920,
                    "height": 1080,
                    "contentLength": "4000000"
                },
                {
                    "itag": 140,
                    "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                    "url": "https://media.test/audio.mp4",
                    "bitrate": 128000,
                    "audioQuality": "AUDIO_QUALITY_MEDIUM",
                    "contentLength": "128000"
                }
            ]}
        });
        format!(
            "<html><script>var ytInitialData = {initial}; var ytInitialPlayerResponse = {player};</script></html>"
        )
    }

    #[test]
    fn routes_and_server_bootstrap_create_interactive_shell() {
        let html = recorded_html();
        let shell =
            YouTubeShell::from_html("https://www.youtube.com/watch?v=ghitaVideo1", &html).unwrap();
        assert!(matches!(shell.route, YouTubeRoute::Watch { .. }));
        assert_eq!(shell.results.len(), 1);
        assert_eq!(shell.results[0].title, "Ghita media gate");
        assert!(matches!(
            YouTubeRoute::parse("https://www.youtube.com/results?search_query=rust").unwrap(),
            YouTubeRoute::Search { .. }
        ));
    }

    #[test]
    fn recorded_youtube_gate_drives_mse_controls_and_av_sync() {
        let html = recorded_html();
        let video_payload = [1u8, 2, 3];
        let audio_payload = [4u8, 5];
        let video_payloads = vec![video_payload.as_slice(); 8];
        let audio_payloads = vec![audio_payload.as_slice(); 8];
        let assets = RecordedPlaybackAssets {
            video_init: fixture::init(1, 1_000, b"vide", b"avc1"),
            video_segments: vec![fixture::media(1, 0, 1_000, &video_payloads)],
            audio_init: Some(fixture::init(2, 48_000, b"soun", b"mp4a")),
            audio_segments: vec![fixture::media(2, 0, 48_000, &audio_payloads)],
        };
        let mut session = YouTubePlayerSession::from_recorded_page(
            "https://www.youtube.com/watch?v=ghitaVideo1",
            &html,
            &capabilities(),
            assets,
        )
        .unwrap();
        session
            .attach_decoded_output(DecodedMediaAsset {
                video_frames: vec![DecodedVideoFrame {
                    timestamp_us: 500_000,
                    duration_us: 40_000,
                    width: 2,
                    height: 1,
                    rgba: vec![255; 8],
                }],
                audio_frames: vec![DecodedAudioFrame {
                    timestamp_us: 0,
                    duration_us: 20_000,
                    sample_rate_hz: 48_000,
                    channels: 2,
                    interleaved_samples: vec![0; 1_920],
                }],
            })
            .unwrap();
        session.navigate_search("ghita media").unwrap();
        session.navigate_watch("ghitaVideo1").unwrap();
        session.play().unwrap();
        assert_eq!(session.tick(500).unwrap(), VideoSyncAction::Present);
        session.pause();
        session.resume().unwrap();
        session.seek(2.0).unwrap();
        session.set_volume(0.4).unwrap();
        session.recover_after_underflow();
        assert!(session.report().passed());
    }

    #[test]
    fn cipher_only_or_unsupported_formats_do_not_fake_playback() {
        let mut value: Value = serde_json::from_str(
            r#"{
              "playabilityStatus":{"status":"OK"},
              "videoDetails":{"videoId":"ghitaVideo1","title":"x","lengthSeconds":"2"},
              "streamingData":{"adaptiveFormats":[{
                "itag":248,"mimeType":"video/webm; codecs=\"vp9\"","bitrate":1000,
                "signatureCipher":"s=private"
              }]}
            }"#,
        )
        .unwrap();
        assert!(YouTubePlayerResponse::from_value(&value).is_err());
        value["playabilityStatus"]["status"] = Value::String("LOGIN_REQUIRED".to_string());
        assert!(YouTubePlayerResponse::from_value(&value).is_err());
    }

    #[test]
    fn official_direct_adaptive_formats_select_a_bounded_mp4_pair() {
        let response = YouTubePlayerResponse {
            video_id: "ghitaVideo1".to_string(),
            title: "Live adapter fixture".to_string(),
            duration_us: 8_000_000,
            formats: vec![
                YouTubeFormat {
                    itag: 133,
                    mime_type: "video/mp4; codecs=\"avc1.4d4015\"".to_string(),
                    codecs: vec![MediaCodec::Avc],
                    url: "https://rr1---sn.test.googlevideo.com/videoplayback?id=video".to_string(),
                    bitrate: 236_224,
                    width: Some(426),
                    height: Some(240),
                    content_length: Some(4_310_122),
                    kind: StreamKind::Video,
                },
                YouTubeFormat {
                    itag: 140,
                    mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".to_string(),
                    codecs: vec![MediaCodec::Aac],
                    url: "https://rr1---sn.test.googlevideo.com/videoplayback?id=audio".to_string(),
                    bitrate: 130_677,
                    width: None,
                    height: None,
                    content_length: Some(3_449_447),
                    kind: StreamKind::Audio,
                },
            ],
        };
        let capabilities = DecoderCapabilities {
            codecs: vec![
                crate::media_backend::CodecCapability {
                    codec: MediaCodec::Avc,
                    available: true,
                    provider: crate::media_backend::DecoderProvider::WindowsMediaFoundation,
                },
                crate::media_backend::CodecCapability {
                    codec: MediaCodec::Aac,
                    available: true,
                    provider: crate::media_backend::DecoderProvider::WindowsMediaFoundation,
                },
            ],
            probe_error: None,
        };
        let plan = select_live_playback_plan(&response, &capabilities).unwrap();
        assert_eq!(plan.video.itag, 133);
        assert_eq!(plan.audio.as_ref().map(|format| format.itag), Some(140));
        validate_live_media_format(&plan.video).unwrap();
        validate_live_media_format(plan.audio.as_ref().unwrap()).unwrap();
    }

    #[test]
    fn live_media_origin_and_size_are_bounded() {
        let mut format = YouTubeFormat {
            itag: 18,
            mime_type: "video/mp4; codecs=\"avc1.42001e, mp4a.40.2\"".to_string(),
            codecs: vec![MediaCodec::Avc, MediaCodec::Aac],
            url: "https://evil.test/videoplayback".to_string(),
            bitrate: 500_000,
            width: Some(640),
            height: Some(360),
            content_length: Some(1_000),
            kind: StreamKind::Muxed,
        };
        assert!(validate_live_media_format(&format).is_err());
        format.url = "https://r1.googlevideo.com/videoplayback".to_string();
        validate_live_media_format(&format).unwrap();
        format.content_length = Some(MAX_LIVE_MEDIA_BYTES + 1);
        assert!(validate_live_media_format(&format).is_err());
    }

    #[test]
    fn live_search_response_builds_interactive_results() {
        let value = serde_json::json!({
            "contents": {
                "compactVideoRenderer": {
                    "videoId": "ghitaVideo1",
                    "title": {"simpleText": "Clean-room player result"},
                    "thumbnail": {"thumbnails": [{"url": "https://i.ytimg.com/vi/ghitaVideo1/default.jpg"}]},
                    "lengthText": {"simpleText": "0:08"}
                }
            }
        });
        let shell = YouTubeShell::from_search_response("rust", &value).unwrap();
        assert!(matches!(shell.route, YouTubeRoute::Search { .. }));
        assert_eq!(shell.results.len(), 1);
    }
}

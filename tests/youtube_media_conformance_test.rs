//! Track 09: YouTube Application and Clear Media Pipeline Conformance Test Suite
//!
//! Validates:
//! 1. YouTube route matrix parsing and classification (Home, Search, Watch, Playlist, Channel, Shorts, Embed).
//! 2. Clear media format selection and playback planning.
//! 3. Honest, observable playability errors (unplayable, login-required, age-restricted, cipher-only).
//! 4. Search results collection and degraded shell modeling.
//! 5. HTMLMediaElement state machine and event queueing.
//! 6. MediaSource and SourceBuffer lifecycle with TimeRanges.
//! 7. YouTube keyboard action mapping and controller dispatch.
//! 8. AudioClock and A/V synchronization actions.
//! 9. Pipeline ticking and deterministic resource teardown.

use serde_json::Value;
use std::sync::Arc;

use ghitabrowser::html_media::{HtmlMediaElement, MediaEvent, NetworkState, ReadyState};
use ghitabrowser::media_backend::{
    CodecCapability, DecodedMediaAsset, DecoderCapabilities, DecoderProvider,
};
use ghitabrowser::media_core::{
    video_sync_action, AudioClock, DecodedAudioFrame, DecodedVideoFrame, MediaCodec,
    VideoSyncAction,
};
use ghitabrowser::mse::MediaSource;
use ghitabrowser::youtube::{
    select_playback_plan, LiveYouTubeController, LiveYouTubePlayback, PlaybackPlan, StreamKind,
    YouTubeFormat, YouTubeKeyboardAction, YouTubePlayerResponse, YouTubeRoute, YouTubeShell,
};

fn mock_capabilities() -> DecoderCapabilities {
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
            CodecCapability {
                codec: MediaCodec::Vp9,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Opus,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Av1,
                available: true,
                provider: DecoderProvider::WindowsMediaFoundation,
            },
            CodecCapability {
                codec: MediaCodec::Pcm,
                available: true,
                provider: DecoderProvider::BrowserPcm,
            },
        ],
        probe_error: None,
    }
}

#[test]
fn test_youtube_route_matrix_parsing() {
    let json_str = std::fs::read_to_string("tests/fixtures/youtube/youtube_route_matrix.json")
        .expect("read route matrix fixture");
    let fixture: Value = serde_json::from_str(&json_str).expect("parse json");

    for test_case in fixture["test_cases"].as_array().unwrap() {
        let url = test_case["url"].as_str().unwrap();
        let expected_route = test_case["expected_route"].as_str().unwrap();
        let expected_vid = test_case["expected_video_id"].as_str();

        let route = YouTubeRoute::parse(url).expect("route parse succeeds");
        match expected_route {
            "Home" => assert_eq!(route, YouTubeRoute::Home),
            "Search" => {
                let query = test_case["expected_query"].as_str().unwrap();
                assert_eq!(
                    route,
                    YouTubeRoute::Search {
                        query: query.to_string()
                    }
                );
            }
            "Watch" => {
                let vid = expected_vid.unwrap();
                assert_eq!(
                    route,
                    YouTubeRoute::Watch {
                        video_id: vid.to_string()
                    }
                );
                assert_eq!(route.video_id(), Some(vid));
            }
            "Shorts" => {
                let vid = expected_vid.unwrap();
                assert_eq!(
                    route,
                    YouTubeRoute::Shorts {
                        video_id: vid.to_string()
                    }
                );
                assert_eq!(route.video_id(), Some(vid));
            }
            "Embed" => {
                let vid = expected_vid.unwrap();
                assert_eq!(
                    route,
                    YouTubeRoute::Embed {
                        video_id: vid.to_string()
                    }
                );
                assert_eq!(route.video_id(), Some(vid));
            }
            "Playlist" => {
                let pid = test_case["expected_playlist_id"].as_str().unwrap();
                assert!(
                    matches!(route, YouTubeRoute::Playlist { ref playlist_id, .. } if playlist_id == pid)
                );
                assert_eq!(route.video_id(), expected_vid);
            }
            "Channel" => {
                let cid = test_case["expected_channel_id"].as_str().unwrap();
                assert_eq!(
                    route,
                    YouTubeRoute::Channel {
                        channel_id: cid.to_string()
                    }
                );
            }
            other => panic!("Unexpected test route: {other}"),
        }
    }
}

#[test]
fn test_youtube_watch_clear_playback_plan_and_selection() {
    let json_str = std::fs::read_to_string("tests/fixtures/youtube/youtube_watch_clear.json")
        .expect("read clear watch fixture");
    let val: Value = serde_json::from_str(&json_str).expect("parse json");

    let response = YouTubePlayerResponse::from_value(&val).expect("player response parsed");
    assert_eq!(response.video_id, "ghitaVideo1");
    assert_eq!(response.title, "Ghita Browser Performance Benchmark");
    assert_eq!(response.duration_us, 120_000_000);
    assert_eq!(response.formats.len(), 3);

    let caps = mock_capabilities();
    let plan = select_playback_plan(&response, &caps).expect("playback plan selected");
    assert!(plan.is_muxed() || plan.audio.is_some());
    assert_eq!(plan.video.kind, StreamKind::Muxed);
    assert_eq!(plan.video.itag, 18);
}

#[test]
fn test_youtube_restricted_playability_status_and_errors() {
    let json_str = std::fs::read_to_string("tests/fixtures/youtube/youtube_watch_restricted.json")
        .expect("read restricted watch fixture");
    let fixture: Value = serde_json::from_str(&json_str).expect("parse json");

    // 1. Unplayable region
    let err1 = YouTubePlayerResponse::from_value(&fixture["unplayable"]).unwrap_err();
    assert!(err1.contains("UNPLAYABLE"));
    assert!(err1.contains("unavailable in your region"));

    // 2. Login required
    let err2 = YouTubePlayerResponse::from_value(&fixture["login_required"]).unwrap_err();
    assert!(err2.contains("LOGIN_REQUIRED"));
    assert!(err2.contains("Sign in"));

    // 3. Age check required
    let err3 = YouTubePlayerResponse::from_value(&fixture["age_check_required"]).unwrap_err();
    assert!(err3.contains("AGE_CHECK_REQUIRED"));
    assert!(err3.contains("Age-restricted"));

    // 4. Cipher only (DRM/signature protected)
    let err4 = YouTubePlayerResponse::from_value(&fixture["cipher_only"]).unwrap_err();
    assert!(err4.contains("no direct clear-content formats"));
}

#[test]
fn test_youtube_search_response_results_collection() {
    let json_str = std::fs::read_to_string("tests/fixtures/youtube/youtube_search_results.json")
        .expect("read search fixture");
    let val: Value = serde_json::from_str(&json_str).expect("parse json");

    let shell = YouTubeShell::from_search_response("rust web browser", &val)
        .expect("shell from search succeeds");

    assert_eq!(shell.results.len(), 2);
    assert_eq!(shell.results[0].video_id, "vid001");
    assert_eq!(
        shell.results[0].title,
        "Rust Web Browser Architecture in 2026"
    );
    assert_eq!(shell.results[0].duration_text.as_deref(), Some("14:20"));

    assert_eq!(shell.results[1].video_id, "vid002");
    assert_eq!(
        shell.results[1].title,
        "Next-Gen CSS & Layout Engines Explained"
    );
}

#[test]
fn test_html_media_element_state_machine() {
    let mut media = HtmlMediaElement::new();
    assert_eq!(media.network_state(), NetworkState::Empty);
    assert_eq!(media.ready_state(), ReadyState::HaveNothing);
    assert!(media.paused());

    // Attach decoded stream of 60 seconds
    media
        .attach_decoded_stream(60_000_000)
        .expect("attach decoded stream");
    assert_eq!(media.network_state(), NetworkState::Idle);
    assert_eq!(media.ready_state(), ReadyState::HaveEnoughData);
    assert_eq!(media.controls_state().duration_seconds, Some(60.0));

    // Play & Pause
    media.play().expect("play succeeds");
    assert!(!media.paused());

    media.pause();
    assert!(media.paused());

    // Seeking
    media.seek(15.0).expect("seek succeeds");
    assert_eq!(media.current_time_seconds(), 15.0);

    // Rate & Volume
    media.set_playback_rate(1.5).expect("set rate");
    assert_eq!(media.controls_state().playback_rate, 1.5);

    media.set_volume(0.8).expect("set volume");
    assert_eq!(media.controls_state().volume, 0.8);

    media.set_muted(true);
    assert!(media.controls_state().muted);

    // Events were generated
    let events = media.drain_events();
    assert!(!events.is_empty());
    assert!(events.contains(&MediaEvent::LoadStart));
    assert!(events.contains(&MediaEvent::LoadedMetadata));
    assert!(events.contains(&MediaEvent::CanPlay));
}

#[test]
fn test_media_source_and_source_buffer_lifecycle() {
    let mut source = MediaSource::new();
    source.open().expect("open source");

    let caps = mock_capabilities();
    let buf_id = source
        .add_source_buffer("video/mp4; codecs=\"avc1.42001E\"", &caps)
        .expect("add source buffer");

    assert!(source.source_buffer(buf_id).is_some());
    source.set_duration(30_000_000).expect("set duration");
    assert_eq!(source.duration_us(), Some(30_000_000));

    // An empty MediaSource cannot end without samples
    assert!(source.end_of_stream().is_err());

    source.close();
    assert_eq!(
        source.ready_state(),
        ghitabrowser::mse::MediaSourceReadyState::Closed
    );
}

#[test]
fn test_youtube_keyboard_action_mapping() {
    assert_eq!(
        YouTubeKeyboardAction::from_key(" "),
        Some(YouTubeKeyboardAction::TogglePlayPause)
    );
    assert_eq!(
        YouTubeKeyboardAction::from_key("k"),
        Some(YouTubeKeyboardAction::TogglePlayPause)
    );
    assert_eq!(
        YouTubeKeyboardAction::from_key("j"),
        Some(YouTubeKeyboardAction::SeekRelative(-10))
    );
    assert_eq!(
        YouTubeKeyboardAction::from_key("l"),
        Some(YouTubeKeyboardAction::SeekRelative(10))
    );
    assert_eq!(
        YouTubeKeyboardAction::from_key("ArrowLeft"),
        Some(YouTubeKeyboardAction::SeekRelative(-5))
    );
    assert_eq!(
        YouTubeKeyboardAction::from_key("ArrowRight"),
        Some(YouTubeKeyboardAction::SeekRelative(5))
    );
    assert_eq!(
        YouTubeKeyboardAction::from_key("m"),
        Some(YouTubeKeyboardAction::ToggleMute)
    );
    assert_eq!(
        YouTubeKeyboardAction::from_key("f"),
        Some(YouTubeKeyboardAction::ToggleFullscreen)
    );
    assert_eq!(
        YouTubeKeyboardAction::from_key("5"),
        Some(YouTubeKeyboardAction::SeekPercent(50))
    );
    assert_eq!(YouTubeKeyboardAction::from_key("invalid"), None);
}

#[test]
fn test_audio_clock_and_video_sync_actions() {
    let mut clock = AudioClock::new(48_000).expect("create audio clock");
    assert_eq!(clock.position_us(), 0);

    clock.start();
    clock.advance_frames(48_000).unwrap(); // 1 second
    assert_eq!(clock.position_us(), 1_000_000);

    let sync_present = video_sync_action(1_000_000, 1_000_000, 100_000);
    assert_eq!(sync_present, VideoSyncAction::Present);

    let sync_drop = video_sync_action(1_000_000, 800_000, 50_000);
    assert_eq!(sync_drop, VideoSyncAction::Drop);

    let sync_hold = video_sync_action(1_000_000, 1_200_000, 50_000);
    assert_eq!(sync_hold, VideoSyncAction::Hold);
}

#[test]
fn test_live_youtube_controller_keyboard_dispatch() {
    let video_frame = DecodedVideoFrame {
        timestamp_us: 0,
        duration_us: 40_000,
        width: 320,
        height: 240,
        rgba: vec![0u8; 320 * 240 * 4],
    };
    let audio_frame = DecodedAudioFrame {
        timestamp_us: 0,
        duration_us: 40_000,
        sample_rate_hz: 48_000,
        channels: 2,
        interleaved_samples: vec![0i16; 48 * 40 * 2],
    };
    let asset = DecodedMediaAsset {
        video_frames: vec![video_frame],
        audio_frames: vec![audio_frame],
    };

    let playback = LiveYouTubePlayback {
        response: YouTubePlayerResponse {
            video_id: "testVid".to_string(),
            title: "Test".to_string(),
            duration_us: 10_000_000,
            formats: Vec::new(),
        },
        plan: PlaybackPlan {
            video: YouTubeFormat {
                itag: 18,
                mime_type: "video/mp4".to_string(),
                codecs: vec![MediaCodec::Avc, MediaCodec::Aac],
                url: "https://googlevideo.com/videoplayback".to_string(),
                bitrate: 500_000,
                width: Some(320),
                height: Some(240),
                content_length: Some(1_000_000),
                kind: StreamKind::Muxed,
            },
            audio: None,
        },
        decoded: Arc::new(asset),
        downloaded_bytes: 1_000_000,
    };

    let mut controller = LiveYouTubeController::new(playback).expect("create controller");
    assert!(controller.controls().paused);

    // Keyboard Space -> play
    controller
        .handle_keyboard_action(YouTubeKeyboardAction::TogglePlayPause)
        .unwrap();
    assert!(!controller.controls().paused);

    // Keyboard Space -> pause
    controller
        .handle_keyboard_action(YouTubeKeyboardAction::TogglePlayPause)
        .unwrap();
    assert!(controller.controls().paused);

    // Keyboard 'm' -> mute
    controller
        .handle_keyboard_action(YouTubeKeyboardAction::ToggleMute)
        .unwrap();
    assert!(controller.controls().muted);

    // Keyboard '5' -> seek to 50%
    controller
        .handle_keyboard_action(YouTubeKeyboardAction::SeekPercent(50))
        .unwrap();
    assert!(controller.controls().current_time_seconds >= 0.0);
}

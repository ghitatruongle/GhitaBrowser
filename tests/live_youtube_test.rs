//! Live black-box network/player gate
//! This target is ignored in the normal offline suite because it intentionally
//! contacts the current YouTube service. The release Phase 17 gate runs it
//! explicitly with `--ignored` and records the exact date/network result.

use ghitabrowser::network_scheduler::CancellationToken;
use ghitabrowser::youtube::{
    fetch_live_youtube_search, prepare_live_youtube_playback, LiveYouTubeController,
};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires live YouTube and Googlevideo access"]
async fn current_youtube_direct_stream_drives_controls_av_and_recovery() {
    let prepared = prepare_live_youtube_playback("dQw4w9WgXcQ", CancellationToken::default())
        .await
        .expect("current official player response and bounded media download");
    assert_eq!(prepared.response.video_id, "dQw4w9WgXcQ");
    assert!(prepared.downloaded_bytes > 1_000_000);
    assert!(prepared.downloaded_bytes <= 50 * 1024 * 1024);
    assert!(prepared
        .plan
        .video
        .url
        .contains(".googlevideo.com/videoplayback"));
    assert!(prepared.plan.audio.is_some() || prepared.plan.is_muxed());

    let expected_duration_seconds = prepared.response.duration_us as f64 / 1_000_000.0;
    let mut controller = LiveYouTubeController::new(prepared).expect("live controller");
    let decoded_duration_seconds = controller
        .controls()
        .duration_seconds
        .expect("decoded duration");
    assert!(
        decoded_duration_seconds >= expected_duration_seconds - 1.0,
        "decoded output must cover the current media duration: decoded={decoded_duration_seconds}, expected={expected_duration_seconds}"
    );
    assert!(controller.toggle_playback().expect("play"));
    let mut video_presented = false;
    let mut audio_frames = 0usize;
    for _ in 0..300 {
        let tick = controller.tick(33).expect("clocked playback tick");
        video_presented |= tick.video_frame_presented;
        audio_frames = audio_frames.saturating_add(controller.drain_audio_frames().len());
        if video_presented && audio_frames > 0 {
            break;
        }
    }
    assert!(video_presented, "decoded video must reach the live output");
    assert!(audio_frames > 0, "decoded PCM must reach the live output");

    controller.set_volume(0.25).expect("volume");
    controller.toggle_mute();
    assert!(controller.controls().muted);
    controller.toggle_mute();
    controller.seek_to(2.0).expect("seek");
    controller
        .recover_after_interruption()
        .expect("bounded interruption recovery");
    assert!((controller.controls().current_time_seconds - 2.0).abs() < 0.01);
    assert!(!controller.toggle_playback().expect("pause"));
    assert!(controller.toggle_playback().expect("resume"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires live YouTube search access"]
async fn current_youtube_search_returns_navigable_watch_results() {
    let shell =
        fetch_live_youtube_search("rust programming language", CancellationToken::default())
            .await
            .expect("current official search response");
    assert!(
        !shell.results.is_empty(),
        "search must expose watch results"
    );
    assert!(shell.results.iter().all(|result| {
        result.video_id.len() == 11
            && result
                .video_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            && !result.title.trim().is_empty()
    }));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires live YouTube network adapter"]
async fn cancelled_live_player_request_fails_closed() {
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let error = prepare_live_youtube_playback("dQw4w9WgXcQ", cancellation)
        .await
        .expect_err("cancelled player request must not continue");
    assert!(error.to_ascii_lowercase().contains("cancel"));
}

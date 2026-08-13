use std::time::Instant;

use ghitabrowser::html_media::MediaControlAction;
use ghitabrowser::media_backend::DecodedMediaAsset;
use ghitabrowser::media_core::{DecodedAudioFrame, DecodedVideoFrame};
use ghitabrowser::media_runtime::{MediaRuntimeLimits, PageMediaRuntime};
use ghitabrowser::performance::Profiler;
use ghitabrowser::runtime_core::{RuntimeLimits, RuntimeRealm};
use ghitabrowser::Browser;

fn stress_asset() -> DecodedMediaAsset {
    let mut video_frames = Vec::new();
    let mut audio_frames = Vec::new();
    for index in 0..120_i64 {
        video_frames.push(DecodedVideoFrame {
            timestamp_us: index * 16_667,
            duration_us: 16_667,
            width: 32,
            height: 32,
            rgba: vec![index as u8; 32 * 32 * 4],
        });
        audio_frames.push(DecodedAudioFrame {
            timestamp_us: index * 20_000,
            duration_us: 20_000,
            sample_rate_hz: 48_000,
            channels: 2,
            interleaved_samples: vec![0; 1_920],
        });
    }
    DecodedMediaAsset {
        video_frames,
        audio_frames,
    }
}

#[test]
fn navigation_and_twenty_tab_media_soak_have_bounded_p95_and_clean_teardown() {
    let mut profiler = Profiler::new();
    let mut browser = Browser::new_in_memory();
    let mut navigation_peak_bytes = 0_usize;
    for index in 0..1_000 {
        let started = Instant::now();
        browser
            .load_html(
                &format!("https://soak.test/{index}"),
                &format!(
                    "<html><head><title>Soak {index}</title></head><body><main><h1>{index}</h1><p>{}</p></main></body></html>",
                    "bounded navigation ".repeat(32)
                ),
            )
            .unwrap();
        profiler.record("navigation", started.elapsed().as_millis() as u64);
        navigation_peak_bytes = navigation_peak_bytes.max(browser.estimate_memory().total_bytes);
    }
    let navigation = profiler.snapshot("navigation").unwrap();
    assert_eq!(navigation.sample_count, 512);
    assert!(
        navigation.p95_ms <= 250,
        "navigation p95 regressed: {navigation:?}"
    );
    assert!(navigation_peak_bytes < 64 * 1024 * 1024);
    assert!(browser.active_tab().unwrap().history_len() <= 60);

    let limits = MediaRuntimeLimits {
        max_decoded_bytes: 16 * 1024 * 1024,
        ..MediaRuntimeLimits::default()
    };
    let mut pages = Vec::new();
    let mut element_ids = Vec::new();
    for page_id in 0..20_u64 {
        let realm = RuntimeRealm::new(10_000 + page_id, RuntimeLimits::default()).unwrap();
        let mut page = PageMediaRuntime::new(realm, limits);
        let element = page.create_media_element().unwrap();
        page.attach_decoded_output(element.id, stress_asset())
            .unwrap();
        page.apply_control(element.id, MediaControlAction::TogglePlayback)
            .unwrap();
        element_ids.push(element.id);
        pages.push(page);
    }
    let peak_media_bytes = pages
        .iter()
        .zip(&element_ids)
        .map(|(page, id)| page.output(*id).unwrap().queued_bytes())
        .sum::<usize>();
    assert!(peak_media_bytes < 64 * 1024 * 1024);

    for _frame in 0..120 {
        let started = Instant::now();
        for (page, element_id) in pages.iter_mut().zip(&element_ids) {
            page.tick(*element_id, 17).unwrap();
        }
        profiler.record(
            "twenty-tab-media-frame",
            started.elapsed().as_millis() as u64,
        );
    }
    let media = profiler.snapshot("twenty-tab-media-frame").unwrap();
    assert_eq!(media.sample_count, 120);
    assert!(media.p95_ms <= 32, "media frame p95 regressed: {media:?}");

    for page in &mut pages {
        page.teardown();
        assert_eq!(page.live_binding_count(), 0);
    }
    assert!(pages
        .iter()
        .zip(&element_ids)
        .all(|(page, id)| page.output(*id).is_none()));
}

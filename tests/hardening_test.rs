use std::panic::{catch_unwind, AssertUnwindSafe};

use ghitabrowser::css_parser::parse_css;
use ghitabrowser::document::prepare_document;
use ghitabrowser::iso_bmff::{parse_init_segment, parse_media_segment};
use ghitabrowser::media_backend::DecodedMediaAsset;
use ghitabrowser::media_core::{DecodedAudioFrame, DecodedVideoFrame};
use ghitabrowser::media_runtime::{MediaRuntimeLimits, PageMediaRuntime};
use ghitabrowser::pdf;
use ghitabrowser::runtime_core::{RuntimeLimits, RuntimeRealm};

fn deterministic_bytes(seed: &mut u64, length: usize) -> Vec<u8> {
    (0..length)
        .map(|_| {
            *seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (*seed >> 56) as u8
        })
        .collect()
}

fn small_decoded_asset(index: i64) -> DecodedMediaAsset {
    DecodedMediaAsset {
        video_frames: vec![DecodedVideoFrame {
            timestamp_us: index * 40_000,
            duration_us: 40_000,
            width: 64,
            height: 64,
            rgba: vec![index as u8; 64 * 64 * 4],
        }],
        audio_frames: vec![DecodedAudioFrame {
            timestamp_us: index * 20_000,
            duration_us: 20_000,
            sample_rate_hz: 48_000,
            channels: 2,
            interleaved_samples: vec![0; 1_920],
        }],
    }
}

#[test]
fn deterministic_parser_corpus_never_panics_or_escapes_budgets() {
    let mut seed = 0x4748_4954_415f_3230;
    for case in 0..512usize {
        let bytes = deterministic_bytes(&mut seed, case % 2_048);
        let text = String::from_utf8_lossy(&bytes);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = parse_init_segment(&bytes);
            let _ = parse_media_segment(&bytes, &[]);
            let _ = pdf::parse(&bytes, "fuzz.pdf");
            let _ = parse_css(&text);
            let prepared = prepare_document(&text, "fuzz", &[], 1_280, 720);
            assert!(prepared.stats.dom_nodes <= 250_000);
            assert!(prepared.stats.layout_nodes <= 250_000);
        }));
        assert!(result.is_ok(), "parser corpus case {case} panicked");
    }
}

#[test]
fn twenty_page_media_outputs_remain_bounded_and_teardown_cleanly() {
    let realm = RuntimeRealm::new(18, RuntimeLimits::default()).unwrap();
    let mut runtime = PageMediaRuntime::new(realm, MediaRuntimeLimits::default());
    let mut ids = Vec::new();
    for index in 0..20 {
        let element = runtime.create_media_element().unwrap();
        runtime
            .attach_decoded_output(element.id, small_decoded_asset(index))
            .unwrap();
        ids.push(element.id);
    }
    let queued_bytes = ids
        .iter()
        .map(|id| runtime.output(*id).unwrap().queued_bytes())
        .sum::<usize>();
    assert!(queued_bytes < 2 * 1024 * 1024);
    assert_eq!(runtime.live_binding_count(), 20);
    runtime.teardown();
    assert_eq!(runtime.live_binding_count(), 0);
}

#[test]
fn deterministic_runtime_fuzz_never_panics_or_hangs() {
    // Phase 21 expansion: interpreter, host bridge and module graph must
    // fail closed on arbitrary input without panicking or escaping budgets.
    let mut seed = 0x5032_315f_4655_5a5a;
    for case in 0..256usize {
        let bytes = deterministic_bytes(&mut seed, case % 4_096);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut engine = ghitabrowser::javascript::JsvEngine::new();
            let _ = engine.eval(&text);
            let mut page = ghitabrowser::web_runtime::PageRuntime::from_html(
                &format!(
                    "<main><p>{}</p><script>{}</script></main>",
                    &text[..text.len().min(512)],
                    text
                ),
                Vec::new(),
                800,
                "https://fuzz.test/",
            )
            .expect("page runtime construction must succeed");
            let _ = page.run_document();
            let _ = page.dom_element();
            let _ = page.flush_pending();
        }));
        assert!(result.is_ok(), "runtime fuzz case {case} panicked");
    }
}

#[test]
fn malformed_modules_and_json_fail_closed_without_panics() {
    let mut seed = 0x4d4f_4455_4c45_5353;
    for case in 0..128usize {
        let bytes = deterministic_bytes(&mut seed, case % 1_024);
        let source = String::from_utf8_lossy(&bytes).into_owned();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut engine = ghitabrowser::javascript::JsvEngine::new();
            let _ = engine.modules.register("fuzz", &source);
            let _ = engine.modules.evaluate("fuzz");
            let mut page = ghitabrowser::web_runtime::PageRuntime::from_html(
                "<main><script>let e='';import('fuzz').catch(x=>{e=String(x)});</script></main>",
                Vec::new(),
                800,
                "https://fuzz.test/",
            )
            .expect("page runtime construction must succeed");
            let _ = page.register_module("fuzz", &source);
            let _ = page.run_document();
            let _ = page.flush_pending();
            // JSON parser must fail closed on arbitrary bytes.
            let mut json_engine = ghitabrowser::javascript::JsvEngine::new();
            let _ = json_engine.eval(&format!(
                "JSON.parse({:?})",
                &source[..source.len().min(256)]
            ));
        }));
        assert!(result.is_ok(), "module fuzz case {case} panicked");
    }
}

#[test]
fn media_source_append_fuzz_stays_bounded_and_teardown_cleanly() {
    // Arbitrary bytes appended to a SourceBuffer must never panic; the byte
    // budget and live bindings must stay bounded and teardown must clear.
    let mut seed = 0x4d53_455f_4655_5a5a;
    let capabilities = ghitabrowser::media_backend::merged_capabilities(
        &ghitabrowser::media_backend::WindowsMediaFoundationBackend,
        &ghitabrowser::media_backend::FallbackRegistry::default(),
    );
    let mut source = ghitabrowser::mse::MediaSource::new();
    source.open().unwrap();
    let buffer = source
        .add_source_buffer("video/mp4; codecs=\"avc1\"", &capabilities)
        .unwrap();
    for case in 0..64usize {
        let bytes = deterministic_bytes(&mut seed, case % 8_192);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = source.append_buffer(buffer, &bytes);
        }));
        assert!(result.is_ok(), "mse append fuzz case {case} panicked");
        let total = source
            .source_buffer(buffer)
            .map(ghitabrowser::mse::SourceBuffer::queued_bytes)
            .unwrap_or(0);
        // The MSE module caps total queued bytes at 32 MiB (private to the
        // module); verify a loose safety bound holds through the public API.
        assert!(total <= 32 * 1024 * 1024);
    }
    source.close();
    assert!(
        source.source_buffer(buffer).is_none(),
        "close clears buffers"
    );
}

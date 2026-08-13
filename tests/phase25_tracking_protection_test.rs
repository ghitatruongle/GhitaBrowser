// Integration tests for Phase 25 — Tracking Protection & Canvas Anti-Fingerprinting

use ghitabrowser::tracking_protection::{
    CanvasFingerprintProtector, CookiePolicy, ThirdPartyCookieBlocker, UserAgentMasker,
};
use ghitabrowser::{javascript::JsvValue, web_runtime::PageRuntime};

#[test]
fn third_party_cookie_blocking_isolation() {
    let blocker = ThirdPartyCookieBlocker::new(CookiePolicy::BlockThirdParty);

    // First party cookie allowed
    assert!(blocker.should_allow_cookie("https://shop.example.com", "example.com"));

    // Cross-site third party cookie blocked
    assert!(!blocker.should_allow_cookie("https://shop.example.com", "doubleclick.net"));
}

#[test]
fn canvas_fingerprint_noise_injection_preserves_alpha() {
    let mut protector = CanvasFingerprintProtector::new(true);

    // Create 100 RGBA pixels
    let mut buffer = vec![128u8; 400];
    for i in 0..100 {
        buffer[i * 4 + 3] = 255; // Alpha = 255
    }

    protector.scramble_pixel_buffer(&mut buffer);

    // Alpha channels remain exactly 255
    for i in 0..100 {
        assert_eq!(buffer[i * 4 + 3], 255);
    }
}

#[test]
fn user_agent_masker_returns_uniform_agent() {
    let ua = UserAgentMasker::get_masked_user_agent();
    assert!(ua.contains("GhitaBrowser"));
    assert!(!ua.contains("Chrome/"));
    assert!(!ua.contains("AppleWebKit/"));
}

fn canvas_readback(origin: &str) -> (Vec<u8>, usize) {
    let mut page = PageRuntime::from_html(
        "<canvas id='canvas'></canvas><script>\
         let context=document.getElementById('canvas').getContext('2d');\
         context.fillStyle='#ff0000';context.fillRect(0,0,4,4);\
         let image=context.getImageData(0,0,4,4);let pixels=image.data;</script>",
        Vec::new(),
        800,
        origin,
    )
    .expect("runtime");
    page.run_document().expect("canvas script");
    let pixels = page.evaluate("pixels").expect("pixel buffer");
    let JsvValue::Array(values) = pixels else {
        panic!("getImageData.data must be an array")
    };
    let bytes = values
        .borrow()
        .iter()
        .map(|value| match value {
            JsvValue::Number(number) => *number as u8,
            _ => panic!("pixel must be numeric"),
        })
        .collect();
    (bytes, page.report().canvas_readbacks)
}

#[test]
fn canvas_readback_is_bounded_and_partitioned_by_origin() {
    let (first, count) = canvas_readback("https://canvas-a.test/");
    let (same_origin, _) = canvas_readback("https://canvas-a.test/other");
    let (different_origin, _) = canvas_readback("https://canvas-b.test/");

    assert_eq!(count, 1);
    assert_eq!(
        first, same_origin,
        "noise must remain stable within an origin"
    );
    assert_ne!(
        first, different_origin,
        "different origins must not share a canvas fingerprint"
    );
    assert_eq!(first.len(), 4 * 4 * 4);
    for alpha in first.iter().skip(3).step_by(4) {
        assert_eq!(*alpha, 255);
    }
}

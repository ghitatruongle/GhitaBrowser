use ghitabrowser::adblock::{AdBlockConfig, AdBlocker, ResourceType};

#[test]
fn ten_thousand_decisions_are_bounded() {
    let mut blocker = AdBlocker::new(AdBlockConfig::default());
    let started = std::time::Instant::now();
    for index in 0..10_000 {
        let url = format!("https://cdn{index}.other.test/assets/app.js");
        let _ = blocker.evaluate_resource(&url, Some("https://site.test"), ResourceType::Script);
    }
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "10,000 decisions took {:?}",
        started.elapsed()
    );
    assert_eq!(blocker.stats().evaluated_count, 10_000);
}

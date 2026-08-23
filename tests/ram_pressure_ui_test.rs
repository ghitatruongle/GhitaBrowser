use ghitabrowser::memory_tracker::{MemoryPressureLevel, MemoryReliefReport};

#[test]
fn memory_relief_status_is_quiet_normally_and_explains_actions() {
    let normal = MemoryReliefReport {
        level: MemoryPressureLevel::Normal,
        ..MemoryReliefReport::default()
    };
    assert!(ghitabrowser::ui::format_memory_relief_status(&normal).is_empty());

    let critical = MemoryReliefReport {
        level: MemoryPressureLevel::Critical,
        before_bytes: 600 * 1024 * 1024,
        after_bytes: 450 * 1024 * 1024,
        slept_tabs: vec![2, 3],
        discarded_tabs: vec![4],
        ..MemoryReliefReport::default()
    };
    let status = ghitabrowser::ui::format_memory_relief_status(&critical);
    assert!(status.contains("slept 2"));
    assert!(status.contains("discarded 1"));
    assert!(!status.contains("http"));
}

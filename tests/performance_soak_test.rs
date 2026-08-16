use std::time::Duration;

use ghitabrowser::acceptance::{
    current_process_working_set_bytes, PerformanceSoakTracker, SoakSample,
};

#[test]
fn performance_tracker_starts_without_invented_measurements() {
    let tracker = PerformanceSoakTracker::default();
    assert!(tracker.summary().is_none());
    assert!(tracker.cold_start_samples_ms.is_empty());
    assert_eq!(tracker.navigation_count, 0);
}

#[test]
fn measured_samples_produce_real_percentiles_and_peak_working_set() {
    let mut tracker = PerformanceSoakTracker::default();
    for value in [100, 120, 150, 180, 200] {
        tracker.record_cold_start(Duration::from_millis(value));
        tracker.record_warm_start(Duration::from_millis(value / 4));
    }
    for (index, latency) in [10, 20, 30, 40, 500].into_iter().enumerate() {
        tracker
            .add_sample(SoakSample {
                workload: "worker".into(),
                iteration: index as u64,
                tab_count: 50,
                working_set_bytes: (200 + index as u64) * 1024 * 1024,
                frame_time_micros: 16_000,
                latency_micros: latency,
            })
            .unwrap();
    }
    let summary = tracker.summary().unwrap();
    assert_eq!(summary.cold_start_p95_ms, 180);
    assert_eq!(summary.warm_start_p95_ms, 45);
    assert_eq!(summary.working_set_peak_mb, 204);
    assert_eq!(summary.worker_latency_p50_micros, 30);
    assert_eq!(summary.worker_latency_p95_micros, 40);
    assert_eq!(summary.worker_latency_p99_micros, 40);
}

#[test]
fn live_operation_timer_and_windows_working_set_are_observed_not_estimated() {
    let mut tracker = PerformanceSoakTracker::default();
    let result = tracker.measure_operation("real-operation", || (0..10_000).sum::<u64>());
    assert_eq!(result, 49_995_000);
    let measured = current_process_working_set_bytes().unwrap();
    assert!(measured > 0);
    assert_eq!(tracker.samples.len(), 1);
    assert!(tracker.samples[0].working_set_bytes > 0);
}

#[test]
fn sample_and_counter_budgets_are_explicit() {
    let mut tracker = PerformanceSoakTracker::default();
    assert!(tracker
        .add_sample(SoakSample {
            workload: String::new(),
            iteration: 0,
            tab_count: 0,
            working_set_bytes: 0,
            frame_time_micros: 0,
            latency_micros: 0,
        })
        .is_err());
    for _ in 0..500 {
        tracker.record_navigation();
    }
    tracker.record_media_minutes(30);
    tracker.record_download_bytes(100 * 1024 * 1024);
    assert_eq!(tracker.navigation_count, 500);
    assert_eq!(tracker.media_minutes, 30);
    assert_eq!(tracker.download_bytes, 100 * 1024 * 1024);
}

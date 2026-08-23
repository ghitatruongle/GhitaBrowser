use ghitabrowser::release_metrics::validate_build_metrics;

#[test]
fn release_runner_metrics_have_the_required_budget_fields() {
    let valid = r#"{
      "schema_version":1,
      "tier":"release",
      "passed":true,
      "started_at_utc":"2026-08-16T00:00:00Z",
      "elapsed_seconds":1.2,
      "target_directory":"E:\\GhitaBrowser\\target",
      "target_bytes":1024,
      "debug_bytes":512,
      "free_bytes_before":7000000000,
      "free_bytes_after":6999990000,
      "commands":[{"name":"Formatting","passed":true,"elapsed_seconds":0.2}]
    }"#;
    validate_build_metrics(valid).unwrap();

    let missing_debug_budget = valid.replace("\"debug_bytes\":512,", "");
    assert!(validate_build_metrics(&missing_debug_budget).is_err());
}

//! Schema validation for build/test metrics emitted by `tools/test.ps1`.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BuildMetrics {
    schema_version: u32,
    tier: String,
    passed: bool,
    started_at_utc: String,
    elapsed_seconds: f64,
    target_directory: String,
    target_bytes: u64,
    debug_bytes: u64,
    free_bytes_before: u64,
    free_bytes_after: u64,
    commands: Vec<CommandMetric>,
}

#[derive(Debug, Deserialize)]
struct CommandMetric {
    name: String,
    passed: bool,
    elapsed_seconds: f64,
}

pub fn validate_build_metrics(json: &str) -> Result<(), String> {
    let metrics: BuildMetrics = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if metrics.schema_version != 1 {
        return Err("unsupported build metrics schema".to_string());
    }
    if !matches!(metrics.tier.as_str(), "fast" | "release" | "full") {
        return Err("invalid build metrics tier".to_string());
    }
    if metrics.started_at_utc.trim().is_empty()
        || metrics.target_directory.trim().is_empty()
        || !metrics.elapsed_seconds.is_finite()
        || metrics.elapsed_seconds < 0.0
    {
        return Err("invalid build metrics values".to_string());
    }
    if metrics.commands.is_empty() {
        return Err("build metrics contain no commands".to_string());
    }
    if metrics.commands.iter().any(|command| {
        command.name.trim().is_empty()
            || !command.elapsed_seconds.is_finite()
            || command.elapsed_seconds < 0.0
    }) {
        return Err("invalid command metrics".to_string());
    }
    if metrics.passed && metrics.commands.iter().any(|command| !command.passed) {
        return Err("passing run contains a failed command".to_string());
    }

    // Reading the budget fields is intentional: serde already verifies their
    // type/presence, and this prevents accidental removal as unused fields.
    let _budget_snapshot = metrics
        .target_bytes
        .saturating_add(metrics.debug_bytes)
        .saturating_add(metrics.free_bytes_before)
        .saturating_add(metrics.free_bytes_after);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_metrics_require_budget_fields() {
        let json = r#"{
          "schema_version":1,"tier":"fast","passed":true,
          "started_at_utc":"2026-08-16T00:00:00Z","elapsed_seconds":1.2,
          "target_directory":"E:\\GhitaBrowser\\target",
          "target_bytes":1024,"debug_bytes":512,
          "free_bytes_before":7000000000,"free_bytes_after":6999990000,
          "commands":[{"name":"Formatting","passed":true,"elapsed_seconds":0.2}]
        }"#;
        validate_build_metrics(json).unwrap();
    }

    #[test]
    fn passing_metrics_reject_failed_commands() {
        let json = r#"{
          "schema_version":1,"tier":"fast","passed":true,
          "started_at_utc":"2026-08-16T00:00:00Z","elapsed_seconds":1.2,
          "target_directory":"target","target_bytes":1,"debug_bytes":1,
          "free_bytes_before":2,"free_bytes_after":1,
          "commands":[{"name":"tests","passed":false,"elapsed_seconds":0.2}]
        }"#;
        assert!(validate_build_metrics(json).is_err());
    }
}

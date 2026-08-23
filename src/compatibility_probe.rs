//! Manual live-site compatibility probe used by the personal release gate.
//! Live URLs are intentionally kept out of deterministic unit tests.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::compatibility_diagnostics::{
    build_compatibility_report, evaluate_compatibility_outcome, redact_sensitive_url,
    summarize_compatibility, CompatibilityOutcome, CompatibilityProbeSafeError,
    CompatibilityReport, CompatibilityStatus, CompatibilitySummary,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityProbeResult {
    pub report: CompatibilityReport,
    pub outcome: CompatibilityOutcome,
    pub rendered_text_bytes: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityProbeEntry {
    pub url: String,
    pub result: Option<CompatibilityProbeResult>,
    pub outcome: CompatibilityOutcome,
    pub error: Option<CompatibilityProbeSafeError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityProbeRun {
    pub entries: Vec<CompatibilityProbeEntry>,
    pub summary: CompatibilitySummary,
}

#[derive(Debug, Deserialize)]
struct LiveSiteMatrix {
    urls: Vec<String>,
}

fn should_use_readable_fallback(
    run_failed: bool,
    runtime: &crate::web_runtime::RuntimeReport,
    rendered_text: &str,
) -> bool {
    rendered_text.trim().len() >= 128
        && (run_failed || runtime.scripts_failed > 0 || !runtime.errors.is_empty())
}

fn sanitize_probe_report(report: &mut CompatibilityReport) {
    let error_count = report.runtime.runtime_errors.len();
    if error_count > 0 {
        report.runtime.runtime_errors = vec![format!("{error_count} runtime error(s) omitted")];
    }
    if matches!(report.status, CompatibilityStatus::ScriptError { .. }) {
        report.status = CompatibilityStatus::ScriptError {
            error: "runtime error omitted".to_string(),
        };
    }
}

pub async fn probe_url(url: &str) -> Result<CompatibilityProbeResult, String> {
    let started = std::time::Instant::now();
    let fetched =
        crate::network_scheduler::fetch_navigation(url.to_string(), String::new(), 1).await?;
    let mut page =
        crate::web_runtime::PageRuntime::from_html(&fetched.body, Vec::new(), 1_200, &fetched.url)?;
    let run = page.run_document();
    let runtime = page.report_snapshot();
    let render = page.refresh_render().clone();
    let text = render.layout.as_ref().map_or_else(String::new, |layout| {
        crate::text_renderer::TextRenderer::new(1_200, 800).render_to_text(layout)
    });
    let mut report = build_compatibility_report(
        &fetched.url,
        None,
        Some(&runtime),
        render.layout.as_ref(),
        None,
        1_200.0,
        800.0,
    );
    if should_use_readable_fallback(run.is_err(), &runtime, &text) {
        report.status = CompatibilityStatus::DegradedShell {
            reason: "page requires unsupported runtime features".to_string(),
        };
    }
    sanitize_probe_report(&mut report);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let outcome = evaluate_compatibility_outcome(&report, &text, elapsed_ms);
    Ok(CompatibilityProbeResult {
        report,
        outcome,
        rendered_text_bytes: text.len(),
        elapsed_ms,
    })
}

pub async fn probe_manifest(path: &Path) -> Result<CompatibilityProbeRun, String> {
    let raw = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let matrix: LiveSiteMatrix = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    if matrix.urls.is_empty() || matrix.urls.len() > 64 {
        return Err("live-site matrix must contain 1..=64 URLs".to_string());
    }

    let mut entries = Vec::with_capacity(matrix.urls.len());
    for url in matrix.urls {
        let redacted = redact_sensitive_url(&url);
        match probe_url(&url).await {
            Ok(result) => entries.push(CompatibilityProbeEntry {
                url: redacted,
                outcome: result.outcome,
                result: Some(result),
                error: None,
            }),
            Err(error) => {
                let safe_error = CompatibilityProbeSafeError::from_internal_error(&error);
                let outcome = if safe_error == CompatibilityProbeSafeError::TimedOut {
                    CompatibilityOutcome::TimedOut
                } else {
                    CompatibilityOutcome::Broken
                };
                entries.push(CompatibilityProbeEntry {
                    url: redacted,
                    result: None,
                    outcome,
                    error: Some(safe_error),
                });
            }
        }
    }

    let outcomes: Vec<_> = entries.iter().map(|entry| entry.outcome).collect();
    Ok(CompatibilityProbeRun {
        summary: summarize_compatibility(&outcomes),
        entries,
    })
}

/// Handle compatibility probe CLI flags before the GUI starts.
/// Returns `Ok(None)` when the invocation is unrelated to the probe.
pub fn try_run_cli(args: &[String]) -> Result<Option<bool>, String> {
    let matrix = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--compatibility-probe="));
    let report = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--compatibility-report="));
    let (matrix, report) = match (matrix, report) {
        (None, None) => return Ok(None),
        (Some(matrix), Some(report)) => (matrix, report),
        _ => return Err("compatibility probe requires matrix and report paths".to_string()),
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let run = runtime.block_on(probe_manifest(Path::new(matrix)))?;
    let output = Path::new(report);
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_vec_pretty(&run).map_err(|error| error.to_string())?;
    std::fs::write(output, json).map_err(|error| error.to_string())?;
    Ok(Some(run.summary.passed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_ignores_unrelated_arguments() {
        assert_eq!(try_run_cli(&["https://example.test".into()]).unwrap(), None);
    }

    #[test]
    fn cli_requires_both_probe_paths() {
        let error = try_run_cli(&["--compatibility-probe=matrix.json".into()]).unwrap_err();
        assert!(error.contains("requires matrix and report"));
    }

    #[test]
    fn runtime_errors_with_readable_text_become_a_safe_fallback() {
        let raw_marker = "token=must-not-leak";
        let runtime = crate::web_runtime::RuntimeReport {
            scripts_failed: 1,
            errors: vec![raw_marker.to_string()],
            ..Default::default()
        };
        assert!(should_use_readable_fallback(
            false,
            &runtime,
            &"readable article text ".repeat(16)
        ));

        let mut report = build_compatibility_report(
            "https://example.test/?token=secret",
            None,
            Some(&runtime),
            None,
            None,
            1_200.0,
            800.0,
        );
        sanitize_probe_report(&mut report);
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!encoded.contains(raw_marker));
        assert!(!encoded.contains("token=secret"));
    }
}

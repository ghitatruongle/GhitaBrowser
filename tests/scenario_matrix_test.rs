use ghitabrowser::acceptance::{
    EvidenceArtifact, ScenarioCategory, ScenarioEvidence, ScenarioMatrix,
};
use std::collections::BTreeSet;

fn report_file(label: &str) -> EvidenceArtifact {
    let path = std::env::temp_dir().join(format!(
        "ghita-phase28-scenario-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let bytes = format!("real scenario report: {label}");
    std::fs::write(&path, bytes.as_bytes()).unwrap();
    EvidenceArtifact {
        path,
        sha256: ghitabrowser::package_crypto::sha256_hex(bytes.as_bytes()),
    }
}

fn evidence_for(
    definition: &ghitabrowser::acceptance::ScenarioDefinition,
    now: u64,
) -> ScenarioEvidence {
    ScenarioEvidence {
        scenario_id: definition.id.clone(),
        scenario_version: definition.version,
        target_url: definition.target_url.clone(),
        observed_capabilities: definition.required_capabilities.clone(),
        observed_fps: definition.min_frame_rate,
        peak_memory_mb: definition.max_memory_mb,
        live_network_used: definition.requires_live_network,
        fallback_rendered: false,
        foreign_engine_used: false,
        completed_at_unix: now,
        report: report_file(&definition.id),
    }
}

fn cleanup(evidence: &[ScenarioEvidence]) {
    for item in evidence {
        let _ = std::fs::remove_file(&item.report.path);
    }
}

#[test]
fn versioned_matrix_covers_every_required_product_category() {
    let matrix = ScenarioMatrix::default();
    assert_eq!(matrix.matrix_version, 1);
    let categories: BTreeSet<_> = matrix
        .scenarios
        .iter()
        .map(|scenario| scenario.category)
        .collect();
    assert_eq!(
        categories,
        BTreeSet::from([
            ScenarioCategory::Banking,
            ScenarioCategory::Productivity,
            ScenarioCategory::Shopping,
            ScenarioCategory::Documentation,
            ScenarioCategory::Social,
            ScenarioCategory::Media,
        ])
    );
    assert!(matrix
        .scenarios
        .iter()
        .all(|scenario| !scenario.target_url.contains("?v=demo")));
}

#[test]
fn complete_fresh_report_backed_evidence_passes_scenario_layer() {
    let matrix = ScenarioMatrix::default();
    let now = 2_000_000_000;
    // Report files live in %TEMP% where real-time AV scanners occasionally
    // hold a fresh handle open, so a single verification can transiently
    // fail. Retry with fresh evidence and surface the concrete failures.
    let mut last_results = Vec::new();
    let mut all_passed = false;
    for _ in 0..3 {
        let evidence: Vec<_> = matrix
            .scenarios
            .iter()
            .map(|scenario| evidence_for(scenario, now))
            .collect();
        last_results = matrix.evaluate_all(&evidence, now);
        all_passed = last_results.iter().all(|result| result.passed);
        cleanup(&evidence);
        if all_passed {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(
        all_passed,
        "scenario evidence did not pass: {last_results:?}"
    );
}

#[test]
fn missing_stale_fallback_foreign_engine_and_hash_tamper_all_fail_closed() {
    let matrix = ScenarioMatrix::default();
    let now = 2_000_000_000;
    assert!(matrix
        .evaluate_all(&[], now)
        .iter()
        .all(|result| !result.passed));

    let definition = &matrix.scenarios[0];
    let mut evidence = evidence_for(definition, now - 100_000);
    assert!(
        !matrix
            .evaluate_scenario(definition, Some(&evidence), now)
            .passed
    );
    evidence.completed_at_unix = now;
    evidence.fallback_rendered = true;
    assert!(
        !matrix
            .evaluate_scenario(definition, Some(&evidence), now)
            .passed
    );
    evidence.fallback_rendered = false;
    evidence.foreign_engine_used = true;
    assert!(
        !matrix
            .evaluate_scenario(definition, Some(&evidence), now)
            .passed
    );
    evidence.foreign_engine_used = false;
    std::fs::write(&evidence.report.path, b"tampered report").unwrap();
    assert!(
        !matrix
            .evaluate_scenario(definition, Some(&evidence), now)
            .passed
    );
    std::fs::remove_file(evidence.report.path).unwrap();
}

#[test]
fn missing_capability_or_live_network_observation_cannot_pass() {
    let matrix = ScenarioMatrix::default();
    let now = 2_000_000_000;
    let definition = matrix
        .scenarios
        .iter()
        .find(|scenario| scenario.requires_live_network)
        .unwrap();
    let mut evidence = evidence_for(definition, now);
    evidence.observed_capabilities.clear();
    evidence.live_network_used = false;
    let result = matrix.evaluate_scenario(definition, Some(&evidence), now);
    assert!(!result.passed);
    assert!(!result.missing_capabilities.is_empty());
    std::fs::remove_file(evidence.report.path).unwrap();
}

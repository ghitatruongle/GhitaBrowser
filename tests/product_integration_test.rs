use std::collections::BTreeMap;
use std::path::PathBuf;

use ghitabrowser::acceptance::{
    AcceptanceEvidenceBundle, AuditEvidence, EvidenceArtifact, ExternalReleaseEvidence,
    PerformanceSummary,
};
use ghitabrowser::Browser;

fn nonexistent(label: &str) -> EvidenceArtifact {
    EvidenceArtifact {
        path: PathBuf::from(format!(r"Z:\missing-phase28-{label}.json")),
        sha256: "a".repeat(64),
    }
}

fn fabricated_bundle(now: u64) -> AcceptanceEvidenceBundle {
    AcceptanceEvidenceBundle {
        evidence_schema_version: ghitabrowser::acceptance::EVIDENCE_SCHEMA_VERSION,
        matrix_version: 1,
        scenarios: vec![],
        performance: PerformanceSummary {
            cold_start_p95_ms: 100,
            warm_start_p95_ms: 20,
            working_set_peak_mb: 500,
            worker_latency_p50_micros: 100,
            worker_latency_p95_micros: 200,
            worker_latency_p99_micros: 300,
            navigation_count: 500,
            media_minutes: 30,
            download_bytes: 100 * 1024 * 1024,
        },
        audit: AuditEvidence {
            license_audit_passed: true,
            rustsec_passed: true,
            fuzz_passed: true,
            accessibility_passed: true,
            reproducible_build_passed: true,
            source_revision: "abcdef123456".into(),
            lockfile: nonexistent("lockfile"),
            license_report: nonexistent("licenses"),
            rustsec_report: nonexistent("rustsec"),
            fuzz_report: nonexistent("fuzz"),
            accessibility_report: nonexistent("a11y"),
            reproducible_artifact_a: nonexistent("build-a"),
            reproducible_artifact_b: nonexistent("build-b"),
            completed_at_unix: now,
        },
        external: ExternalReleaseEvidence {
            supported_windows_reports: BTreeMap::new(),
            signed_artifact: nonexistent("signed-installer"),
            authenticode_publisher: "Fabricated Publisher".into(),
            approved_certificate_subject: "Fabricated Publisher".into(),
            approved_certificate_thumbprint_sha256: "b".repeat(64),
            clean_vm_report: nonexistent("vm"),
            phase17_live_report: nonexistent("live"),
            completed_at_unix: now,
        },
    }
}

#[test]
fn browser_acceptance_defaults_to_rejected_without_evidence() {
    let mut browser = Browser::new_in_memory();
    let report = browser
        .acceptance
        .evaluate_release_acceptance(None, 2_000_000_000);
    assert!(!report.accepted);
    assert!(report
        .failures
        .iter()
        .any(|failure| failure.contains("missing")));
}

#[test]
fn fabricated_hashes_and_missing_external_artifacts_cannot_release_product() {
    let now = 2_000_000_000;
    let mut browser = Browser::new_in_memory();
    let bundle = fabricated_bundle(now);
    let report = browser
        .acceptance
        .evaluate_release_acceptance(Some(&bundle), now);
    assert!(!report.accepted);
    assert!(report.failures.len() >= 3);
}

#[test]
fn evidence_bundle_with_wrong_schema_version_is_rejected() {
    let now = 2_000_000_000;
    let mut browser = Browser::new_in_memory();
    let mut bundle = fabricated_bundle(now);
    bundle.evidence_schema_version = 1;
    let report = browser
        .acceptance
        .evaluate_release_acceptance(Some(&bundle), now);
    assert!(!report.accepted);
    assert!(report
        .failures
        .iter()
        .any(|failure| failure.contains("schema version")));
}

#[test]
fn signer_binding_rejects_any_identity_other_than_the_approved_certificate() {
    use ghitabrowser::windows_integration::{validate_signer_identity, SignerIdentity};

    let approved = SignerIdentity {
        subject: "GhitaBrowser Release".into(),
        thumbprint_sha256: "ab".repeat(32),
    };
    assert!(validate_signer_identity(&approved, "GhitaBrowser Release", &"ab".repeat(32)).is_ok());
    // A valid signature from an unapproved thumbprint must fail.
    let different_thumbprint = SignerIdentity {
        subject: "GhitaBrowser Release".into(),
        thumbprint_sha256: "cd".repeat(32),
    };
    assert!(validate_signer_identity(
        &different_thumbprint,
        "GhitaBrowser Release",
        &"ab".repeat(32)
    )
    .is_err());
    // An unapproved subject must fail even with a matching thumbprint.
    let different_subject = SignerIdentity {
        subject: "Unapproved Publisher".into(),
        thumbprint_sha256: "ab".repeat(32),
    };
    assert!(
        validate_signer_identity(&different_subject, "GhitaBrowser Release", &"ab".repeat(32))
            .is_err()
    );
    // A missing or malformed approved identity fails closed.
    assert!(validate_signer_identity(&approved, "  ", &"ab".repeat(32)).is_err());
    assert!(validate_signer_identity(&approved, "GhitaBrowser Release", "not-a-hash").is_err());
    assert!(validate_signer_identity(&approved, "GhitaBrowser Release", "").is_err());
}

#[cfg(target_os = "windows")]
#[test]
fn unsigned_file_has_no_approved_signer() {
    use ghitabrowser::windows_integration::verify_signed_executable;

    let root = std::env::temp_dir().join(format!("ghita-phase28-signer-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let unsigned = root.join("unsigned.exe");
    std::fs::write(&unsigned, b"not a signed PE").unwrap();
    assert!(verify_signed_executable(&unsigned, "GhitaBrowser Release", &"ab".repeat(32)).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reproducibility_auditor_compares_actual_artifact_bytes() {
    let root = std::env::temp_dir().join(format!("ghita-phase28-repro-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let first = root.join("first.bin");
    let second = root.join("second.bin");
    std::fs::write(&first, b"identical release bytes").unwrap();
    std::fs::write(&second, b"identical release bytes").unwrap();
    assert!(
        ghitabrowser::acceptance::AcceptanceAuditor::verify_reproducible_artifacts(&first, &second)
            .is_ok()
    );
    std::fs::write(&second, b"different").unwrap();
    assert!(
        ghitabrowser::acceptance::AcceptanceAuditor::verify_reproducible_artifacts(&first, &second)
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

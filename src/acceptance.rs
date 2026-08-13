//! Evidence-driven Phase 28 product acceptance.
//!
//! This module deliberately cannot manufacture a passing release. Scenario,
//! performance, audit, live-site, signed-artifact and clean-VM observations
//! must be supplied by the corresponding runners and remain fresh. Missing or
//! stale evidence fails closed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::package_crypto::sha256_hex;

const MAX_EVIDENCE_AGE_SECONDS: u64 = 24 * 60 * 60;
const MAX_SCENARIO_EVIDENCE: usize = 128;
const MAX_SOAK_SAMPLES: usize = 100_000;

/// Version of the `AcceptanceEvidenceBundle` JSON schema. Version 2 adds the
/// cryptographic signer binding (approved certificate subject and SHA-256
/// thumbprint) and makes the signed distribution envelope explicitly distinct
/// from the reproducible unsigned payload artifacts.
pub const EVIDENCE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioCategory {
    Banking,
    Productivity,
    Shopping,
    Documentation,
    Social,
    Media,
}

impl ScenarioCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Banking => "banking",
            Self::Productivity => "productivity",
            Self::Shopping => "shopping",
            Self::Documentation => "documentation",
            Self::Social => "social",
            Self::Media => "media",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioCapability {
    DomMutations,
    JsEngine,
    StorageQuota,
    ServiceWorker,
    IpcIsolation,
    CssFlexGrid,
    MsePlayback,
    WebSockets,
    Extensions,
    Downloads,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioDefinition {
    pub id: String,
    pub version: u32,
    pub category: ScenarioCategory,
    pub target_url: String,
    pub required_capabilities: BTreeSet<ScenarioCapability>,
    pub min_frame_rate: u32,
    pub max_memory_mb: u64,
    pub requires_live_network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioEvidence {
    pub scenario_id: String,
    pub scenario_version: u32,
    pub target_url: String,
    pub observed_capabilities: BTreeSet<ScenarioCapability>,
    pub observed_fps: u32,
    pub peak_memory_mb: u64,
    pub live_network_used: bool,
    pub fallback_rendered: bool,
    pub foreign_engine_used: bool,
    pub completed_at_unix: u64,
    pub report: EvidenceArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceArtifact {
    pub path: PathBuf,
    pub sha256: String,
}

impl EvidenceArtifact {
    pub fn verify(&self, maximum_bytes: u64) -> Result<(), String> {
        let metadata = std::fs::metadata(&self.path).map_err(|error| {
            format!(
                "evidence artifact is missing ({}): {error}",
                self.path.display()
            )
        })?;
        if !metadata.is_file() || metadata.len() > maximum_bytes {
            return Err(format!(
                "evidence artifact is not a bounded regular file: {}",
                self.path.display()
            ));
        }
        let bytes = std::fs::read(&self.path).map_err(|error| error.to_string())?;
        if !self.sha256.eq_ignore_ascii_case(&sha256_hex(&bytes)) {
            return Err(format!(
                "evidence SHA-256 mismatch: {}",
                self.path.display()
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario_id: String,
    pub category: ScenarioCategory,
    pub passed: bool,
    pub missing_capabilities: Vec<ScenarioCapability>,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioMatrix {
    pub matrix_version: u32,
    pub scenarios: Vec<ScenarioDefinition>,
}

impl Default for ScenarioMatrix {
    fn default() -> Self {
        Self::version_1()
    }
}

impl ScenarioMatrix {
    pub fn version_1() -> Self {
        use ScenarioCapability as Capability;
        let scenarios = vec![
            scenario(
                "banking-auth-form-v1",
                ScenarioCategory::Banking,
                "https://banking.ghita-acceptance.invalid/dashboard",
                [
                    Capability::DomMutations,
                    Capability::JsEngine,
                    Capability::StorageQuota,
                    Capability::IpcIsolation,
                ],
            ),
            scenario(
                "productivity-offline-editor-v1",
                ScenarioCategory::Productivity,
                "https://productivity.ghita-acceptance.invalid/editor",
                [
                    Capability::DomMutations,
                    Capability::JsEngine,
                    Capability::ServiceWorker,
                    Capability::StorageQuota,
                    Capability::WebSockets,
                ],
            ),
            scenario(
                "shopping-checkout-v1",
                ScenarioCategory::Shopping,
                "https://shopping.ghita-acceptance.invalid/checkout",
                [
                    Capability::DomMutations,
                    Capability::JsEngine,
                    Capability::StorageQuota,
                    Capability::IpcIsolation,
                ],
            ),
            scenario(
                "documentation-layout-v1",
                ScenarioCategory::Documentation,
                "https://docs.rust-lang.org/book/",
                [
                    Capability::DomMutations,
                    Capability::CssFlexGrid,
                    Capability::Downloads,
                ],
            ),
            scenario(
                "social-realtime-feed-v1",
                ScenarioCategory::Social,
                "https://social.ghita-acceptance.invalid/feed",
                [
                    Capability::DomMutations,
                    Capability::JsEngine,
                    Capability::WebSockets,
                    Capability::IpcIsolation,
                ],
            ),
            scenario(
                "media-youtube-watch-v1",
                ScenarioCategory::Media,
                "https://www.youtube.com/watch?v=jNQXAC9IVRw",
                [
                    Capability::DomMutations,
                    Capability::JsEngine,
                    Capability::MsePlayback,
                    Capability::IpcIsolation,
                ],
            ),
        ];
        Self {
            matrix_version: 1,
            scenarios,
        }
    }

    pub fn evaluate_scenario(
        &self,
        definition: &ScenarioDefinition,
        evidence: Option<&ScenarioEvidence>,
        now_unix: u64,
    ) -> ScenarioResult {
        let mut failures = Vec::new();
        let Some(evidence) = evidence else {
            return ScenarioResult {
                scenario_id: definition.id.clone(),
                category: definition.category,
                passed: false,
                missing_capabilities: definition.required_capabilities.iter().copied().collect(),
                failures: vec!["missing scenario execution evidence".into()],
            };
        };
        if evidence.scenario_id != definition.id
            || evidence.scenario_version != definition.version
            || evidence.target_url != definition.target_url
        {
            failures.push("evidence does not match the matrix scenario/version/URL".into());
        }
        if evidence.completed_at_unix > now_unix
            || now_unix.saturating_sub(evidence.completed_at_unix) > MAX_EVIDENCE_AGE_SECONDS
        {
            failures.push("scenario evidence is stale or from the future".into());
        }
        if definition.requires_live_network && !evidence.live_network_used {
            failures.push("live-network execution was required".into());
        }
        if evidence.fallback_rendered {
            failures.push("fallback content cannot satisfy acceptance".into());
        }
        if evidence.foreign_engine_used {
            failures.push("foreign engine output cannot satisfy acceptance".into());
        }
        if evidence.observed_fps < definition.min_frame_rate {
            failures.push(format!(
                "observed {} FPS is below {} FPS",
                evidence.observed_fps, definition.min_frame_rate
            ));
        }
        if evidence.peak_memory_mb > definition.max_memory_mb {
            failures.push(format!(
                "observed {} MiB exceeds {} MiB",
                evidence.peak_memory_mb, definition.max_memory_mb
            ));
        }
        if let Err(error) = evidence.report.verify(16 * 1024 * 1024) {
            failures.push(error);
        }
        let missing_capabilities: Vec<_> = definition
            .required_capabilities
            .difference(&evidence.observed_capabilities)
            .copied()
            .collect();
        if !missing_capabilities.is_empty() {
            failures.push("one or more required capabilities were not observed".into());
        }
        ScenarioResult {
            scenario_id: definition.id.clone(),
            category: definition.category,
            passed: failures.is_empty() && missing_capabilities.is_empty(),
            missing_capabilities,
            failures,
        }
    }

    pub fn evaluate_all(
        &self,
        evidence: &[ScenarioEvidence],
        now_unix: u64,
    ) -> Vec<ScenarioResult> {
        if evidence.len() > MAX_SCENARIO_EVIDENCE {
            return self
                .scenarios
                .iter()
                .map(|definition| ScenarioResult {
                    scenario_id: definition.id.clone(),
                    category: definition.category,
                    passed: false,
                    missing_capabilities: Vec::new(),
                    failures: vec!["scenario evidence count exceeds safety limit".into()],
                })
                .collect();
        }
        let by_id: BTreeMap<_, _> = evidence
            .iter()
            .map(|item| (item.scenario_id.as_str(), item))
            .collect();
        self.scenarios
            .iter()
            .map(|definition| {
                self.evaluate_scenario(
                    definition,
                    by_id.get(definition.id.as_str()).copied(),
                    now_unix,
                )
            })
            .collect()
    }
}

fn scenario<const N: usize>(
    id: &str,
    category: ScenarioCategory,
    target_url: &str,
    capabilities: [ScenarioCapability; N],
) -> ScenarioDefinition {
    ScenarioDefinition {
        id: id.into(),
        version: 1,
        category,
        target_url: target_url.into(),
        required_capabilities: capabilities.into_iter().collect(),
        min_frame_rate: if category == ScenarioCategory::Media {
            24
        } else {
            15
        },
        max_memory_mb: if category == ScenarioCategory::Media {
            768
        } else {
            512
        },
        requires_live_network: matches!(
            category,
            ScenarioCategory::Documentation | ScenarioCategory::Media
        ),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoakSample {
    pub workload: String,
    pub iteration: u64,
    pub tab_count: usize,
    pub working_set_bytes: u64,
    pub frame_time_micros: u64,
    pub latency_micros: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceSoakTracker {
    pub cold_start_samples_ms: Vec<u64>,
    pub warm_start_samples_ms: Vec<u64>,
    pub navigation_count: u64,
    pub media_minutes: u64,
    pub download_bytes: u64,
    pub samples: Vec<SoakSample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerformanceSummary {
    pub cold_start_p95_ms: u64,
    pub warm_start_p95_ms: u64,
    pub working_set_peak_mb: u64,
    pub worker_latency_p50_micros: u64,
    pub worker_latency_p95_micros: u64,
    pub worker_latency_p99_micros: u64,
    pub navigation_count: u64,
    pub media_minutes: u64,
    pub download_bytes: u64,
}

impl PerformanceSoakTracker {
    pub fn record_cold_start(&mut self, elapsed: Duration) {
        push_bounded(
            &mut self.cold_start_samples_ms,
            elapsed.as_millis() as u64,
            128,
        );
    }

    pub fn record_warm_start(&mut self, elapsed: Duration) {
        push_bounded(
            &mut self.warm_start_samples_ms,
            elapsed.as_millis() as u64,
            256,
        );
    }

    pub fn add_sample(&mut self, sample: SoakSample) -> Result<(), String> {
        if sample.workload.is_empty() || sample.workload.len() > 128 {
            return Err("soak workload identifier is invalid".into());
        }
        if self.samples.len() >= MAX_SOAK_SAMPLES {
            return Err("soak sample limit exceeded".into());
        }
        self.samples.push(sample);
        Ok(())
    }

    pub fn record_navigation(&mut self) {
        self.navigation_count = self.navigation_count.saturating_add(1);
    }

    pub fn record_media_minutes(&mut self, minutes: u64) {
        self.media_minutes = self.media_minutes.saturating_add(minutes);
    }

    pub fn record_download_bytes(&mut self, bytes: u64) {
        self.download_bytes = self.download_bytes.saturating_add(bytes);
    }

    pub fn measure_operation<T>(&mut self, workload: &str, operation: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let output = operation();
        let _ = self.add_sample(SoakSample {
            workload: workload.to_string(),
            iteration: self.samples.len() as u64,
            tab_count: 0,
            working_set_bytes: current_process_working_set_bytes().unwrap_or(0),
            frame_time_micros: 0,
            latency_micros: start.elapsed().as_micros() as u64,
        });
        output
    }

    pub fn summary(&self) -> Option<PerformanceSummary> {
        if self.cold_start_samples_ms.is_empty()
            || self.warm_start_samples_ms.is_empty()
            || self.samples.is_empty()
        {
            return None;
        }
        let latencies: Vec<_> = self
            .samples
            .iter()
            .map(|sample| sample.latency_micros)
            .collect();
        let working_set_peak = self
            .samples
            .iter()
            .map(|sample| sample.working_set_bytes)
            .max()
            .unwrap_or(0);
        Some(PerformanceSummary {
            cold_start_p95_ms: percentile(&self.cold_start_samples_ms, 95),
            warm_start_p95_ms: percentile(&self.warm_start_samples_ms, 95),
            working_set_peak_mb: working_set_peak.div_ceil(1024 * 1024),
            worker_latency_p50_micros: percentile(&latencies, 50),
            worker_latency_p95_micros: percentile(&latencies, 95),
            worker_latency_p99_micros: percentile(&latencies, 99),
            navigation_count: self.navigation_count,
            media_minutes: self.media_minutes,
            download_bytes: self.download_bytes,
        })
    }
}

fn push_bounded(values: &mut Vec<u64>, value: u64, maximum: usize) {
    if values.len() == maximum {
        values.remove(0);
    }
    values.push(value);
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    let rank = (values.len() - 1) * percentile / 100;
    values[rank]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvidence {
    pub license_audit_passed: bool,
    pub rustsec_passed: bool,
    pub fuzz_passed: bool,
    pub accessibility_passed: bool,
    pub reproducible_build_passed: bool,
    pub source_revision: String,
    pub lockfile: EvidenceArtifact,
    pub license_report: EvidenceArtifact,
    pub rustsec_report: EvidenceArtifact,
    pub fuzz_report: EvidenceArtifact,
    pub accessibility_report: EvidenceArtifact,
    pub reproducible_artifact_a: EvidenceArtifact,
    pub reproducible_artifact_b: EvidenceArtifact,
    pub completed_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalReleaseEvidence {
    pub supported_windows_reports: BTreeMap<String, EvidenceArtifact>,
    pub signed_artifact: EvidenceArtifact,
    pub authenticode_publisher: String,
    /// Exact subject of the user-approved signing certificate. A valid
    /// signature from any other subject must fail acceptance.
    pub approved_certificate_subject: String,
    /// Lowercase hex SHA-256 thumbprint of the user-approved signing
    /// certificate. This is the cryptographic identity binding.
    pub approved_certificate_thumbprint_sha256: String,
    pub clean_vm_report: EvidenceArtifact,
    pub phase17_live_report: EvidenceArtifact,
    pub completed_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceEvidenceBundle {
    pub evidence_schema_version: u32,
    pub matrix_version: u32,
    pub scenarios: Vec<ScenarioEvidence>,
    pub performance: PerformanceSummary,
    pub audit: AuditEvidence,
    pub external: ExternalReleaseEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceReport {
    pub accepted: bool,
    pub scenario_results: Vec<ScenarioResult>,
    pub failures: Vec<String>,
}

pub struct AcceptanceAuditor;

impl AcceptanceAuditor {
    pub fn sha256_file(path: &Path) -> Result<String, String> {
        let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 * 1024 {
            return Err("audit artifact is not a bounded regular file".into());
        }
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        Ok(sha256_hex(&bytes))
    }

    pub fn verify_reproducible_artifacts(first: &Path, second: &Path) -> Result<String, String> {
        let first_hash = Self::sha256_file(first)?;
        let second_hash = Self::sha256_file(second)?;
        if first_hash != second_hash {
            return Err("independent release artifacts are not byte reproducible".into());
        }
        Ok(first_hash)
    }
}

pub struct AcceptanceReleaseManager {
    pub matrix: ScenarioMatrix,
    pub soak_tracker: PerformanceSoakTracker,
    pub last_report: Option<AcceptanceReport>,
}

impl Default for AcceptanceReleaseManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AcceptanceReleaseManager {
    pub fn new() -> Self {
        Self {
            matrix: ScenarioMatrix::default(),
            soak_tracker: PerformanceSoakTracker::default(),
            last_report: None,
        }
    }

    /// Evaluate only supplied evidence. There are no default capabilities,
    /// invented timings or estimated memory values in this path.
    pub fn evaluate_release_acceptance(
        &mut self,
        evidence: Option<&AcceptanceEvidenceBundle>,
        now_unix: u64,
    ) -> AcceptanceReport {
        let Some(evidence) = evidence else {
            let report = AcceptanceReport {
                accepted: false,
                scenario_results: self.matrix.evaluate_all(&[], now_unix),
                failures: vec!["acceptance evidence bundle is missing".into()],
            };
            self.last_report = Some(report.clone());
            return report;
        };
        let scenario_results = self.matrix.evaluate_all(&evidence.scenarios, now_unix);
        let mut failures = Vec::new();
        if evidence.evidence_schema_version != EVIDENCE_SCHEMA_VERSION {
            failures.push(format!(
                "evidence uses schema version {} instead of {}",
                evidence.evidence_schema_version, EVIDENCE_SCHEMA_VERSION
            ));
        }
        if evidence.matrix_version != self.matrix.matrix_version {
            failures.push("evidence uses a different scenario matrix version".into());
        }
        if scenario_results.iter().any(|result| !result.passed) {
            failures.push("one or more representative scenarios failed".into());
        }
        validate_performance(&evidence.performance, &mut failures);
        validate_audit(&evidence.audit, now_unix, &mut failures);
        validate_external(&evidence.external, now_unix, &mut failures);
        let report = AcceptanceReport {
            accepted: failures.is_empty(),
            scenario_results,
            failures,
        };
        self.last_report = Some(report.clone());
        report
    }

    pub fn load_bundle(path: &Path) -> Result<AcceptanceEvidenceBundle, String> {
        let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
            return Err("acceptance bundle must be a regular file up to 16 MiB".into());
        }
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }

    pub fn persist_report(path: &Path, report: &AcceptanceReport) -> Result<PathBuf, String> {
        let bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, bytes).map_err(|error| error.to_string())?;
        Ok(path.to_path_buf())
    }
}

fn validate_performance(summary: &PerformanceSummary, failures: &mut Vec<String>) {
    if summary.cold_start_p95_ms > 2_000 {
        failures.push("cold startup p95 exceeds 2 seconds".into());
    }
    if summary.warm_start_p95_ms > 750 {
        failures.push("warm startup p95 exceeds 750 ms".into());
    }
    if summary.working_set_peak_mb == 0 || summary.working_set_peak_mb > 1_024 {
        failures.push("50-tab measured peak working set is absent or exceeds 1 GiB".into());
    }
    if summary.navigation_count < 500 {
        failures.push("long-navigation soak did not reach 500 navigations".into());
    }
    if summary.media_minutes < 30 {
        failures.push("media soak did not reach 30 minutes".into());
    }
    if summary.download_bytes < 100 * 1024 * 1024 {
        failures.push("download soak did not transfer at least 100 MiB".into());
    }
    if summary.worker_latency_p99_micros == 0 || summary.worker_latency_p99_micros > 100_000 {
        failures.push("background-worker p99 is absent or exceeds 100 ms".into());
    }
}

fn validate_audit(evidence: &AuditEvidence, now_unix: u64, failures: &mut Vec<String>) {
    for (passed, name) in [
        (evidence.license_audit_passed, "license audit"),
        (evidence.rustsec_passed, "RustSec audit"),
        (evidence.fuzz_passed, "fuzz/security review"),
        (evidence.accessibility_passed, "accessibility review"),
        (evidence.reproducible_build_passed, "reproducible build"),
    ] {
        if !passed {
            failures.push(format!("{name} did not pass"));
        }
    }
    if evidence.source_revision.len() < 7 {
        failures.push("source revision is missing".into());
    }
    for (artifact, name, maximum) in [
        (&evidence.lockfile, "Cargo.lock", 4 * 1024 * 1024),
        (
            &evidence.license_report,
            "license audit report",
            16 * 1024 * 1024,
        ),
        (&evidence.rustsec_report, "RustSec report", 16 * 1024 * 1024),
        (&evidence.fuzz_report, "fuzz report", 16 * 1024 * 1024),
        (
            &evidence.accessibility_report,
            "accessibility report",
            16 * 1024 * 1024,
        ),
        (
            &evidence.reproducible_artifact_a,
            "first reproducible artifact",
            2 * 1024 * 1024 * 1024,
        ),
        (
            &evidence.reproducible_artifact_b,
            "second reproducible artifact",
            2 * 1024 * 1024 * 1024,
        ),
    ] {
        if let Err(error) = artifact.verify(maximum) {
            failures.push(format!("{name}: {error}"));
        }
    }
    if evidence.reproducible_artifact_a.sha256 != evidence.reproducible_artifact_b.sha256 {
        failures.push("independent artifact SHA-256 values differ".into());
    }
    validate_freshness(evidence.completed_at_unix, now_unix, "audit", failures);
}

fn validate_external(
    evidence: &ExternalReleaseEvidence,
    now_unix: u64,
    failures: &mut Vec<String>,
) {
    let supported = [
        "windows-10-22h2-x64",
        "windows-11-23h2-x64",
        "windows-11-24h2-x64",
    ];
    for version in supported {
        match evidence.supported_windows_reports.get(version) {
            Some(report) if report.verify(16 * 1024 * 1024).is_ok() => {}
            _ => failures.push(format!("clean-profile report missing for {version}")),
        }
    }
    for (artifact, name, maximum) in [
        (
            &evidence.signed_artifact,
            "signed artifact",
            2 * 1024 * 1024 * 1024,
        ),
        (
            &evidence.clean_vm_report,
            "clean VM report",
            16 * 1024 * 1024,
        ),
        (
            &evidence.phase17_live_report,
            "Phase 17 live report",
            16 * 1024 * 1024,
        ),
    ] {
        if let Err(error) = artifact.verify(maximum) {
            failures.push(format!("{name}: {error}"));
        }
    }
    if evidence.authenticode_publisher.trim().is_empty() {
        failures.push("trusted Authenticode publisher is missing".into());
    }
    if let Err(error) = crate::windows_integration::verify_signed_executable(
        &evidence.signed_artifact.path,
        &evidence.approved_certificate_subject,
        &evidence.approved_certificate_thumbprint_sha256,
    ) {
        failures.push(format!(
            "signed artifact Authenticode validation failed: {error}"
        ));
    }
    validate_freshness(
        evidence.completed_at_unix,
        now_unix,
        "external release",
        failures,
    );
}

fn validate_freshness(timestamp: u64, now: u64, name: &str, failures: &mut Vec<String>) {
    if timestamp > now || now.saturating_sub(timestamp) > MAX_EVIDENCE_AGE_SECONDS {
        failures.push(format!("{name} evidence is stale or from the future"));
    }
}

#[cfg(target_os = "windows")]
pub fn current_process_working_set_bytes() -> Result<u64, String> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb)
            .map_err(|error| error.to_string())?;
    }
    Ok(counters.WorkingSetSize as u64)
}

#[cfg(not(target_os = "windows"))]
pub fn current_process_working_set_bytes() -> Result<u64, String> {
    Err("working-set measurement is only implemented on Windows".into())
}

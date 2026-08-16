// Deterministic local application-profile matrix for GhitaBrowser 2.0.5.
// These profiles are not a release-acceptance mechanism. They are a compact,
// local cross-subsystem contract used to ensure language, DOM, storage,
// realtime and media work together through browser-owned paths.

use crate::web_runtime::RuntimeReport;
use std::collections::{BTreeMap, BTreeSet};

pub const PROFILE_VERSION: u32 = 1;
pub const MAX_PROFILE_ERRORS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApplicationProfileKind {
    Banking,
    Productivity,
    Shopping,
    Social,
    Media,
}

impl ApplicationProfileKind {
    pub const ALL: [Self; 5] = [
        Self::Banking,
        Self::Productivity,
        Self::Shopping,
        Self::Social,
        Self::Media,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Banking => "banking-local-v1",
            Self::Productivity => "productivity-local-v1",
            Self::Shopping => "shopping-local-v1",
            Self::Social => "social-local-v1",
            Self::Media => "media-local-v1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProfileCapability {
    EcmaScript,
    PersistentCallbacks,
    DomMutation,
    Forms,
    History,
    CustomElements,
    Observers,
    Streams,
    IndexedDb,
    ServiceWorker,
    BackgroundWorker,
    Push,
    WebSocket,
    WebTransport,
    ProtectedMedia,
    WebRtc,
    Accessibility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationProfile {
    pub id: String,
    pub kind: ApplicationProfileKind,
    pub required_capabilities: BTreeSet<ProfileCapability>,
    pub max_runtime_errors: usize,
    pub max_memory_bytes: usize,
    pub min_interactions: usize,
}

impl ApplicationProfile {
    pub fn version_1(kind: ApplicationProfileKind) -> Self {
        use ProfileCapability as Capability;
        let required_capabilities: BTreeSet<_> = match kind {
            ApplicationProfileKind::Banking => vec![
                Capability::EcmaScript,
                Capability::PersistentCallbacks,
                Capability::DomMutation,
                Capability::Forms,
                Capability::History,
                Capability::IndexedDb,
                Capability::Accessibility,
            ],
            ApplicationProfileKind::Productivity => vec![
                Capability::EcmaScript,
                Capability::DomMutation,
                Capability::CustomElements,
                Capability::Observers,
                Capability::IndexedDb,
                Capability::ServiceWorker,
                Capability::BackgroundWorker,
                Capability::WebSocket,
                Capability::Accessibility,
            ],
            ApplicationProfileKind::Shopping => vec![
                Capability::EcmaScript,
                Capability::DomMutation,
                Capability::Forms,
                Capability::History,
                Capability::Streams,
                Capability::IndexedDb,
                Capability::Accessibility,
            ],
            ApplicationProfileKind::Social => vec![
                Capability::EcmaScript,
                Capability::PersistentCallbacks,
                Capability::DomMutation,
                Capability::Observers,
                Capability::WebSocket,
                Capability::WebTransport,
                Capability::Push,
                Capability::BackgroundWorker,
                Capability::Accessibility,
            ],
            ApplicationProfileKind::Media => vec![
                Capability::EcmaScript,
                Capability::DomMutation,
                Capability::Streams,
                Capability::ProtectedMedia,
                Capability::WebRtc,
                Capability::Accessibility,
            ],
        }
        .into_iter()
        .collect();
        Self {
            id: kind.id().to_string(),
            kind,
            required_capabilities,
            max_runtime_errors: 0,
            max_memory_bytes: match kind {
                ApplicationProfileKind::Media => 768 * 1024 * 1024,
                _ => 512 * 1024 * 1024,
            },
            min_interactions: 3,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileMeasurements {
    pub observed_capabilities: BTreeSet<ProfileCapability>,
    pub interactions: usize,
    pub memory_bytes: usize,
    pub fallback_rendered: bool,
    pub foreign_engine_used: bool,
    pub accessibility_nodes: usize,
    pub runtime_errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileResult {
    pub profile_id: String,
    pub passed: bool,
    pub failures: Vec<String>,
    pub missing_capabilities: Vec<ProfileCapability>,
}

impl ApplicationProfile {
    pub fn evaluate(&self, measurements: &ProfileMeasurements) -> ProfileResult {
        let mut failures = Vec::new();
        if measurements.fallback_rendered {
            failures
                .push("fallback rendering cannot satisfy a local application profile".to_string());
        }
        if measurements.foreign_engine_used {
            failures.push(
                "foreign engine output cannot satisfy a local application profile".to_string(),
            );
        }
        if measurements.interactions < self.min_interactions {
            failures.push(format!(
                "observed {} interactions; profile requires {}",
                measurements.interactions, self.min_interactions
            ));
        }
        if measurements.memory_bytes > self.max_memory_bytes {
            failures.push(format!(
                "observed {} bytes; profile budget is {}",
                measurements.memory_bytes, self.max_memory_bytes
            ));
        }
        if measurements.accessibility_nodes == 0 {
            failures.push("profile did not produce an accessibility tree".to_string());
        }
        if measurements.runtime_errors.len() > self.max_runtime_errors {
            failures.push(format!(
                "profile produced {} runtime errors",
                measurements.runtime_errors.len()
            ));
        }
        let missing_capabilities: Vec<_> = self
            .required_capabilities
            .difference(&measurements.observed_capabilities)
            .copied()
            .collect();
        ProfileResult {
            profile_id: self.id.clone(),
            passed: failures.is_empty() && missing_capabilities.is_empty(),
            failures,
            missing_capabilities,
        }
    }
}

/// Browser-runtime observations shared by every local profile. Feature-specific
/// runners add their own capability observations before calling `evaluate`.
pub fn measurements_from_runtime(
    report: &RuntimeReport,
    accessibility_nodes: usize,
) -> ProfileMeasurements {
    let mut observed_capabilities = BTreeSet::new();
    if report.scripts_executed > 0 || report.scripts_seen > 0 {
        observed_capabilities.insert(ProfileCapability::EcmaScript);
    }
    if report.event_listeners > 0 || !report.events_dispatched.is_empty() || report.timers_fired > 0
    {
        observed_capabilities.insert(ProfileCapability::PersistentCallbacks);
    }
    if report.dom_mutations > 0 {
        observed_capabilities.insert(ProfileCapability::DomMutation);
    }
    if !report.submitted_forms.is_empty() || !report.validation_errors.is_empty() {
        observed_capabilities.insert(ProfileCapability::Forms);
    }
    if !report.history_mutations.is_empty() {
        observed_capabilities.insert(ProfileCapability::History);
    }
    for operation in &report.platform_operations {
        if operation.starts_with("customElements.") {
            observed_capabilities.insert(ProfileCapability::CustomElements);
        }
        if operation.starts_with("indexedDB.") {
            observed_capabilities.insert(ProfileCapability::IndexedDb);
        }
        if operation.starts_with("WebSocket.") {
            observed_capabilities.insert(ProfileCapability::WebSocket);
        }
        if operation.starts_with("ReadableStream.") {
            observed_capabilities.insert(ProfileCapability::Streams);
        }
    }
    ProfileMeasurements {
        observed_capabilities,
        interactions: report
            .events_dispatched
            .len()
            .saturating_add(report.timers_fired),
        memory_bytes: report.realm_heap_bytes,
        fallback_rendered: false,
        foreign_engine_used: false,
        accessibility_nodes,
        runtime_errors: report
            .errors
            .iter()
            .take(MAX_PROFILE_ERRORS)
            .cloned()
            .collect(),
    }
}

#[derive(Debug, Clone)]
pub struct LocalCompatibilityMatrix {
    pub version: u32,
    pub profiles: Vec<ApplicationProfile>,
}

impl Default for LocalCompatibilityMatrix {
    fn default() -> Self {
        Self {
            version: PROFILE_VERSION,
            profiles: ApplicationProfileKind::ALL
                .into_iter()
                .map(ApplicationProfile::version_1)
                .collect(),
        }
    }
}

impl LocalCompatibilityMatrix {
    pub fn evaluate_all(
        &self,
        measurements: &BTreeMap<String, ProfileMeasurements>,
    ) -> Vec<ProfileResult> {
        self.profiles
            .iter()
            .map(|profile| {
                measurements
                    .get(&profile.id)
                    .map(|measurement| profile.evaluate(measurement))
                    .unwrap_or_else(|| ProfileResult {
                        profile_id: profile.id.clone(),
                        passed: false,
                        failures: vec!["missing deterministic profile measurements".to_string()],
                        missing_capabilities: profile
                            .required_capabilities
                            .iter()
                            .copied()
                            .collect(),
                    })
            })
            .collect()
    }
}

use std::collections::{BTreeMap, BTreeSet};

use ghitabrowser::application_profiles::{
    ApplicationProfileKind, LocalCompatibilityMatrix, ProfileCapability, ProfileMeasurements,
};

#[test]
fn every_local_205_profile_requires_real_interaction_and_all_declared_capabilities() {
    let matrix = LocalCompatibilityMatrix::default();
    let mut measurements = BTreeMap::new();
    for profile in &matrix.profiles {
        measurements.insert(
            profile.id.clone(),
            ProfileMeasurements {
                observed_capabilities: profile.required_capabilities.clone(),
                interactions: 3,
                memory_bytes: 1024,
                fallback_rendered: false,
                foreign_engine_used: false,
                accessibility_nodes: 1,
                runtime_errors: Vec::new(),
            },
        );
    }
    assert!(matrix
        .evaluate_all(&measurements)
        .into_iter()
        .all(|result| result.passed));
}

#[test]
fn matrix_rejects_static_or_foreign_output() {
    let profile = ghitabrowser::application_profiles::ApplicationProfile::version_1(
        ApplicationProfileKind::Banking,
    );
    let result = profile.evaluate(&ProfileMeasurements {
        observed_capabilities: BTreeSet::from([ProfileCapability::EcmaScript]),
        interactions: 0,
        memory_bytes: 0,
        fallback_rendered: true,
        foreign_engine_used: true,
        accessibility_nodes: 0,
        runtime_errors: Vec::new(),
    });
    assert!(!result.passed);
    assert!(!result.missing_capabilities.is_empty());
}

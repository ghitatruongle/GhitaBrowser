//! Integration tests for Phase 25 — Web API Permissions Framework & Origin Store.

use ghitabrowser::permissions::{PermissionState, PermissionStore, PermissionType};

#[test]
fn origin_permission_prompt_grant_deny_and_reset_flow() {
    let mut store = PermissionStore::new();
    let site_a = "https://app-a.com";
    let site_b = "https://app-b.com";

    // Default state is Prompt
    assert_eq!(
        store.get_permission(site_a, PermissionType::Camera),
        PermissionState::Prompt
    );
    assert_eq!(
        store.get_permission(site_a, PermissionType::Notifications),
        PermissionState::Prompt
    );

    // Grant Camera on site A, Deny Notifications on site A
    store
        .set_permission(site_a, PermissionType::Camera, PermissionState::Granted)
        .unwrap();
    store
        .set_permission(
            site_a,
            PermissionType::Notifications,
            PermissionState::Denied,
        )
        .unwrap();

    assert_eq!(
        store.get_permission(site_a, PermissionType::Camera),
        PermissionState::Granted
    );
    assert_eq!(
        store.get_permission(site_a, PermissionType::Notifications),
        PermissionState::Denied
    );

    // Site B remains Prompt (origin isolation)
    assert_eq!(
        store.get_permission(site_b, PermissionType::Camera),
        PermissionState::Prompt
    );

    // Reset origin
    assert!(store.reset_origin(site_a));
    assert_eq!(
        store.get_permission(site_a, PermissionType::Camera),
        PermissionState::Prompt
    );
}

#[test]
fn permission_store_json_serialization_roundtrip() {
    let mut store = PermissionStore::new();
    let site = "https://geolocation-test.com";

    store
        .set_permission(site, PermissionType::Geolocation, PermissionState::Granted)
        .unwrap();
    store
        .set_permission(site, PermissionType::Midi, PermissionState::Denied)
        .unwrap();

    let json = store.to_json().expect("to_json");
    assert!(json.contains("Geolocation"));
    assert!(json.contains("Granted"));

    let loaded = PermissionStore::from_json(&json).expect("from_json");
    assert_eq!(
        loaded.get_permission(site, PermissionType::Geolocation),
        PermissionState::Granted
    );
    assert_eq!(
        loaded.get_permission(site, PermissionType::Midi),
        PermissionState::Denied
    );
}

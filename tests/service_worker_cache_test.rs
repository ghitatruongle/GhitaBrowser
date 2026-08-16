//! Integration tests for Phase 22 — Cache API and Service Worker lifecycle.

use ghitabrowser::cache_api::CacheStorage;
use ghitabrowser::service_worker::{
    ServiceWorkerContainer, ServiceWorkerRegistrationOptions, ServiceWorkerState,
};
use std::collections::HashMap;

#[test]
fn cache_storage_crud_and_match() {
    let mut cs = CacheStorage::new("https://example.com", None);

    let cache = cs.open("static-v1").expect("open cache");
    cache
        .put(
            "https://example.com/styles.css",
            "GET",
            200,
            HashMap::new(),
            b"body { background: black; }".to_vec(),
        )
        .expect("put css");

    assert!(cs.has("static-v1"));
    assert_eq!(cs.keys(), vec!["static-v1"]);

    let entry = cs
        .match_all("https://example.com/styles.css")
        .expect("match_all");
    assert_eq!(entry.response_status, 200);
    assert_eq!(entry.response_body, b"body { background: black; }");

    assert!(cs.delete("static-v1"));
    assert!(!cs.has("static-v1"));
}

#[test]
fn service_worker_lifecycle_state_transitions() {
    let mut sw_container = ServiceWorkerContainer::new("https://example.com");

    let reg = sw_container
        .register(
            "https://example.com/sw.js",
            Some(ServiceWorkerRegistrationOptions {
                scope: "/app/".to_string(),
            }),
        )
        .expect("register sw");

    assert_eq!(reg.scope, "/app/");
    assert_eq!(reg.state, ServiceWorkerState::Active);

    // Matching fetch requests
    let matched_reg = sw_container
        .get_registration("https://example.com/app/home")
        .expect("get_registration");
    assert_eq!(matched_reg.script_url, "https://example.com/sw.js");

    // Unregister
    assert!(sw_container.unregister("/app/"));
    assert!(sw_container
        .get_registration("https://example.com/app/home")
        .is_none());
}

#[test]
fn service_worker_fetch_interception_fallback() {
    let mut sw_container = ServiceWorkerContainer::new("https://example.com");

    // Pre-seed cache storage
    let cache = sw_container
        .cache_storage
        .open("sw-cache")
        .expect("open cache");
    cache
        .put(
            "https://example.com/app/data.json",
            "GET",
            200,
            HashMap::new(),
            b"{\"status\":\"offline\"}".to_vec(),
        )
        .expect("put json");

    // Register active SW
    sw_container
        .register(
            "https://example.com/sw.js",
            Some(ServiceWorkerRegistrationOptions {
                scope: "/app/".to_string(),
            }),
        )
        .expect("register");

    // Intercept fetch
    let intercepted = sw_container
        .intercept_fetch("https://example.com/app/data.json")
        .expect("intercepted");
    assert_eq!(intercepted.response_body, b"{\"status\":\"offline\"}");
}

#[test]
fn cache_body_and_headers_survive_disk_round_trip() {
    let directory =
        std::env::temp_dir().join(format!("ghitabrowser-phase22-cache-{}", std::process::id()));
    let path = directory.join("cache.json");
    std::fs::create_dir_all(&directory).expect("create cache fixture dir");
    {
        let mut storage = CacheStorage::new("https://example.com", Some(path.clone()));
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        storage
            .open("offline")
            .expect("open")
            .put(
                "https://example.com/data",
                "GET",
                200,
                headers,
                br#"{"cached":true}"#.to_vec(),
            )
            .expect("put");
    }
    let storage = CacheStorage::new("https://example.com", Some(path));
    let entry = storage
        .match_all("https://example.com/data")
        .expect("persisted cache entry");
    assert_eq!(entry.response_body, br#"{"cached":true}"#);
    assert_eq!(
        entry
            .response_headers
            .get("content-type")
            .map(String::as_str),
        Some("application/json")
    );
    let _ = std::fs::remove_dir_all(directory);
}

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use ed25519_dalek::{Signer, SigningKey};
use ghitabrowser::extensions::{
    ContentScriptConfig, ExtensionApproval, ExtensionError, ExtensionManager, ExtensionPackage,
    ExtensionPermission, ExtensionWorker, GhitaExtensionManifest,
};
use ghitabrowser::javascript::JsvEngine;
use ghitabrowser::package_crypto::PublisherTrustStore;

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[26; 32])
}

fn signed_package(id: &str) -> ExtensionPackage {
    let files = HashMap::from([
        (
            "scripts/background.js".into(),
            "let total = 1 + 2; total".into(),
        ),
        (
            "scripts/content.js".into(),
            "document.title = 'Ghita extension active'".into(),
        ),
    ]);
    let manifest = GhitaExtensionManifest {
        id: id.into(),
        name: "Security fixture".into(),
        version: "1.0.0".into(),
        description: Some("Original signed GhitaBrowser extension fixture".into()),
        author: Some("GhitaBrowser tests".into()),
        permissions: vec![
            ExtensionPermission::Storage,
            ExtensionPermission::Network,
            ExtensionPermission::ContentScript,
        ],
        network_origins: vec!["https://api.example.test/*".into()],
        background_script: Some("scripts/background.js".into()),
        content_scripts: vec![ContentScriptConfig {
            matches: vec!["https://example.test/app/*".into()],
            script_path: "scripts/content.js".into(),
        }],
        publisher_key_id: "phase26-test-publisher".into(),
        signature: "0".repeat(128),
    };
    let mut package = ExtensionPackage { manifest, files };
    let signature = signing_key().sign(&package.canonical_payload().unwrap());
    package.manifest.signature = ghitabrowser::package_crypto::encode_hex(&signature.to_bytes());
    package
}

fn manager_with_trust() -> ExtensionManager {
    let mut trust = PublisherTrustStore::new();
    trust
        .insert_ed25519(
            "phase26-test-publisher",
            signing_key().verifying_key().to_bytes(),
        )
        .unwrap();
    ExtensionManager::new_in_memory_with_trust(trust)
}

fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ghita-phase26-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn ed25519_authenticity_rejects_unknown_key_and_tampering() {
    let package = signed_package("signed_ext");
    assert!(package
        .verify_signature(&PublisherTrustStore::new())
        .is_err());

    let mut manager = manager_with_trust();
    let review = manager.review_package(&package).unwrap();
    manager
        .install_reviewed_package(package.clone(), ExtensionApproval::approve_all(&review))
        .unwrap();

    let mut tampered = package;
    tampered
        .files
        .insert("scripts/background.js".into(), "malicious()".into());
    assert!(matches!(
        manager.review_package(&tampered),
        Err(ExtensionError::InvalidSignature(_))
    ));
}

#[test]
fn review_is_bound_to_bytes_and_never_grants_undeclared_permissions() {
    let package = signed_package("review_ext");
    let mut manager = manager_with_trust();
    let review = manager.review_package(&package).unwrap();
    let mut approval = ExtensionApproval::approve_all(&review);
    approval
        .approved_permissions
        .insert(ExtensionPermission::Tabs);
    assert!(matches!(
        manager.install_reviewed_package(package.clone(), approval),
        Err(ExtensionError::PermissionDenied(_))
    ));

    let mut wrong_bytes = ExtensionApproval::approve_all(&review);
    wrong_bytes.package_digest = "00".repeat(32);
    assert!(matches!(
        manager.install_reviewed_package(package, wrong_bytes),
        Err(ExtensionError::ReviewRequired(_))
    ));
}

#[test]
fn identifiers_package_paths_and_origin_patterns_fail_closed() {
    let mut escaped = signed_package("safe_before_escape");
    escaped.manifest.id = "../escape".into();
    assert!(matches!(
        escaped.validate(),
        Err(ExtensionError::InvalidManifest(_))
    ));

    escaped = signed_package("safe_ext");
    escaped.files.insert("../../outside.js".into(), "1".into());
    assert!(escaped.validate().is_err());

    let config = ContentScriptConfig {
        matches: vec!["https://example.test/app/*".into()],
        script_path: "script.js".into(),
    };
    assert!(config.matches_url("https://example.test/app/page"));
    assert!(!config.matches_url("https://example.test.evil/app/page"));
    assert!(!config.matches_url("file:///example.test/app/page"));
}

#[test]
fn worker_has_real_step_budget_and_single_use_cancellation() {
    let mut worker = ExtensionWorker::new("worker_ext", vec![]);
    let mut engine = JsvEngine::new();
    let error = worker
        .execute_script(
            "loop.js",
            "let i = 0; while (true) { i = i + 1; }",
            &mut engine,
        )
        .unwrap_err();
    assert!(error.to_string().contains("step budget"));
    assert!(worker
        .execute_script("second.js", "1 + 1", &mut engine)
        .is_err());

    let mut cancelled = ExtensionWorker::new("cancelled", vec![]);
    cancelled.cancel();
    assert!(cancelled
        .execute_script("never.js", "1 + 1", &mut JsvEngine::new())
        .is_err());
}

#[test]
fn storage_network_grants_persist_and_uninstall_removes_only_owned_data() {
    let root = temp_dir("profile");
    let public_key = signing_key().verifying_key().to_bytes();
    {
        let mut manager = ExtensionManager::new_with_profile(&root).unwrap();
        manager
            .trust_publisher("phase26-test-publisher", public_key)
            .unwrap();
        let package = signed_package("persistent_ext");
        let review = manager.review_package(&package).unwrap();
        let mut approval = ExtensionApproval::approve_all(&review);
        approval.approved_permissions =
            BTreeSet::from([ExtensionPermission::Storage, ExtensionPermission::Network]);
        manager.install_reviewed_package(package, approval).unwrap();
        manager
            .storage_set("persistent_ext", "theme", "dark")
            .unwrap();
        assert!(manager
            .authorize_network_request("persistent_ext", "https://api.example.test/v1")
            .is_ok());
        assert!(manager
            .authorize_network_request("persistent_ext", "https://evil.example/v1")
            .is_err());
    }
    {
        let mut manager = ExtensionManager::new_with_profile(&root).unwrap();
        assert_eq!(
            manager.storage_get("persistent_ext", "theme").unwrap(),
            Some(&"dark".to_string())
        );
        manager.uninstall_extension("persistent_ext").unwrap();
        assert!(!root.join("extensions").join("persistent_ext").exists());
        assert!(root
            .join("extensions")
            .join("trusted_publishers.json")
            .exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}

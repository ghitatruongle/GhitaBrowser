use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use ed25519_dalek::{Signer, SigningKey};
use ghitabrowser::package_crypto::PublisherTrustStore;
use ghitabrowser::updater::{
    UninstallChoice, UpdateError, UpdateFault, UpdateInstaller, UpdateManager, UpdateManifest,
    UpdatePackage, UpdateState, VersionComparer,
};

fn key() -> SigningKey {
    SigningKey::from_bytes(&[27; 32])
}

fn package(version: &str, delta_base: Option<&str>) -> UpdatePackage {
    let files = HashMap::from([
        (
            "ghitabrowser.exe".into(),
            format!("BINARY-{version}").into_bytes(),
        ),
        ("assets/icon.png".into(), b"PNG-DATA".to_vec()),
    ]);
    let manifest = UpdateManifest {
        version: version.into(),
        min_supported_version: "2.0.0".into(),
        channel: "stable".into(),
        download_url: "https://updates.example.test/ghitabrowser.pkg".into(),
        release_notes: Some("Signed Phase 27 fixture".into()),
        is_delta: delta_base.is_some(),
        delta_base_version: delta_base.map(str::to_string),
        publisher_key_id: "phase27-release-key".into(),
        file_hashes: BTreeMap::new(),
        signature: "0".repeat(128),
    };
    let mut package = UpdatePackage::new(manifest, files).unwrap();
    package.manifest.signature = ghitabrowser::package_crypto::encode_hex(
        &key().sign(&package.canonical_payload().unwrap()).to_bytes(),
    );
    package
}

fn trust() -> PublisherTrustStore {
    let mut trust = PublisherTrustStore::new();
    trust
        .insert_ed25519("phase27-release-key", key().verifying_key().to_bytes())
        .unwrap();
    trust
}

fn roots(label: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "ghita-phase27-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let install = root.join("install");
    let profile = root.join("profile");
    let state = profile.join("updater");
    (root, install, profile, state)
}

#[test]
fn signed_manifest_and_every_payload_hash_fail_closed_on_tamper() {
    let mut package = package("2.1.0", None);
    package.verify_signature(&trust()).unwrap();
    package
        .files
        .insert("ghitabrowser.exe".into(), b"MALICIOUS".to_vec());
    assert!(matches!(
        package.verify_signature(&trust()),
        Err(UpdateError::PayloadCorrupt(_))
    ));
    assert!(package
        .verify_signature(&PublisherTrustStore::new())
        .is_err());
}

#[test]
fn semver_selection_rejects_invalid_downgrade_and_wrong_delta_base() {
    assert!(VersionComparer::is_newer("2.1.0-beta.2", "2.1.0-beta.1"));
    assert!(!VersionComparer::is_newer("broken", "2.0.0"));
    let mut manager = UpdateManager::new_in_memory_with_trust("2.0.0", trust());
    assert!(manager
        .check_update(&package("2.1.0", None).manifest)
        .unwrap());
    assert!(matches!(
        manager.check_update(&package("1.9.0", None).manifest),
        Err(UpdateError::DowngradeDisallowed(_))
    ));
    assert!(matches!(
        manager.check_update(&package("2.1.0", Some("1.9.0")).manifest),
        Err(UpdateError::PayloadCorrupt(_))
    ));
}

#[test]
fn full_update_changes_real_install_root_and_preserves_profile() {
    let (root, install, profile, state) = roots("full");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(install.join("ghitabrowser.exe"), b"BINARY-2.0.0").unwrap();
    std::fs::write(profile.join("settings.json"), b"owned-user-data").unwrap();
    let mut manager =
        UpdateManager::new_with_paths("2.0.0", &install, &state, &profile, trust()).unwrap();
    manager.apply_update(package("2.1.0", None)).unwrap();
    assert_eq!(
        std::fs::read(install.join("ghitabrowser.exe")).unwrap(),
        b"BINARY-2.1.0"
    );
    assert_eq!(
        std::fs::read(profile.join("settings.json")).unwrap(),
        b"owned-user-data"
    );
    assert!(!state.join("journal.json").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn delta_merges_only_signed_files_and_keeps_unchanged_install_files() {
    let (root, install, profile, state) = roots("delta");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("unchanged.dat"), b"KEEP").unwrap();
    std::fs::create_dir_all(&profile).unwrap();
    let mut manager =
        UpdateManager::new_with_paths("2.0.0", &install, &state, &profile, trust()).unwrap();
    manager
        .apply_update(package("2.1.0", Some("2.0.0")))
        .unwrap();
    assert_eq!(
        std::fs::read(install.join("unchanged.dat")).unwrap(),
        b"KEEP"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn interruption_journal_recovers_backup_on_next_start() {
    let (root, install, profile, state) = roots("recovery");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(install.join("ghitabrowser.exe"), b"BINARY-2.0.0").unwrap();
    {
        let mut manager =
            UpdateManager::new_with_paths("2.0.0", &install, &state, &profile, trust()).unwrap();
        assert!(matches!(
            manager
                .apply_update_with_fault(package("2.1.0", None), UpdateFault::InterruptAfterBackup),
            Err(UpdateError::Interrupted(_))
        ));
        assert!(state.join("journal.json").exists());
    }
    let manager =
        UpdateManager::new_with_paths("2.0.0", &install, &state, &profile, trust()).unwrap();
    assert_eq!(manager.state, UpdateState::RolledBack);
    assert_eq!(
        std::fs::read(install.join("ghitabrowser.exe")).unwrap(),
        b"BINARY-2.0.0"
    );
    assert!(!state.join("journal.json").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn disk_full_preflight_never_modifies_install_tree() {
    let (root, install, profile, state) = roots("diskfull");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(install.join("ghitabrowser.exe"), b"ORIGINAL").unwrap();
    let mut manager =
        UpdateManager::new_with_paths("2.0.0", &install, &state, &profile, trust()).unwrap();
    assert!(matches!(
        manager.apply_update_with_fault(package("2.1.0", None), UpdateFault::DiskFullBeforeStage),
        Err(UpdateError::DiskFull(_))
    ));
    assert_eq!(
        std::fs::read(install.join("ghitabrowser.exe")).unwrap(),
        b"ORIGINAL"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repair_restores_corruption_only_from_verified_package() {
    let (root, install, profile, state) = roots("repair");
    std::fs::create_dir_all(&profile).unwrap();
    let mut manager =
        UpdateManager::new_with_paths("2.0.0", &install, &state, &profile, trust()).unwrap();
    let package = package("2.1.0", None);
    manager.apply_update(package.clone()).unwrap();
    std::fs::write(install.join("ghitabrowser.exe"), b"CORRUPT").unwrap();
    let repaired = manager.repair(&package).unwrap();
    assert_eq!(repaired, vec!["ghitabrowser.exe"]);
    assert_eq!(
        std::fs::read(install.join("ghitabrowser.exe")).unwrap(),
        b"BINARY-2.1.0"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn uninstall_keeps_profile_by_default_and_requires_exact_confirmation_to_remove_it() {
    let (root, install, profile, state) = roots("uninstall");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("history.json"), b"user data").unwrap();
    let mut manager =
        UpdateManager::new_with_paths("2.0.0", &install, &state, &profile, trust()).unwrap();
    manager.uninstall(UninstallChoice::KeepUserProfile).unwrap();
    assert!(!install.exists());
    assert!(profile.join("history.json").exists());

    std::fs::create_dir_all(&install).unwrap();
    let mut manager =
        UpdateManager::new_with_paths("2.0.0", &install, &state, &profile, trust()).unwrap();
    assert!(manager
        .uninstall(UninstallChoice::RemoveUserProfile {
            confirmed_path: root.join("wrong")
        })
        .is_err());
    assert!(install.exists());
    assert!(profile.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn standalone_installer_rejects_symlink_or_path_escape_payloads() {
    let (root, install, _profile, state) = roots("paths");
    let mut package = package("2.1.0", None);
    package
        .files
        .insert("../outside".into(), b"escape".to_vec());
    assert!(UpdateInstaller::stage_update(&state.join("staging"), &package).is_err());
    assert!(!root.join("outside").exists());
    assert!(!install.exists());
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
}

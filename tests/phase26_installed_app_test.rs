use std::path::PathBuf;

use ghitabrowser::installed_app::{
    AppDisplayMode, AppError, AppIconConfig, InstalledAppApproval, InstalledAppManager,
    InstalledAppManifest,
};

fn manifest(id: &str) -> InstalledAppManifest {
    InstalledAppManifest {
        id: id.into(),
        name: "Phase 26 App".into(),
        start_url: "https://app.example.test/workspace/start".into(),
        scope_url: "https://app.example.test/workspace/".into(),
        display_mode: AppDisplayMode::Standalone,
        icons: vec![AppIconConfig {
            src: "icons/192.png".into(),
            sizes: "192x192".into(),
        }],
        permissions: vec!["storage".into(), "notifications".into()],
    }
}

fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ghita-phase26-app-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn install_requires_same_origin_review_and_exact_approval() {
    let mut manager = InstalledAppManager::new_in_memory();
    let manifest = manifest("reviewed_app");
    assert!(matches!(
        manager.review_manifest(&manifest, "https://evil.example/install"),
        Err(AppError::PermissionDenied(_))
    ));
    let review = manager
        .review_manifest(&manifest, "https://app.example.test/install")
        .unwrap();
    let mut wrong = InstalledAppApproval::approve_all(&review);
    wrong.origin = "https://evil.example".into();
    assert!(matches!(
        manager.install_reviewed_app(manifest.clone(), "https://app.example.test/install", wrong),
        Err(AppError::ReviewRequired(_))
    ));
    manager
        .install_reviewed_app(
            manifest,
            "https://app.example.test/install",
            InstalledAppApproval::approve_all(&review),
        )
        .unwrap();
}

#[test]
fn manifest_scope_icons_and_identifier_cannot_escape_origin_or_profile() {
    let mut invalid = manifest("../escape");
    assert!(invalid.validate().is_err());
    invalid = manifest("cross_origin_icon");
    invalid.icons[0].src = "https://evil.example/icon.png".into();
    assert!(invalid.validate().is_err());
    invalid = manifest("scope_escape");
    invalid.start_url = "https://app.example.test/outside".into();
    assert!(invalid.validate().is_err());
}

#[test]
fn app_runtime_uses_owned_web_platform_partition_and_window_lifecycle() {
    let root = temp_dir("runtime");
    let mut manager = InstalledAppManager::new_with_profile(&root).unwrap();
    let manifest = manifest("runtime_app");
    let review = manager
        .review_manifest(&manifest, "https://app.example.test/install")
        .unwrap();
    manager
        .install_reviewed_app(
            manifest,
            "https://app.example.test/install",
            InstalledAppApproval::approve_all(&review),
        )
        .unwrap();
    let window = manager.launch_app("runtime_app").unwrap();
    assert!(window
        .storage_partition
        .as_ref()
        .unwrap()
        .starts_with(root.join("apps").join("runtime_app")));
    let mut runtime = manager
        .create_runtime(
            "runtime_app",
            "<script>localStorage.theme = 'dark'</script>",
            800,
        )
        .unwrap();
    runtime.run_document().unwrap();
    assert_eq!(manager.active_windows_count(), 1);
    manager.close_window(window.window_id).unwrap();
    assert_eq!(manager.active_windows_count(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn restart_and_uninstall_remove_all_app_owned_offline_data() {
    let root = temp_dir("cleanup");
    {
        let mut manager = InstalledAppManager::new_with_profile(&root).unwrap();
        let manifest = manifest("cleanup_app");
        let review = manager
            .review_manifest(&manifest, "https://app.example.test/install")
            .unwrap();
        manager
            .install_reviewed_app(
                manifest,
                "https://app.example.test/install",
                InstalledAppApproval::approve_all(&review),
            )
            .unwrap();
        std::fs::write(
            root.join("apps/cleanup_app/web-platform/cache-fixture"),
            b"owned",
        )
        .unwrap();
    }
    {
        let mut manager = InstalledAppManager::new_with_profile(&root).unwrap();
        assert!(manager.get_app("cleanup_app").is_some());
        manager.uninstall_app("cleanup_app").unwrap();
        assert!(!root.join("apps/cleanup_app").exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}

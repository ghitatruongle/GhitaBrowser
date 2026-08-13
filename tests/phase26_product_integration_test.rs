use std::collections::HashMap;
use std::path::PathBuf;

use ed25519_dalek::{Signer, SigningKey};
use ghitabrowser::extensions::{
    ContentScriptConfig, ExtensionApproval, ExtensionPackage, ExtensionPermission,
    GhitaExtensionManifest,
};
use ghitabrowser::installed_app::{
    AppDisplayMode, InstalledAppApproval, InstalledAppManager, InstalledAppManifest,
};
use ghitabrowser::web_runtime::PageRuntime;
use ghitabrowser::Browser;

fn key() -> SigningKey {
    SigningKey::from_bytes(&[42; 32])
}

fn package(id: &str) -> ExtensionPackage {
    let files = HashMap::from([(
        "content.js".into(),
        "document.getElementById('target').textContent = 'Injected by reviewed extension'".into(),
    )]);
    let manifest = GhitaExtensionManifest {
        id: id.into(),
        name: "Product integration".into(),
        version: "1.0.0".into(),
        description: None,
        author: None,
        permissions: vec![ExtensionPermission::ContentScript],
        network_origins: vec![],
        background_script: None,
        content_scripts: vec![ContentScriptConfig {
            matches: vec!["https://product.example.test/*".into()],
            script_path: "content.js".into(),
        }],
        publisher_key_id: "product-test-key".into(),
        signature: "0".repeat(128),
    };
    let mut package = ExtensionPackage { manifest, files };
    package.manifest.signature = ghitabrowser::package_crypto::encode_hex(
        &key().sign(&package.canonical_payload().unwrap()).to_bytes(),
    );
    package
}

fn app_manifest() -> InstalledAppManifest {
    InstalledAppManifest {
        id: "product_app".into(),
        name: "Product App".into(),
        start_url: "https://product.example.test/app/start".into(),
        scope_url: "https://product.example.test/app/".into(),
        display_mode: AppDisplayMode::Standalone,
        icons: vec![],
        permissions: vec!["storage".into()],
    }
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ghita-phase26-product-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn browser_executes_only_reviewed_matching_content_scripts_in_page_runtime() {
    let mut browser = Browser::new_in_memory();
    browser
        .extension_manager
        .trust_publisher("product-test-key", key().verifying_key().to_bytes())
        .unwrap();
    let package = package("product_extension");
    let review = browser.extension_manager.review_package(&package).unwrap();
    browser
        .extension_manager
        .install_reviewed_package(package, ExtensionApproval::approve_all(&review))
        .unwrap();

    let mut runtime = PageRuntime::from_html(
        "<p id='target'>Before</p>",
        vec![],
        800,
        "https://product.example.test/page",
    )
    .unwrap();
    let results = browser
        .extension_manager
        .execute_content_scripts("https://product.example.test/page", &mut runtime);
    assert_eq!(results.len(), 1);
    assert!(results[0].2.is_ok());
    assert_eq!(
        runtime
            .evaluate("document.getElementById('target').textContent")
            .unwrap()
            .to_display_string(),
        "Injected by reviewed extension"
    );
    assert!(browser
        .extension_manager
        .get_content_scripts_for_url("https://product.example.test.evil/page")
        .is_empty());
}

#[test]
fn browser_profile_restores_trust_extension_and_reviewed_app() {
    let root = temp_dir();
    {
        let mut browser = Browser::new_with_profile(&root, "Work").unwrap();
        browser
            .extension_manager
            .trust_publisher("product-test-key", key().verifying_key().to_bytes())
            .unwrap();
        let package = package("persistent_product_extension");
        let review = browser.extension_manager.review_package(&package).unwrap();
        browser
            .extension_manager
            .install_reviewed_package(package, ExtensionApproval::approve_all(&review))
            .unwrap();

        let manifest = app_manifest();
        let review = browser
            .app_manager
            .review_manifest(&manifest, "https://product.example.test/install")
            .unwrap();
        browser
            .app_manager
            .install_reviewed_app(
                manifest,
                "https://product.example.test/install",
                InstalledAppApproval::approve_all(&review),
            )
            .unwrap();
    }
    {
        let browser = Browser::new_with_profile(&root, "Work").unwrap();
        assert!(browser
            .extension_manager
            .get_extension("persistent_product_extension")
            .is_some());
        assert!(browser.app_manager.get_app("product_app").is_some());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn app_manager_rejects_install_without_product_review() {
    let mut manager = InstalledAppManager::new_in_memory();
    let manifest = app_manifest();
    let review = manager
        .review_manifest(&manifest, "https://product.example.test/install")
        .unwrap();
    let mut approval = InstalledAppApproval::approve_all(&review);
    approval.user_confirmed = false;
    assert!(manager
        .install_reviewed_app(manifest, "https://product.example.test/install", approval)
        .is_err());
}

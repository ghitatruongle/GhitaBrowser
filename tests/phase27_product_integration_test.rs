use std::path::PathBuf;

use ghitabrowser::windows_integration::CrashReportConsent;
use ghitabrowser::Browser;

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ghita-phase27-product-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn browser_owns_real_updater_and_windows_policy_managers() {
    let browser = Browser::new_in_memory();
    assert_eq!(browser.updater.current_version, ghitabrowser::VERSION);
    assert_eq!(browser.win_integration.file_associations.len(), 4);
    assert_eq!(browser.win_integration.protocol_handlers.len(), 2);
}

#[test]
fn browser_profile_restores_consent_and_notifications_without_uploading() {
    let root = temp_dir();
    {
        let mut browser = Browser::new_with_profile(&root, "Work").unwrap();
        browser
            .win_integration
            .set_crash_consent(CrashReportConsent::Denied)
            .unwrap();
        browser
            .win_integration
            .push_notification("Ready", "Phase 27", "update")
            .unwrap();
    }
    {
        let browser = Browser::new_with_profile(&root, "Work").unwrap();
        assert_eq!(
            browser.win_integration.crash_consent,
            CrashReportConsent::Denied
        );
        assert!(!browser.win_integration.crash_upload_allowed());
        assert_eq!(browser.win_integration.list_notifications().len(), 1);
    }
    std::fs::remove_dir_all(root).unwrap();
}

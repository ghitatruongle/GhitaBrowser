use std::path::PathBuf;

use ghitabrowser::windows_integration::{CliAction, CrashReportConsent, WindowsIntegration};

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ghita-phase27-windows-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn registry_plan_contains_per_user_capabilities_commands_and_no_userchoice() {
    let integration = WindowsIntegration::new_in_memory();
    let plan = integration
        .registration_plan(
            PathBuf::from(r"C:\Program Files\GhitaBrowser\ghitabrowser.exe").as_path(),
        )
        .unwrap();
    assert!(plan
        .iter()
        .any(|value| value.subkey.contains("FileAssociations")));
    assert!(plan
        .iter()
        .any(|value| value.subkey.contains("URLAssociations")));
    assert!(plan
        .iter()
        .any(|value| value.value.contains("ghitabrowser.exe\" \"%1")));
    assert!(plan
        .iter()
        .all(|value| !value.subkey.contains("UserChoice")));
    assert!(plan
        .iter()
        .all(|value| value.subkey.starts_with("Software\\")));
}

#[test]
fn command_line_activation_is_single_action_bounded_and_scheme_checked() {
    assert_eq!(
        WindowsIntegration::parse_cli_args(&["ghita.exe".into(), "--app=my_app".into()]).unwrap(),
        CliAction::LaunchApp("my_app".into())
    );
    assert_eq!(
        WindowsIntegration::parse_cli_args(&[
            "ghita.exe".into(),
            "https://example.test/path".into()
        ])
        .unwrap(),
        CliAction::OpenUrl("https://example.test/path".into())
    );
    assert!(WindowsIntegration::parse_cli_args(&[
        "ghita.exe".into(),
        "--update".into(),
        "--uninstall".into()
    ])
    .is_err());
    assert!(WindowsIntegration::parse_cli_args(&[
        "ghita.exe".into(),
        "javascript:alert(1)".into()
    ])
    .is_err());
    assert!(
        WindowsIntegration::parse_cli_args(&["ghita.exe".into(), "--app=../escape".into()])
            .is_err()
    );
}

#[test]
fn consent_and_bounded_notification_state_persist_exactly() {
    let root = temp_dir();
    {
        let mut integration = WindowsIntegration::new_with_profile(&root).unwrap();
        assert!(!integration.crash_upload_allowed());
        integration
            .set_crash_consent(CrashReportConsent::Granted)
            .unwrap();
        assert!(integration.crash_upload_allowed());
        for index in 0..105 {
            integration
                .push_notification("Update", format!("event {index}"), "update")
                .unwrap();
        }
        assert_eq!(integration.list_notifications().len(), 100);
        assert!(integration
            .push_notification("x".repeat(4096), "overflow", "update")
            .is_err());
    }
    {
        let integration = WindowsIntegration::new_with_profile(&root).unwrap();
        assert_eq!(integration.crash_consent, CrashReportConsent::Granted);
        assert_eq!(integration.list_notifications().len(), 100);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn host_registration_requires_authenticode_and_is_not_exercised_by_local_unit_tests() {
    let integration = WindowsIntegration::new_in_memory();
    let unsigned = temp_dir().join("unsigned.exe");
    std::fs::create_dir_all(unsigned.parent().unwrap()).unwrap();
    std::fs::write(&unsigned, b"not a PE signature").unwrap();
    let digest = ghitabrowser::package_crypto::sha256_hex(b"not a PE signature");
    assert!(integration
        .register_for_current_user(&unsigned, &digest)
        .is_err());
    std::fs::remove_dir_all(unsigned.parent().unwrap()).unwrap();
}

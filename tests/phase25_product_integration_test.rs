use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ghitabrowser::permissions::{PermissionState, PermissionType};
use ghitabrowser::storage::Cookie;
use ghitabrowser::Browser;

fn isolated_profile_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ghita-phase25-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn browser_applies_https_only_and_content_control_to_real_load_path() {
    let mut browser = Browser::new_in_memory();
    assert_eq!(
        browser.secure_navigation_url("http://example.test/account"),
        "https://example.test/account"
    );
    assert_eq!(
        browser.secure_navigation_url("http://localhost:8080/"),
        "http://localhost:8080/"
    );
    assert_eq!(
        browser.secure_navigation_url("http://localhost.attacker.test/"),
        "https://localhost.attacker.test/"
    );

    browser.content_control.add_rule(".sponsor", None);
    let rendered = browser
        .load_html(
            "http://example.test/page",
            "<main><p>Article</p><p class='sponsor'>Advertisement</p></main>",
        )
        .expect("offline page load");
    assert_eq!(
        browser.active_tab().expect("active tab").url,
        "https://example.test/page"
    );
    assert!(rendered.contains("Article"));
    assert!(!rendered.contains("Advertisement"));
}

#[test]
fn browser_blocks_cross_site_cookie_header_without_suffix_confusion() {
    let mut browser = Browser::new_in_memory();
    browser
        .storage
        .cookies_mut()
        .add_cookie(Cookie::new("session", "first", "first.test", "/"));
    browser
        .storage
        .cookies_mut()
        .add_cookie(Cookie::new("tracker", "third", "tracker.test", "/"));

    assert_eq!(
        browser.cookie_header_for_navigation("https://shop.first.test/", "https://first.test/api"),
        "session=first"
    );
    assert!(browser
        .cookie_header_for_navigation("https://shop.first.test/", "https://tracker.test/pixel")
        .is_empty());
    assert!(browser
        .cookie_header_for_navigation("https://evilfirst.test/", "https://first.test/api")
        .is_empty());
}

#[test]
fn profile_permission_decisions_are_origin_isolated_and_persistent() {
    let root = isolated_profile_root("permissions");
    {
        let mut browser = Browser::new_with_profile(&root, "Work").expect("profile");
        browser
            .set_permission(
                "https://meet.example.test/call",
                PermissionType::Camera,
                PermissionState::Granted,
            )
            .expect("persist permission");
        assert_eq!(
            browser.permission_state("https://meet.example.test", PermissionType::Camera),
            PermissionState::Granted
        );
        assert_eq!(
            browser.permission_state("https://other.example.test", PermissionType::Camera),
            PermissionState::Prompt
        );
    }
    {
        let browser = Browser::new_with_profile(&root, "Work").expect("restore profile");
        assert_eq!(
            browser.permission_state("https://meet.example.test", PermissionType::Camera),
            PermissionState::Granted
        );
    }
    std::fs::remove_dir_all(&root).expect("remove isolated profile");
}

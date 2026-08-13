use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ghitabrowser::Browser;

fn isolated_profile_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ghita-phase24-profile-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn real_browser_tabs_and_named_profile_restore_as_one_product_path() {
    let profile_root = isolated_profile_root();
    {
        let mut browser = Browser::new_with_profile(&profile_root, "Work").expect("work profile");
        browser
            .load_html("https://one.example.test/", "<title>One</title>")
            .expect("first tab");
        browser.add_tab(
            "https://two.example.test/",
            ghitabrowser::parse_html("<title>Two</title>"),
            "Two",
        );
        assert!(browser.pin_tab(1, true));
        assert_eq!(browser.tab_by_index(0).expect("pinned tab").title, "Two");
        assert_eq!(browser.toggle_tab_mute(0), Some(true));
        let group = browser
            .create_tab_group("Research", "#4f8cff")
            .expect("group");
        assert!(browser.assign_tab_group(0, Some(group)));
        browser.persist_session();
    }

    {
        let mut restored =
            Browser::new_with_profile(&profile_root, "Work").expect("restored profile");
        assert_eq!(restored.restore_previous_session(), 2);
        let pinned = restored.tab_by_index(0).expect("restored pinned tab");
        assert_eq!(pinned.url, "https://two.example.test/");
        assert!(pinned.is_pinned);
        assert!(pinned.is_muted);
        assert!(pinned.group_id.is_some());
        assert_eq!(restored.tab_groups().len(), 1);
    }

    {
        let mut separate =
            Browser::new_with_profile(&profile_root, "Personal").expect("separate profile");
        assert_eq!(separate.restore_previous_session(), 0);
        assert_eq!(separate.tab_count(), 0);
    }

    std::fs::remove_dir_all(profile_root).expect("remove test profile");
}

#[test]
fn profile_names_reject_path_traversal() {
    let profile_root = isolated_profile_root();
    assert!(Browser::new_with_profile(&profile_root, "../escape").is_err());
    assert!(Browser::new_with_profile(&profile_root, "").is_err());
}

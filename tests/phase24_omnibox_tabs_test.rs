//! Integration tests for Phase 24 — Omnibox Autocomplete & Advanced Tab Strip.

use ghitabrowser::omnibox::{OmniboxEngine, OmniboxMatchKind, SecurityIndicator};
use ghitabrowser::tab_strip::TabStripManager;

#[test]
fn omnibox_security_indicator_nav_search_and_autocomplete() {
    assert_eq!(
        SecurityIndicator::from_url("https://secure.site"),
        SecurityIndicator::Secure
    );
    assert_eq!(
        SecurityIndicator::from_url("http://insecure.site"),
        SecurityIndicator::Insecure
    );
    assert_eq!(
        SecurityIndicator::from_url("file:///C:/doc.html"),
        SecurityIndicator::Local
    );

    let omni = OmniboxEngine::new();

    // Direct domain navigation vs search query
    assert_eq!(
        omni.format_nav_or_search_url("example.org"),
        "https://example.org"
    );
    assert!(omni
        .format_nav_or_search_url("how to write rust")
        .contains("google.com/search?q=how"));

    // Autocomplete matching
    let history = vec![("https://news.com".to_string(), "Daily News".to_string())];
    let bookmarks = vec![("https://docs.rs".to_string(), "Rust Docs".to_string())];

    let matches = omni.autocomplete("doc", &history, &bookmarks);
    assert!(!matches.is_empty());
    assert_eq!(matches[0].kind, OmniboxMatchKind::Bookmark);
    assert_eq!(matches[0].url, "https://docs.rs");
}

#[test]
fn tab_strip_pinning_muting_grouping_and_reordering() {
    let mut manager = TabStripManager::new();

    let t1 = manager.add_tab("https://tab1.com", "Tab 1");
    let t2 = manager.add_tab("https://tab2.com", "Tab 2");
    let t3 = manager.add_tab("https://tab3.com", "Tab 3");

    assert_eq!(manager.tabs.len(), 3);
    assert_eq!(manager.active_tab_id, Some(t1));

    // Pinning t3 moves it to index 0
    assert!(manager.pin_tab(t3));
    assert_eq!(manager.tabs[0].id, t3);
    assert!(manager.tabs[0].pinned);

    // Audio muting toggle
    assert_eq!(manager.toggle_mute_tab(t2), Some(true));
    assert!(manager.tabs.iter().find(|t| t.id == t2).unwrap().muted);

    // Tab grouping
    let group_id = manager.create_group("Work", "#00ff00");
    assert!(manager.assign_to_group(t1, Some(group_id)));
    assert_eq!(
        manager.tabs.iter().find(|t| t.id == t1).unwrap().group_id,
        Some(group_id)
    );

    // Tab reordering
    assert!(manager.reorder_tab(0, 2));

    // Closing tab
    assert!(manager.close_tab(t1));
    assert_eq!(manager.tabs.len(), 2);
}

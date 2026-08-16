//! Integration tests for Phase 24 — Bookmarks Manager & History Manager.

use ghitabrowser::bookmarks::BookmarksManager;
use ghitabrowser::history_manager::HistoryManager;

#[test]
fn bookmarks_tree_crud_search_and_json_roundtrip() {
    let mut bm = BookmarksManager::new();

    // Create folders and items
    let folder_id = bm.create_folder(1, "Development").expect("create folder");
    let b1 = bm
        .add_bookmark(folder_id, "Rust Language", "https://www.rust-lang.org")
        .expect("add bookmark");

    let b2 = bm
        .add_bookmark(1, "GitHub", "https://github.com")
        .expect("add bookmark 2");

    // Search
    let search_res = bm.search("rust");
    assert_eq!(search_res.len(), 1);
    assert_eq!(search_res[0].id, b1);

    // JSON export & import round-trip
    let json = bm.export_json().expect("export");
    assert!(json.contains("Rust Language"));
    assert!(json.contains("Development"));

    let mut bm2 = BookmarksManager::new();
    bm2.import_json(&json).expect("import");
    assert_eq!(bm2.search("github")[0].id, b2);
}

#[test]
fn history_visit_tracking_search_and_clear_filters() {
    let mut hm = HistoryManager::new();

    hm.record_visit("https://site-a.com", "Site A", 1000);
    hm.record_visit("https://site-b.com", "Site B", 2000);
    hm.record_visit("https://site-a.com", "Site A Updated", 3000);

    // Visit deduplication
    assert_eq!(hm.entries.len(), 2);
    let site_a = hm
        .entries
        .iter()
        .find(|e| e.url.contains("site-a"))
        .unwrap();
    assert_eq!(site_a.visit_count, 2);
    assert_eq!(site_a.timestamp_ms, 3000);

    // Search
    assert_eq!(hm.search("Site B").len(), 1);

    // Date cutoff clear
    hm.clear_history(Some(2500));
    assert_eq!(hm.entries.len(), 1);
    assert_eq!(hm.entries[0].url, "https://site-a.com");

    // Clear all
    hm.clear_history(None);
    assert!(hm.entries.is_empty());
}

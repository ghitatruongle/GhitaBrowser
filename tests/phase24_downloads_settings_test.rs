//! Integration tests for Phase 24 — Downloads Manager & Settings Persistence.

use ghitabrowser::downloads::{DownloadState, DownloadsManager};
use ghitabrowser::settings::{BrowserSettings, StartupBehavior, ThemeMode};
use std::path::PathBuf;

#[test]
fn downloads_manager_lifecycle_pause_resume_cancel() {
    let mut dm = DownloadsManager::new();
    let target_path = PathBuf::from("/downloads/archive.zip");

    let id = dm.start_download("https://cdn.example.com/archive.zip", target_path, 10_000);

    dm.update_progress(id, 2000, 500 * 1024);
    assert_eq!(dm.downloads.get(&id).unwrap().downloaded_bytes, 2000);
    assert_eq!(
        dm.downloads.get(&id).unwrap().state,
        DownloadState::Downloading
    );

    // Pause
    assert!(dm.pause(id));
    assert_eq!(dm.downloads.get(&id).unwrap().state, DownloadState::Paused);

    // Resume
    assert!(dm.resume(id));
    assert_eq!(
        dm.downloads.get(&id).unwrap().state,
        DownloadState::Downloading
    );

    // Cancel
    assert!(dm.cancel(id));
    assert_eq!(
        dm.downloads.get(&id).unwrap().state,
        DownloadState::Cancelled
    );

    dm.clear_finished();
    assert!(dm.downloads.is_empty());
}

#[test]
fn browser_settings_themes_and_json_roundtrip() {
    let mut settings = BrowserSettings::new();
    settings.theme = ThemeMode::Dark;
    settings.default_search_engine = "DuckDuckGo".to_string();
    settings.startup_behavior =
        StartupBehavior::OpenSpecificPages(vec!["https://news.com".to_string()]);
    settings.clear_on_exit = true;

    let json = settings.to_json().expect("to_json");
    assert!(json.contains("Dark"));
    assert!(json.contains("DuckDuckGo"));

    let loaded = BrowserSettings::from_json(&json).expect("from_json");
    assert_eq!(settings, loaded);
}

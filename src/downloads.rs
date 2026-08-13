// Downloads Manager for GhitaBrowser (Phase 24).
// Implements file download tasks, pause/resume/cancel state transitions, and speed metrics.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    Downloading,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub id: u64,
    pub url: String,
    pub target_path: PathBuf,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub speed_bytes_per_sec: u64,
    pub state: DownloadState,
}

#[derive(Default)]
pub struct DownloadsManager {
    pub downloads: HashMap<u64, DownloadItem>,
    next_id: u64,
}

impl DownloadsManager {
    pub fn new() -> Self {
        Self {
            downloads: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn start_download(
        &mut self,
        url: impl Into<String>,
        target_path: PathBuf,
        total_bytes: u64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let item = DownloadItem {
            id,
            url: url.into(),
            target_path,
            total_bytes,
            downloaded_bytes: 0,
            speed_bytes_per_sec: 0,
            state: DownloadState::Downloading,
        };

        self.downloads.insert(id, item);
        id
    }

    pub fn update_progress(&mut self, id: u64, added_bytes: u64, speed_bytes_per_sec: u64) {
        if let Some(item) = self.downloads.get_mut(&id) {
            if item.state == DownloadState::Downloading {
                item.downloaded_bytes = item
                    .downloaded_bytes
                    .saturating_add(added_bytes)
                    .min(item.total_bytes);
                item.speed_bytes_per_sec = speed_bytes_per_sec;
                if item.downloaded_bytes >= item.total_bytes && item.total_bytes > 0 {
                    item.state = DownloadState::Completed;
                    item.speed_bytes_per_sec = 0;
                }
            }
        }
    }

    pub fn pause(&mut self, id: u64) -> bool {
        if let Some(item) = self.downloads.get_mut(&id) {
            if item.state == DownloadState::Downloading {
                item.state = DownloadState::Paused;
                item.speed_bytes_per_sec = 0;
                return true;
            }
        }
        false
    }

    pub fn resume(&mut self, id: u64) -> bool {
        if let Some(item) = self.downloads.get_mut(&id) {
            if item.state == DownloadState::Paused {
                item.state = DownloadState::Downloading;
                return true;
            }
        }
        false
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        if let Some(item) = self.downloads.get_mut(&id) {
            if item.state == DownloadState::Downloading || item.state == DownloadState::Paused {
                item.state = DownloadState::Cancelled;
                item.speed_bytes_per_sec = 0;
                return true;
            }
        }
        false
    }

    pub fn clear_finished(&mut self) {
        self.downloads.retain(|_, item| {
            item.state == DownloadState::Downloading || item.state == DownloadState::Paused
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_manager_lifecycle_pause_resume_complete() {
        let mut dm = DownloadsManager::new();
        let path = PathBuf::from("/tmp/file.zip");
        let id = dm.start_download("https://example.com/file.zip", path, 1000);

        dm.update_progress(id, 500, 100 * 1024);
        assert_eq!(dm.downloads.get(&id).unwrap().downloaded_bytes, 500);

        // Pause
        assert!(dm.pause(id));
        assert_eq!(dm.downloads.get(&id).unwrap().state, DownloadState::Paused);

        // Resume
        assert!(dm.resume(id));
        assert_eq!(
            dm.downloads.get(&id).unwrap().state,
            DownloadState::Downloading
        );

        // Finish download
        dm.update_progress(id, 500, 50 * 1024);
        assert_eq!(
            dm.downloads.get(&id).unwrap().state,
            DownloadState::Completed
        );
    }
}

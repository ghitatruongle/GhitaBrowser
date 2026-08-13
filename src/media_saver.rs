// Media detector and saver

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaType {
    Video,
    Audio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub url: String,
    pub title: String,
    pub media_type: MediaType,
    pub mime_type: Option<String>,
    pub file_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct MediaSaver {
    detected_items: Vec<MediaItem>,
}

impl MediaSaver {
    pub fn new() -> Self {
        Self {
            detected_items: Vec::new(),
        }
    }

    /// Scan HTML tree or URL string for downloadable media elements
    pub fn scan_url(&mut self, url: &str, title: &str) -> Option<MediaItem> {
        let lower = url.to_lowercase();
        let media_type = if lower.ends_with(".mp4")
            || lower.ends_with(".webm")
            || lower.ends_with(".mkv")
            || lower.contains("video/")
        {
            Some(MediaType::Video)
        } else if lower.ends_with(".mp3")
            || lower.ends_with(".m4a")
            || lower.ends_with(".wav")
            || lower.ends_with(".ogg")
            || lower.contains("audio/")
        {
            Some(MediaType::Audio)
        } else {
            None
        };

        if let Some(m_type) = media_type {
            let item = MediaItem {
                url: url.to_string(),
                title: if title.is_empty() {
                    "Web Media".to_string()
                } else {
                    title.to_string()
                },
                media_type: m_type,
                mime_type: None,
                file_size_bytes: None,
            };

            if !self.detected_items.iter().any(|i| i.url == item.url) {
                self.detected_items.push(item.clone());
            }
            Some(item)
        } else {
            None
        }
    }

    pub fn detected_items(&self) -> &[MediaItem] {
        &self.detected_items
    }

    pub fn clear(&mut self) {
        self.detected_items.clear();
    }

    pub fn has_media(&self) -> bool {
        !self.detected_items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_saver_scan() {
        let mut saver = MediaSaver::new();
        let item = saver.scan_url("https://example.com/video.mp4", "Sample Video");
        assert!(item.is_some());
        assert_eq!(item.unwrap().media_type, MediaType::Video);
        assert!(saver.has_media());

        let audio = saver.scan_url("https://example.com/podcast.mp3", "Podcast Episode");
        assert!(audio.is_some());
        assert_eq!(audio.unwrap().media_type, MediaType::Audio);
        assert_eq!(saver.detected_items().len(), 2);
    }
}

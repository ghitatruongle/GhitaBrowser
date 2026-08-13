//! History Manager for GhitaBrowser (Phase 24).
//! Implements history visit tracking, query search, date filtering, and history clearing.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryItem {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub timestamp_ms: u64,
    pub visit_count: u32,
}

#[derive(Default)]
pub struct HistoryManager {
    pub entries: Vec<HistoryItem>,
    next_id: u64,
}

impl HistoryManager {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
        }
    }

    pub fn record_visit(
        &mut self,
        url: impl Into<String>,
        title: impl Into<String>,
        timestamp_ms: u64,
    ) -> u64 {
        let url = url.into();
        let title = title.into();

        if let Some(item) = self.entries.iter_mut().find(|e| e.url == url) {
            item.visit_count += 1;
            item.timestamp_ms = timestamp_ms;
            item.title = title;
            item.id
        } else {
            let id = self.next_id;
            self.next_id += 1;

            let item = HistoryItem {
                id,
                url,
                title,
                timestamp_ms,
                visit_count: 1,
            };

            self.entries.push(item);
            id
        }
    }

    pub fn search(&self, query: &str) -> Vec<&HistoryItem> {
        let query_lower = query.trim().to_lowercase();
        if query_lower.is_empty() {
            return self.entries.iter().collect();
        }

        self.entries
            .iter()
            .filter(|item| {
                item.title.to_lowercase().contains(&query_lower)
                    || item.url.to_lowercase().contains(&query_lower)
            })
            .collect()
    }

    pub fn clear_history(&mut self, before_ms: Option<u64>) {
        if let Some(cutoff) = before_ms {
            self.entries.retain(|e| e.timestamp_ms > cutoff);
        } else {
            self.entries.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_manager_visit_recording_search_and_clear() {
        let mut hm = HistoryManager::new();
        hm.record_visit("https://rust-lang.org", "Rust", 1000);
        hm.record_visit("https://crates.io", "Crates", 2000);
        hm.record_visit("https://rust-lang.org", "Rust Lang", 3000);

        // Deduplication & visit count update
        assert_eq!(hm.entries.len(), 2);
        let rust_entry = hm.entries.iter().find(|e| e.url.contains("rust")).unwrap();
        assert_eq!(rust_entry.visit_count, 2);

        // Search
        let results = hm.search("crates");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://crates.io");

        // Selective clear
        hm.clear_history(Some(2500));
        assert_eq!(hm.entries.len(), 1);
        assert_eq!(hm.entries[0].url, "https://rust-lang.org");

        // Complete clear
        hm.clear_history(None);
        assert!(hm.entries.is_empty());
    }
}

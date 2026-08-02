// src/tab.rs - Tab management system with history navigation (v0.3.0)


use crate::layout::LayoutNode;
use crate::parser::Element;
use std::collections::HashMap;

/// A snapshot of page state for history navigation
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub dom: Element,
    pub layout: Option<LayoutNode>,
}

/// Represents a single browser tab
#[derive(Debug)]
pub struct Tab {
    pub id: usize,
    pub url: String,
    pub title: String,
    /// Cached DOM parsed from HTML content
    pub dom: Element,
    /// Cached layout tree for rendering
    pub layout: Option<LayoutNode>,
    /// Incognito tabs never record global browsing history
    pub incognito: bool,
    /// True when the current page is an error page (excluded from session history)
    pub is_error: bool,
    /// History stack for back/forward navigation
    history: Vec<HistoryEntry>,
    /// Current history position (0-indexed)
    history_pos: usize,
}

impl Tab {
    pub fn new(id: usize, url: String, dom: Element, title: String) -> Self {
        let entry = HistoryEntry {
            url: url.clone(),
            title: title.clone(),
            dom: dom.clone(),
            layout: None,
        };
        Tab {
            id,
            url: url.clone(),
            title,
            dom,
            layout: None,
            incognito: false,
            is_error: false,
            history: vec![entry],
            history_pos: 0,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn set_url(&mut self, url: String) {
        if self.history_pos + 1 < self.history.len() {
            self.history.truncate(self.history_pos + 1);
        }
        self.url = url.clone();
        self.layout = None;
    }

    pub fn push_history(&mut self, entry: HistoryEntry) {
        if self.history_pos + 1 < self.history.len() {
            self.history.truncate(self.history_pos + 1);
        }
        // Reloads (and duplicate notifications for one navigation) must not
        // stack: replace the current entry when the URL matches instead of
        // pushing a second copy of the page the tab is already showing.
        if let Some(last) = self.history.last_mut() {
            if last.url == entry.url {
                *last = entry;
                self.history_pos = self.history.len() - 1;
                self.layout = None;
                return;
            }
        }
        self.history.push(entry);
        self.history_pos = self.history.len() - 1;
        self.layout = None;
    }

    pub fn go_back(&mut self) -> bool {
        if self.is_error {
            // The tab is showing an error page for a URL that failed to load;
            // error pages never enter history, so Back returns to the last
            // good page (history[history_pos]) without moving the cursor.
            if let Some(entry) = self.history.get(self.history_pos) {
                self.url = entry.url.clone();
                self.title = entry.title.clone();
                self.dom = entry.dom.clone();
                self.layout = entry.layout.clone();
                self.is_error = false;
                return true;
            }
            return false;
        }
        if self.history_pos > 0 {
            self.history_pos -= 1;
            let entry = &self.history[self.history_pos];
            self.url = entry.url.clone();
            self.title = entry.title.clone();
            self.dom = entry.dom.clone();
            self.layout = entry.layout.clone();
            self.is_error = false; // history only holds successfully loaded pages
            true
        } else {
            false
        }
    }

    pub fn go_forward(&mut self) -> bool {
        if self.history_pos + 1 < self.history.len() {
            self.history_pos += 1;
            let entry = &self.history[self.history_pos];
            self.url = entry.url.clone();
            self.title = entry.title.clone();
            self.dom = entry.dom.clone();
            self.layout = entry.layout.clone();
            self.is_error = false; // history only holds successfully loaded pages
            true
        } else {
            false
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.history_pos > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_pos + 1 < self.history.len()
    }
}

/// Manages multiple browser tabs
pub struct TabManager {
    tabs: HashMap<usize, Tab>,
    active_tab_id: Option<usize>,
    next_id: usize,
    /// Ordering of tab IDs for UI
    tab_order: Vec<usize>,
    /// Recently closed tabs (url, title) for "Reopen closed tab" (Ctrl+Shift+T)
    closed_tabs: Vec<(String, String)>,
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TabManager {
    pub fn new() -> Self {
        TabManager {
            tabs: HashMap::new(),
            active_tab_id: None,
            next_id: 1,
            tab_order: Vec::new(),
            closed_tabs: Vec::new(),
        }
    }

    pub fn add_tab(&mut self, url: &str, dom: Element, title: &str) -> usize {
        let id = self.next_id;
        self.next_id += 1;

        let tab = Tab::new(id, url.to_string(), dom, title.to_string());
        self.tabs.insert(id, tab);
        self.tab_order.push(id);

        self.set_active_tab(id);
        id
    }

    pub fn get_tab(&self, id: usize) -> Option<&Tab> {
        self.tabs.get(&id)
    }

    pub fn get_tab_mut(&mut self, id: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(&id)
    }

    /// Get a tab by its position in the tab bar (0-indexed)
    pub fn get_tab_by_index(&self, index: usize) -> Option<&Tab> {
        self.tab_order.get(index).and_then(|id| self.tabs.get(id))
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active_tab_id.and_then(|id| self.tabs.get(&id))
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.active_tab_id.and_then(|id| self.tabs.get_mut(&id))
    }

    /// Get the active tab ID
    pub fn active_tab_id(&self) -> Option<usize> {
        self.active_tab_id
    }

    pub fn set_active_tab(&mut self, id: usize) {
        if self.tabs.contains_key(&id) {
            self.active_tab_id = Some(id);
        }
    }

    /// Activate a tab by its position in the tab bar
    pub fn set_active_by_index(&mut self, index: usize) {
        if let Some(&id) = self.tab_order.get(index) {
            self.active_tab_id = Some(id);
        }
    }

    /// Cycle to the next tab (Ctrl+Tab)
    pub fn activate_next(&mut self) {
        if self.tab_order.is_empty() {
            return;
        }
        let pos = self
            .active_tab_id
            .and_then(|id| self.tab_order.iter().position(|&tid| tid == id))
            .unwrap_or(0);
        let next = (pos + 1) % self.tab_order.len();
        self.active_tab_id = Some(self.tab_order[next]);
    }

    /// Cycle to the previous tab (Ctrl+Shift+Tab)
    pub fn activate_prev(&mut self) {
        if self.tab_order.is_empty() {
            return;
        }
        let pos = self
            .active_tab_id
            .and_then(|id| self.tab_order.iter().position(|&tid| tid == id))
            .unwrap_or(0);
        let prev = (pos + self.tab_order.len() - 1) % self.tab_order.len();
        self.active_tab_id = Some(self.tab_order[prev]);
    }

    pub fn remove_tab(&mut self, id: usize) -> Option<Tab> {
        // Remember the position for Chrome-style right-neighbor activation
        let old_pos = self.tab_order.iter().position(|&tid| tid == id);

        // Remove from order
        self.tab_order.retain(|&tid| tid != id);

        // Handle active tab changes
        if self.active_tab_id == Some(id) {
            if self.tab_order.is_empty() {
                self.active_tab_id = None;
            } else {
                // Chrome behavior: activate the tab to the right,
                // or the new last tab if the rightmost tab was closed
                let idx = old_pos.unwrap_or(0).min(self.tab_order.len() - 1);
                self.active_tab_id = Some(self.tab_order[idx]);
            }
        }

        let removed = self.tabs.remove(&id);

        // Remember closed tab for Ctrl+Shift+T (skip internal & incognito pages)
        if let Some(ref tab) = removed {
            if !tab.incognito && (tab.url.starts_with("http://") || tab.url.starts_with("https://"))
            {
                self.closed_tabs.push((tab.url.clone(), tab.title.clone()));
                if self.closed_tabs.len() > 25 {
                    self.closed_tabs.remove(0);
                }
            }
        }

        removed
    }

    /// Pop the most recently closed tab (url, title), if any
    pub fn pop_closed_tab(&mut self) -> Option<(String, String)> {
        self.closed_tabs.pop()
    }

    /// Whether there is a closed tab available to reopen
    pub fn has_closed_tabs(&self) -> bool {
        !self.closed_tabs.is_empty()
    }

    pub fn close_all_tabs(&mut self) {
        self.tabs.clear();
        self.tab_order.clear();
        self.active_tab_id = None;
        self.next_id = 1;
    }

    pub fn active_title(&self) -> Option<String> {
        self.active_tab().map(|t| t.title.clone())
    }

    pub fn active_url(&self) -> Option<String> {
        self.active_tab().map(|t| t.url.clone())
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn all_tabs(&self) -> std::collections::hash_map::Values<'_, usize, Tab> {
        self.tabs.values()
    }

    /// Iterate tabs in UI order
    pub fn iter_tabs(&self) -> Vec<&Tab> {
        self.tab_order
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Element;

    #[test]
    fn test_tab_manager_creation() {
        let mut tm = TabManager::new();
        let dom = Element::new("body");
        let id = tm.add_tab("https://example.com", dom, "Example");
        assert_eq!(tm.tab_count(), 1);
        assert_eq!(id, 1);
    }

    #[test]
    fn test_tab_navigation() {
        let mut tab = Tab::new(
            1,
            "https://a.com".to_string(),
            Element::new("body"),
            "A".to_string(),
        );
        // The seed entry is the first page — nothing behind it yet
        assert!(!tab.can_go_back());

        let entry_b = HistoryEntry {
            url: "https://b.com".to_string(),
            title: "B".to_string(),
            dom: Element::new("body"),
            layout: None,
        };
        tab.push_history(entry_b);

        let entry_c = HistoryEntry {
            url: "https://c.com".to_string(),
            title: "C".to_string(),
            dom: Element::new("body"),
            layout: None,
        };
        tab.push_history(entry_c);

        assert!(tab.can_go_back());
        assert!(!tab.can_go_forward());

        tab.go_back();
        assert_eq!(tab.url, "https://b.com");
        assert!(tab.can_go_forward());
    }

    #[test]
    fn test_tab_set_url_bounds() {
        let mut tab = Tab::new(
            1,
            "https://a.com".to_string(),
            Element::new("body"),
            "A".to_string(),
        );
        tab.set_url("https://b.com".to_string());
        assert_eq!(tab.url, "https://b.com");
    }

    #[test]
    fn test_push_history_dedups_same_url() {
        let mut tab = Tab::new(
            1,
            "https://a.com".to_string(),
            Element::new("body"),
            "A".to_string(),
        );
        // Reloading a.com replaces the seed entry instead of duplicating it
        let entry = HistoryEntry {
            url: "https://a.com".to_string(),
            title: "A (reloaded)".to_string(),
            dom: Element::new("body"),
            layout: None,
        };
        tab.push_history(entry);
        assert_eq!(tab.history.len(), 1);
        assert_eq!(tab.history[0].title, "A (reloaded)");

        // A different URL is appended normally
        let entry_b = HistoryEntry {
            url: "https://b.com".to_string(),
            title: "B".to_string(),
            dom: Element::new("body"),
            layout: None,
        };
        tab.push_history(entry_b);
        assert_eq!(tab.history.len(), 2);

        // Back lands on a.com (the page before b.com), and there is nothing
        // further back — the duplicated seed entry is gone.
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://a.com");
        assert!(!tab.can_go_back());
    }

    #[test]
    fn test_back_from_error_returns_to_last_good_page() {
        let mut tab = Tab::new(
            1,
            "https://newtab".to_string(),
            Element::new("body"),
            "New Tab".to_string(),
        );
        let entry_a = HistoryEntry {
            url: "https://a.com".to_string(),
            title: "A".to_string(),
            dom: Element::new("body"),
            layout: None,
        };
        tab.push_history(entry_a);

        // b.com fails: the tab shows an error page that is not in history
        tab.url = "https://b.com".to_string();
        tab.is_error = true;

        // Back returns to a.com — the last successfully loaded page — without
        // moving the cursor, so Back again still reaches the new tab page.
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://a.com");
        assert!(!tab.is_error);
        assert!(tab.can_go_back());
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://newtab");
        assert!(!tab.can_go_back());
    }

    #[test]
    fn test_navigation_clears_forward_history() {
        let mut tab = Tab::new(
            1,
            "https://newtab".to_string(),
            Element::new("body"),
            "New Tab".to_string(),
        );
        for url in ["https://a.com", "https://b.com", "https://c.com"] {
            tab.push_history(HistoryEntry {
                url: url.to_string(),
                title: url.to_string(),
                dom: Element::new("body"),
                layout: None,
            });
        }
        // Back to b.com, then navigate to d.com — forward entries must drop
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://b.com");
        assert!(tab.can_go_forward());
        tab.push_history(HistoryEntry {
            url: "https://d.com".to_string(),
            title: "D".to_string(),
            dom: Element::new("body"),
            layout: None,
        });
        assert_eq!(tab.url, "https://b.com"); // url unchanged; push only records
        assert!(!tab.can_go_forward());
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://b.com");
        assert!(tab.can_go_forward()); // d.com is forward of b.com again
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://a.com");
    }

    #[test]
    fn test_tab_manager_order() {
        let mut tm = TabManager::new();
        let _id1 = tm.add_tab("https://a.com", Element::new("body"), "A");
        let id2 = tm.add_tab("https://b.com", Element::new("div"), "B");
        let id3 = tm.add_tab("https://c.com", Element::new("span"), "C");

        assert_eq!(tm.tab_count(), 3);
        assert_eq!(tm.active_tab_id(), Some(id3));

        // Get by index
        assert_eq!(tm.get_tab_by_index(0).unwrap().url, "https://a.com");
        assert_eq!(tm.get_tab_by_index(1).unwrap().url, "https://b.com");
        assert_eq!(tm.get_tab_by_index(2).unwrap().url, "https://c.com");

        // Remove middle tab
        tm.remove_tab(id2);
        assert_eq!(tm.tab_count(), 2);
        assert_eq!(tm.get_tab_by_index(0).unwrap().url, "https://a.com");
        assert_eq!(tm.get_tab_by_index(1).unwrap().url, "https://c.com");
    }
}

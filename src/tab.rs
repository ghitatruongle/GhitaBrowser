// src/tab.rs - Tab management system with history navigation (v0.0.2)
#![allow(dead_code)]

use std::collections::HashMap;
use crate::parser::Element;
use crate::layout::LayoutNode;

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
    /// History stack for back/forward navigation
    history: Vec<String>,
    /// Current history position (0-indexed)
    history_pos: usize,
}

impl Tab {
    pub fn new(id: usize, url: String, dom: Element, title: String) -> Self {
        Tab {
            id,
            url: url.clone(),
            title,
            dom,
            layout: None,
            history: vec![url],
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
        self.history.push(url);
        self.history_pos = self.history.len() - 1;
        self.layout = None;
    }

    pub fn go_back(&mut self) -> bool {
        if self.history_pos > 0 {
            self.history_pos -= 1;
            self.url = self.history[self.history_pos].clone();
            self.layout = None;
            true
        } else {
            false
        }
    }

    pub fn go_forward(&mut self) -> bool {
        if self.history_pos + 1 < self.history.len() {
            self.history_pos += 1;
            self.url = self.history[self.history_pos].clone();
            self.layout = None;
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

    pub fn add_history_item(&mut self, url: &str) {
        if self.history_pos + 1 < self.history.len() {
            self.history.truncate(self.history_pos + 1);
        }
        self.history.push(url.to_string());
        self.history_pos = self.history.len() - 1;
        self.layout = None;
    }
}

/// Manages multiple browser tabs
pub struct TabManager {
    tabs: HashMap<usize, Tab>,
    active_tab_id: Option<usize>,
    next_id: usize,
    /// Ordering of tab IDs for UI
    tab_order: Vec<usize>,
}

impl TabManager {
    pub fn new() -> Self {
        TabManager {
            tabs: HashMap::new(),
            active_tab_id: None,
            next_id: 1,
            tab_order: Vec::new(),
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
        self.active_tab_id = Some(id);
    }

    pub fn remove_tab(&mut self, id: usize) -> Option<Tab> {
        // Remove from order
        self.tab_order.retain(|&tid| tid != id);
        
        // Handle active tab changes
        if self.active_tab_id == Some(id) {
            if self.tab_order.is_empty() {
                self.active_tab_id = None;
            } else {
                // Activate the last tab in the list
                self.active_tab_id = Some(self.tab_order[self.tab_order.len() - 1]);
            }
        }
        
        self.tabs.remove(&id)
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
        self.tab_order.iter()
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
        let mut tab = Tab::new(1, "https://a.com".to_string(), Element::new("body"), "A".to_string());
        assert_eq!(tab.history.len(), 1);
        assert_eq!(tab.history[0], "https://a.com");
        
        tab.add_history_item("https://b.com");
        assert_eq!(tab.history.len(), 2);
        
        tab.add_history_item("https://c.com");
        assert_eq!(tab.history.len(), 3);
        assert_eq!(tab.history[2], "https://c.com");
        
        assert!(tab.can_go_back());
        assert!(!tab.can_go_forward());
        
        tab.go_back();
        assert_eq!(tab.url, "https://b.com");
        assert!(tab.can_go_forward());
    }

    #[test]
    fn test_tab_set_url_bounds() {
        let mut tab = Tab::new(1, "https://a.com".to_string(), Element::new("body"), "A".to_string());
        tab.set_url("https://b.com".to_string());
        assert_eq!(tab.url, "https://b.com");
        assert_eq!(tab.history_pos, 1);
        assert_eq!(tab.history[tab.history_pos], "https://b.com");
    }
    
    #[test]
    fn test_tab_manager_order() {
        let mut tm = TabManager::new();
        let id1 = tm.add_tab("https://a.com", Element::new("body"), "A");
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

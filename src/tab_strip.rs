//! Advanced Tab Strip, Tab Pinning, Muting, and Grouping for GhitaBrowser (Phase 24).
//! Implements tab strip reordering, pinned tabs, audio muting, and tab group containers.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct TabGroup {
    pub id: u64,
    pub name: String,
    pub color: String,
    pub collapsed: bool,
}

#[derive(Debug, Clone)]
pub struct TabItem {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub pinned: bool,
    pub muted: bool,
    pub group_id: Option<u64>,
}

pub struct TabStripManager {
    pub tabs: Vec<TabItem>,
    pub groups: HashMap<u64, TabGroup>,
    pub active_tab_id: Option<u64>,
    next_tab_id: u64,
    next_group_id: u64,
}

impl TabStripManager {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            groups: HashMap::new(),
            active_tab_id: None,
            next_tab_id: 1,
            next_group_id: 10,
        }
    }

    pub fn add_tab(&mut self, url: impl Into<String>, title: impl Into<String>) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;

        let tab = TabItem {
            id,
            title: title.into(),
            url: url.into(),
            pinned: false,
            muted: false,
            group_id: None,
        };

        self.tabs.push(tab);
        if self.active_tab_id.is_none() {
            self.active_tab_id = Some(id);
        }
        id
    }

    pub fn pin_tab(&mut self, tab_id: u64) -> bool {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) {
            self.tabs[pos].pinned = true;
            // Move pinned tab to front of tab strip
            let tab = self.tabs.remove(pos);
            self.tabs.insert(0, tab);
            true
        } else {
            false
        }
    }

    pub fn unpin_tab(&mut self, tab_id: u64) -> bool {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.pinned = false;
            true
        } else {
            false
        }
    }

    pub fn toggle_mute_tab(&mut self, tab_id: u64) -> Option<bool> {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.muted = !tab.muted;
            Some(tab.muted)
        } else {
            None
        }
    }

    pub fn create_group(&mut self, name: impl Into<String>, color: impl Into<String>) -> u64 {
        let gid = self.next_group_id;
        self.next_group_id += 1;

        let group = TabGroup {
            id: gid,
            name: name.into(),
            color: color.into(),
            collapsed: false,
        };

        self.groups.insert(gid, group);
        gid
    }

    pub fn assign_to_group(&mut self, tab_id: u64, group_id: Option<u64>) -> bool {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.group_id = group_id;
            true
        } else {
            false
        }
    }

    pub fn reorder_tab(&mut self, from_index: usize, to_index: usize) -> bool {
        if from_index < self.tabs.len() && to_index < self.tabs.len() {
            let tab = self.tabs.remove(from_index);
            self.tabs.insert(to_index, tab);
            true
        } else {
            false
        }
    }

    pub fn close_tab(&mut self, tab_id: u64) -> bool {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) {
            self.tabs.remove(pos);
            if self.active_tab_id == Some(tab_id) {
                self.active_tab_id = self.tabs.first().map(|t| t.id);
            }
            true
        } else {
            false
        }
    }
}

impl Default for TabStripManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_strip_pin_mute_group_reorder() {
        let mut strip = TabStripManager::new();
        let t1 = strip.add_tab("https://site1.com", "Site 1");
        let t2 = strip.add_tab("https://site2.com", "Site 2");

        // Pinning t2 moves it to index 0
        assert!(strip.pin_tab(t2));
        assert_eq!(strip.tabs[0].id, t2);

        // Muting t1
        assert_eq!(strip.toggle_mute_tab(t1), Some(true));
        assert!(strip.tabs[1].muted);

        // Grouping
        let gid = strip.create_group("Work", "#ff0000");
        assert!(strip.assign_to_group(t1, Some(gid)));
        assert_eq!(strip.tabs[1].group_id, Some(gid));
    }
}

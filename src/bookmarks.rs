//! Bookmarks Manager & Tree Data Model for GhitaBrowser (Phase 24).
//! Implements hierarchical bookmark folders, CRUD, search, and JSON export/import.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookmarkItem {
    pub id: u64,
    pub title: String,
    pub url: Option<String>,
    pub is_folder: bool,
    pub children: Vec<BookmarkItem>,
}

pub struct BookmarksManager {
    pub root: BookmarkItem,
    next_id: u64,
}

impl BookmarksManager {
    pub fn new() -> Self {
        Self {
            root: BookmarkItem {
                id: 1,
                title: "Bookmarks Bar".to_string(),
                url: None,
                is_folder: true,
                children: Vec::new(),
            },
            next_id: 2,
        }
    }

    pub fn from_root(root: BookmarkItem) -> Result<Self, String> {
        let mut seen = std::collections::HashSet::new();
        let mut max_id = 0_u64;
        Self::validate_tree(&root, 0, &mut seen, &mut max_id)?;
        if !root.is_folder {
            return Err("Bookmark root must be a folder".to_string());
        }
        Ok(Self {
            root,
            next_id: max_id.saturating_add(1),
        })
    }

    pub fn add_bookmark(
        &mut self,
        parent_id: u64,
        title: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;

        let item = BookmarkItem {
            id,
            title: title.into(),
            url: Some(url.into()),
            is_folder: false,
            children: Vec::new(),
        };

        if Self::insert_into_node(&mut self.root, parent_id, item) {
            Ok(id)
        } else {
            Err(format!("Parent folder ID {parent_id} not found"))
        }
    }

    pub fn create_folder(
        &mut self,
        parent_id: u64,
        title: impl Into<String>,
    ) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;

        let folder = BookmarkItem {
            id,
            title: title.into(),
            url: None,
            is_folder: true,
            children: Vec::new(),
        };

        if Self::insert_into_node(&mut self.root, parent_id, folder) {
            Ok(id)
        } else {
            Err(format!("Parent folder ID {parent_id} not found"))
        }
    }

    pub fn search(&self, query: &str) -> Vec<BookmarkItem> {
        let query_lower = query.trim().to_lowercase();
        if query_lower.is_empty() {
            return Vec::new();
        }
        let mut results = Vec::new();
        Self::search_node(&self.root, &query_lower, &mut results);
        results
    }

    pub fn export_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.root).map_err(|e| format!("Bookmark export failed: {e}"))
    }

    pub fn import_json(&mut self, json_str: &str) -> Result<(), String> {
        if json_str.len() > 8 * 1024 * 1024 {
            return Err("Bookmark import exceeds 8 MB".to_string());
        }
        let imported: BookmarkItem =
            serde_json::from_str(json_str).map_err(|e| format!("Bookmark import failed: {e}"))?;
        *self = Self::from_root(imported)?;
        Ok(())
    }

    pub fn remove(&mut self, id: u64) -> bool {
        if id == self.root.id {
            return false;
        }
        Self::remove_from_node(&mut self.root, id)
    }

    pub fn remove_url(&mut self, url: &str) -> usize {
        Self::remove_url_from_node(&mut self.root, url)
    }

    fn validate_tree(
        node: &BookmarkItem,
        depth: usize,
        seen: &mut std::collections::HashSet<u64>,
        max_id: &mut u64,
    ) -> Result<(), String> {
        if depth > 32 || seen.len() >= 10_000 {
            return Err("Bookmark tree exceeds its depth or item budget".to_string());
        }
        if !seen.insert(node.id) {
            return Err("Bookmark tree contains duplicate IDs".to_string());
        }
        if node.title.len() > 4096 || node.url.as_ref().is_some_and(|url| url.len() > 64 * 1024) {
            return Err("Bookmark title or URL exceeds its byte budget".to_string());
        }
        if node.is_folder != node.url.is_none() || (!node.is_folder && !node.children.is_empty()) {
            return Err("Bookmark node shape is invalid".to_string());
        }
        *max_id = (*max_id).max(node.id);
        for child in &node.children {
            Self::validate_tree(child, depth + 1, seen, max_id)?;
        }
        Ok(())
    }

    fn remove_from_node(node: &mut BookmarkItem, id: u64) -> bool {
        if let Some(index) = node.children.iter().position(|child| child.id == id) {
            node.children.remove(index);
            return true;
        }
        node.children
            .iter_mut()
            .any(|child| child.is_folder && Self::remove_from_node(child, id))
    }

    fn remove_url_from_node(node: &mut BookmarkItem, url: &str) -> usize {
        let before = node.children.len();
        node.children
            .retain(|child| child.is_folder || child.url.as_deref() != Some(url));
        let mut removed = before - node.children.len();
        for child in &mut node.children {
            if child.is_folder {
                removed += Self::remove_url_from_node(child, url);
            }
        }
        removed
    }

    fn insert_into_node(node: &mut BookmarkItem, parent_id: u64, item: BookmarkItem) -> bool {
        if node.id == parent_id && node.is_folder {
            node.children.push(item);
            return true;
        }
        for child in &mut node.children {
            if child.is_folder && Self::insert_into_node(child, parent_id, item.clone()) {
                return true;
            }
        }
        false
    }

    fn search_node(node: &BookmarkItem, query: &str, results: &mut Vec<BookmarkItem>) {
        if !node.is_folder
            && (node.title.to_lowercase().contains(query)
                || node
                    .url
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(query))
        {
            results.push(node.clone());
        }
        for child in &node.children {
            Self::search_node(child, query, results);
        }
    }
}

impl Default for BookmarksManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_tree_crud_search_json_roundtrip() {
        let mut bm = BookmarksManager::new();
        let b1 = bm.add_bookmark(1, "GitHub", "https://github.com").unwrap();

        let f1 = bm.create_folder(1, "Dev Tools").unwrap();
        bm.add_bookmark(f1, "Docs", "https://docs.rs").unwrap();

        let found = bm.search("git");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, b1);

        let json = bm.export_json().unwrap();
        assert!(json.contains("GitHub"));

        let mut bm2 = BookmarksManager::new();
        bm2.import_json(&json).unwrap();
        assert_eq!(bm.root, bm2.root);
    }
}

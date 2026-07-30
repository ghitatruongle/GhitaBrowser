// src/lib.rs - Public re-exports for ghitabrowser crate
#![allow(dead_code)]

//! # GhitaBrowser
//! A lightweight Rust browser v0.0.0.

pub mod parser;
pub mod renderer;
pub mod network;
pub mod ui;
pub mod storage;
pub mod css_parser;
pub mod layout;
pub mod text_renderer;
pub mod image_loader;
pub mod tab;

/// Re-export parser module types for convenience
pub use parser::{Element, parse_html};
/// Re-export renderer functions
pub use renderer::render_to_string;
/// Re-export network functions and cache
pub use network::{fetch_url, ResourceCache};
/// Re-export tab system
pub use tab::{Tab, TabManager};
/// Re-export storage system
pub use storage::{StorageManager, Cookie, LocalStorage};

/// Main browser state with tab management and storage
pub struct Browser {
    tabs: TabManager,
    /// Global layout settings
    viewport_width: u32,
    /// Storage manager for cookies and localStorage
    storage: StorageManager,
}

impl Browser {
    /// Create a new browser instance with default viewport
    pub fn new() -> Self {
        Self {
            tabs: TabManager::new(),
            viewport_width: 1024,
            storage: StorageManager::new(),
        }
    }

    /// Load a URL into the current tab, creating a tab if necessary
    pub fn load_url(&mut self, url: &str, html_content: &str) {
        let dom = parser::parse_html(html_content);
        let title = extract_title(html_content);
        
        if let Some(tab) = self.tabs.active_tab_mut() {
            tab.set_url(url.to_string());
            tab.dom = dom;
            tab.title = title.to_string();
        } else {
            self.add_tab(url, dom, title.as_str());
        }
    }

    /// Add a new tab with content
    pub fn add_tab(&mut self, url: &str, dom: Element, title: &str) -> usize {
        self.tabs.add_tab(url, dom, title)
    }

    /// Get the currently active tab
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.active_tab()
    }

    /// Get mutable access to the active tab
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.active_tab_mut()
    }

    /// Go back in the current tab's history
    pub fn go_back(&mut self) {
        if let Some(tab) = self.tabs.active_tab_mut() {
            tab.go_back();
        }
    }

    /// Go forward in the current tab's history
    pub fn go_forward(&mut self) {
        if let Some(tab) = self.tabs.active_tab_mut() {
            tab.go_forward();
        }
    }

    /// Get tab count
    pub fn tab_count(&self) -> usize {
        self.tabs.tab_count()
    }

    /// Set viewport width
    pub fn set_viewport(&mut self, width: u32) {
        self.viewport_width = width;
    }

    /// Render the current tab's content to text (for headless testing)
    pub fn render_current(&self) -> String {
        if let Some(tab) = self.active_tab() {
            let css_rules: Vec<_> = vec![];
            match layout::create_layout_tree(&tab.dom, &css_rules, self.viewport_width) {
                Some(root) => {
                    let tr = text_renderer::TextRenderer::new(self.viewport_width, 768);
                    tr.render_to_text(&root)
                },
                None => String::from("[Error rendering content]"),
            }
        } else {
            String::from("[No active tab]")
        }
    }
}

/// Extract title from HTML (simple implementation)
fn extract_title(html: &str) -> String {
    // Simple title extraction - in production use proper parsing
    if let Some(start) = html.find("<title>") {
        let end = html[start..].find("</title>");
        if let Some(e) = end {
            return html[start + 7..start + e].to_string();
        }
    }
    // Fallback: use first heading or generic title
    if html.contains("<h1>") {
        "Untitled Page".to_string()
    } else {
        "GhitaBrowser".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_new() {
        let browser = Browser::new();
        assert_eq!(browser.tab_count(), 0);
        assert!(browser.active_tab().is_none());
    }

    #[test]
    fn test_browser_load_url() {
        let mut browser = Browser::new();
        let html = "<html><body><h1>Hello</h1></body></html>";
        browser.load_url("https://example.com", html);
        
        assert_eq!(browser.tab_count(), 1);
        assert!(browser.active_tab().is_some());
        assert_eq!(browser.active_tab().unwrap().url, "https://example.com");
    }

    #[test]
    fn test_browser_render() {
        let mut browser = Browser::new();
        browser.load_url("https://example.com", "<html><body><h1>Welcome</h1></body></html>");
        let rendered = browser.render_current();
        assert!(!rendered.is_empty());
        assert!(rendered.contains("Welcome"));
    }
}
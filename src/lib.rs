// src/lib.rs - Public re-exports for ghitabrowser crate (v0.1.5)
#![allow(dead_code)]

//! # GhitaBrowser
//! A lightweight Rust browser v0.1.5 - built from scratch in safe Rust.

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
pub mod javascript;
pub mod performance;

/// Re-export parser module types for convenience
pub use parser::{Element, parse_html};
/// Re-export renderer functions
pub use renderer::render_to_string;
/// Re-export network functions and cache
pub use network::{fetch_url, fetch_with_cache, FetchResult, ResourceCache, CacheStats};
/// Re-export tab system
pub use tab::{Tab, TabManager};
/// Re-export storage system
pub use storage::{StorageManager, Cookie, LocalStorage, CookieStore};
/// Re-export CSS parser
pub use css_parser::{parse_css, CssRule, ComputedStyle};
/// Re-export layout system
pub use layout::{LayoutNode, create_layout_tree, perform_layout};
/// Re-export JavaScript engine
pub use javascript::JsvEngine;
/// Re-export performance profiler
pub use performance::Profiler;

/// Performance statistics for monitoring
#[derive(Debug, Clone)]
pub struct RenderStats {
    pub parse_time_ms: u64,
    pub style_time_ms: u64,
    pub layout_time_ms: u64,
    pub render_time_ms: u64,
    pub total_time_ms: u64,
    pub dom_nodes: usize,
    pub layout_nodes: usize,
}

/// Main browser state with tab management, storage, and full rendering pipeline
pub struct Browser {
    tabs: TabManager,
    /// Global layout settings
    viewport_width: u32,
    viewport_height: u32,
    /// Storage manager for cookies and localStorage
    pub storage: StorageManager,
    /// Resource cache for network responses
    pub cache: ResourceCache,
    /// JavaScript engine
    pub js_engine: JsvEngine,
    /// Performance profiler
    pub profiler: Profiler,
    /// CSS rules (shared across pages, could be per-page)
    pub css_rules: Vec<CssRule>,
    /// Last render stats
    pub last_render_stats: Option<RenderStats>,
}

impl Browser {
    /// Create a new browser instance with default viewport
    pub fn new() -> Self {
        Self {
            tabs: TabManager::new(),
            viewport_width: 1100,
            viewport_height: 780,
            storage: StorageManager::new(),
            cache: ResourceCache::new(),
            js_engine: JsvEngine::new(),
            profiler: Profiler::new(),
            css_rules: Vec::new(),
            last_render_stats: None,
        }
    }

    /// Load a URL: fetch, parse, style, layout, render
    pub fn load_url(&mut self, url: &str) -> Result<String, String> {
        let start = std::time::Instant::now();
        
        // 1. Fetch HTML (with cache + cookie jar integration)
        let fetch_start = std::time::Instant::now();
        let fetch_result = network::fetch_with_cache(
            url,
            &mut self.cache,
            Some(self.storage.cookies_mut()),
        ).map_err(|e| format!("Network error: {}", e))?;
        let fetch_time = fetch_start.elapsed().as_millis() as u64;
        self.profiler.record("fetch", fetch_time);
        
        let html_content = &fetch_result.body;
        
        // 2. Parse HTML
        let parse_start = std::time::Instant::now();
        let dom = parser::parse_html(html_content);
        let parse_time = parse_start.elapsed().as_millis() as u64;
        self.profiler.record("parse", parse_time);
        
        // 3. Extract title from DOM
        let title = extract_title_from_dom(&dom);
        
        // 4. Apply styles - merge global CSS with page <style> tags
        let style_start = std::time::Instant::now();
        
        // Extract and parse <style> tags from the page
        let mut page_css_rules: Vec<css_parser::CssRule> = Vec::new();
        let style_elements = dom.find_all_tags("style");
        for style_elem in &style_elements {
            let css_text = style_elem.text.trim();
            if !css_text.is_empty() {
                let mut rules = css_parser::parse_css(css_text);
                page_css_rules.append(&mut rules);
            }
        }
        
        // Merge: global rules first, then page rules (page overrides global)
        let all_rules: Vec<css_parser::CssRule> = self.css_rules.iter()
            .cloned()
            .chain(page_css_rules)
            .collect();
        
        let style_time = style_start.elapsed().as_millis() as u64;
        self.profiler.record("style", style_time);
        
        // 5. Create layout with merged CSS rules
        let layout_start = std::time::Instant::now();
        let layout_tree = layout::create_layout_tree(&dom, &all_rules, self.viewport_width);
        let layout_time = layout_start.elapsed().as_millis() as u64;
        self.profiler.record("layout", layout_time);
        
        // Cache layout tree for re-rendering
        if let Some(ref _root) = layout_tree {
            if let Some(tab) = self.tabs.active_tab_mut() {
                tab.layout = layout_tree.clone();
            }
        }
        
        // 6. Render to text
        let render_start = std::time::Instant::now();
        let rendered = if let Some(root) = layout_tree {
            let tr = text_renderer::TextRenderer::new(self.viewport_width, self.viewport_height);
            tr.render_to_text(&root)
        } else {
            String::from("[Empty page]")
        };
        let render_time = render_start.elapsed().as_millis() as u64;
        self.profiler.record("render", render_time);
        
        // Count nodes
        let dom_nodes = count_elements(&dom);
        let layout_nodes = 0; // Simplified
        
        let total_time = start.elapsed().as_millis() as u64;
        
        self.last_render_stats = Some(RenderStats {
            parse_time_ms: parse_time,
            style_time_ms: style_time,
            layout_time_ms: layout_time,
            render_time_ms: render_time,
            total_time_ms: total_time,
            dom_nodes,
            layout_nodes,
        });
        
        // Update tab - save history entry then update
        if let Some(tab) = self.tabs.active_tab_mut() {
            // Save current state to history before navigating
            let current_entry = crate::tab::HistoryEntry {
                url: tab.url.clone(),
                title: tab.title.clone(),
                dom: tab.dom.clone(),
                layout: tab.layout.clone(),
            };
            tab.push_history(current_entry);

            // Update with new content
            tab.dom = dom;
            tab.title = title;
            tab.url = url.to_string();
        } else {
            self.tabs.add_tab(url, dom, &title);
        }

        Ok(rendered)
    }

    /// Load a URL with raw HTML content (for testing/offline)
    pub fn load_html(&mut self, url: &str, html_content: &str) -> Result<String, String> {
        let dom = parser::parse_html(html_content);
        let title = extract_title_from_dom(&dom);

        if let Some(tab) = self.tabs.active_tab_mut() {
            // Save current state to history
            let current_entry = crate::tab::HistoryEntry {
                url: tab.url.clone(),
                title: tab.title.clone(),
                dom: tab.dom.clone(),
                layout: tab.layout.clone(),
            };
            tab.push_history(current_entry);
            
            tab.dom = dom;
            tab.title = title;
            tab.url = url.to_string();
        } else {
            self.add_tab(url, dom, &title);
        }
        
        Ok(self.render_current())
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
    pub fn go_back(&mut self) -> bool {
        if let Some(tab) = self.tabs.active_tab_mut() {
            tab.go_back()
        } else {
            false
        }
    }

    /// Go forward in the current tab's history
    pub fn go_forward(&mut self) -> bool {
        if let Some(tab) = self.tabs.active_tab_mut() {
            tab.go_forward()
        } else {
            false
        }
    }

    /// Get tab count
    pub fn tab_count(&self) -> usize {
        self.tabs.tab_count()
    }

    /// Set viewport dimensions
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport_width = width;
        self.viewport_height = height;
    }

    /// Get viewport width
    pub fn viewport_width(&self) -> u32 {
        self.viewport_width
    }
    
    /// Get viewport height
    pub fn viewport_height(&self) -> u32 {
        self.viewport_height
    }
    
    /// Set global CSS rules
    pub fn set_css(&mut self, css: &str) {
        self.css_rules = css_parser::parse_css(css);
    }

    /// Render the current tab's content to text (for headless testing)
    pub fn render_current(&self) -> String {
        if let Some(tab) = self.active_tab() {
            // Use cached layout if available, otherwise rebuild
            if let Some(ref layout_root) = tab.layout {
                let tr = text_renderer::TextRenderer::new(self.viewport_width, self.viewport_height);
                tr.render_to_text(layout_root)
            } else {
                let css_rules = &self.css_rules;
                match layout::create_layout_tree(&tab.dom, css_rules, self.viewport_width) {
                    Some(root) => {
                        let tr = text_renderer::TextRenderer::new(self.viewport_width, self.viewport_height);
                        tr.render_to_text(&root)
                    },
                    None => String::from("[Error rendering content]"),
                }
            }
        } else {
            String::from("[No active tab]")
        }
    }
    
    /// Get status string for display
    pub fn status_string(&self) -> String {
        let cache_stats = self.cache.stats();
        
        format!(
            "Viewport: {}x{} | {} | Cookies: {} | Tabs: {}",
            self.viewport_width,
            self.viewport_height,
            cache_stats,
            self.storage.cookie_count(),
            self.tabs.tab_count(),
        )
    }
}

/// Extract title from parsed DOM tree
fn extract_title_from_dom(dom: &Element) -> String {
    if let Some(title_elem) = dom.find_tag("title") {
        return title_elem.text.trim().to_string();
    }
    if let Some(h1_elem) = dom.find_tag("h1") {
        return h1_elem.text.trim().to_string();
    }
    "Untitled Page".to_string()
}

/// Count total elements in DOM tree
fn count_elements(element: &Element) -> usize {
    1 + element.children.iter().map(count_elements).sum::<usize>()
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
    fn test_browser_load_html() {
        let mut browser = Browser::new();
        let html = "<html><body><h1>Hello</h1></body></html>";
        let _ = browser.load_html("https://example.com", html);
        
        assert_eq!(browser.tab_count(), 1);
        assert!(browser.active_tab().is_some());
        assert_eq!(browser.active_tab().unwrap().url, "https://example.com");
    }

    #[test]
    fn test_browser_render() {
        let mut browser = Browser::new();
        let _ = browser.load_html("https://example.com", "<html><body><h1>Welcome</h1></body></html>");
        let rendered = browser.render_current();
        assert!(!rendered.is_empty());
        assert!(rendered.contains("Welcome"));
    }
    
    #[test]
    fn test_browser_with_css() {
        let mut browser = Browser::new();
        browser.set_css("h1 { color: red; font-size: 24px; }");
        let _ = browser.load_html("https://example.com", "<html><body><h1>Styled</h1></body></html>");
        let rendered = browser.render_current();
        assert!(rendered.contains("Styled"));
    }
    
    #[test]
    fn test_browser_tab_switching() {
        let mut browser = Browser::new();
        let _ = browser.load_html("https://a.com", "<html><body><h1>Page A</h1></body></html>");
        browser.add_tab("https://b.com", parser::parse_html("<html><body><h1>Page B</h1></body></html>"), "Page B");
        
        assert_eq!(browser.tab_count(), 2);
        
        // Active tab should be the last added one
        assert_eq!(browser.active_tab().unwrap().url, "https://b.com");
    }
    
    #[test]
    fn test_extract_title() {
        let dom = parser::parse_html("<html><head><title>My Page</title></head><body></body></html>");
        assert_eq!(extract_title_from_dom(&dom), "My Page");
    }
}

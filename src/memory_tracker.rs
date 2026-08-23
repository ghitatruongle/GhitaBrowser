// Memory tracking and estimation for browser tabs and subsystems

use crate::image_loader::ImageCache;
use crate::layout::LayoutNode;
use crate::parser::Element;
use crate::tab::Tab;

/// Average bytes per DOM node (tag String + attrs HashMap + children Vec + text String + is_void).
/// Measured empirically: a typical <div class="foo">text</div> node costs ~200-250 bytes.
const BYTES_PER_DOM_NODE: usize = 210;

/// Average bytes per layout node (Element clone + RectModel + children Vec + ComputedStyle).
/// Layout nodes carry a full Element clone plus computed style, so they're heavier.
const BYTES_PER_LAYOUT_NODE: usize = 320;

/// Memory estimate for a single tab, broken down by subsystem.
#[derive(Debug, Clone, Default)]
pub struct TabMemoryEstimate {
    /// DOM tree memory (bytes)
    pub dom_bytes: usize,
    /// Layout tree memory (bytes)
    pub layout_bytes: usize,
    /// History stack memory (bytes)
    pub history_bytes: usize,
    /// Compressed DOM snapshots retained by navigation history.
    pub history_snapshot_bytes: usize,
    /// Snapshot retained while this tab is sleeping.
    pub sleep_snapshot_bytes: usize,
    /// Persistent JavaScript runtime / realm memory (bytes)
    pub runtime_bytes: usize,
    /// Total estimated memory for this tab (bytes)
    pub total_bytes: usize,
}

/// Memory estimate for the entire browser process.
#[derive(Debug, Clone, Default)]
pub struct BrowserMemoryEstimate {
    /// Per-tab breakdown
    pub tabs: Vec<TabMemoryEstimate>,
    /// Image cache memory (bytes)
    pub image_cache_bytes: usize,
    /// Resource cache memory (bytes) — sum of cached response bodies + overhead
    pub resource_cache_bytes: usize,
    /// Total estimated memory across all subsystems (bytes)
    pub total_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    pub soft_limit_bytes: usize,
    pub hard_limit_bytes: usize,
}

impl MemoryBudget {
    pub fn from_mb(soft_limit_mb: u32, hard_limit_mb: u32) -> Self {
        const MB: usize = 1024 * 1024;
        Self::from_bytes(
            (soft_limit_mb as usize).saturating_mul(MB),
            (hard_limit_mb as usize).saturating_mul(MB),
        )
    }

    pub fn from_bytes(soft_limit_bytes: usize, hard_limit_bytes: usize) -> Self {
        Self {
            soft_limit_bytes: soft_limit_bytes.min(hard_limit_bytes),
            hard_limit_bytes,
        }
    }

    pub fn level_for(self, bytes: usize) -> MemoryPressureLevel {
        if self.hard_limit_bytes == 0 {
            MemoryPressureLevel::Disabled
        } else if bytes >= self.hard_limit_bytes {
            MemoryPressureLevel::Critical
        } else if bytes >= self.soft_limit_bytes {
            MemoryPressureLevel::Moderate
        } else {
            MemoryPressureLevel::Normal
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryPressureLevel {
    Disabled,
    #[default]
    Normal,
    Moderate,
    Critical,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryReliefReport {
    pub level: MemoryPressureLevel,
    pub before_bytes: usize,
    pub after_bytes: usize,
    pub cache_bytes_freed: usize,
    pub slept_tabs: Vec<usize>,
    pub discarded_tabs: Vec<usize>,
}

/// Tracks and estimates memory usage across the browser.
///
/// This is an *estimation* — it does not read OS-level process memory.
/// Instead it walks internal data structures (DOM, layout, caches) and
/// multiplies node counts by empirically-derived average sizes.
///
/// The estimates are accurate to within ~15-20% of actual RSS for the
/// browser's own allocations, which is sufficient for:
/// - Comparing relative memory usage between tabs
/// - Triggering memory-pressure tab discarding
/// - Displaying per-tab memory in the Task Manager
#[derive(Debug, Default)]
pub struct MemoryTracker;

impl MemoryTracker {
    pub fn new() -> Self {
        Self
    }

    /// Estimate memory used by a single DOM tree.
    pub fn estimate_dom(dom: &Element) -> usize {
        let node_count = count_dom_nodes(dom);
        node_count.saturating_mul(BYTES_PER_DOM_NODE)
    }

    /// Estimate memory used by a layout tree.
    pub fn estimate_layout(layout: &LayoutNode) -> usize {
        let node_count = count_layout_nodes(layout);
        node_count.saturating_mul(BYTES_PER_LAYOUT_NODE)
    }

    /// Estimate memory used by a tab's history stack.
    /// History entries already own bounded compressed snapshots, so account
    /// their actual retained capacities instead of assuming a typical page.
    pub(crate) fn estimate_history(tab: &Tab) -> usize {
        tab.history_retained_bytes()
    }

    /// Estimate total memory for a single tab.
    pub fn estimate_tab(tab: &Tab) -> TabMemoryEstimate {
        let dom_bytes = Self::estimate_dom(&tab.dom);
        let layout_bytes = tab.layout.as_ref().map_or(0, Self::estimate_layout);
        let history_bytes = MemoryTracker::estimate_history(tab);
        // Sleeping tabs drop the live DOM but keep its serialized snapshot —
        // that retained memory must be counted, or the estimator reports a
        // sleeping 100 MB tab as ~200 bytes.
        let sleep_snapshot_bytes = tab.compressed_snapshot_bytes();
        let runtime_bytes = tab.runtime_heap_bytes();
        let total_bytes = dom_bytes
            .saturating_add(layout_bytes)
            .saturating_add(history_bytes)
            .saturating_add(sleep_snapshot_bytes)
            .saturating_add(runtime_bytes);

        TabMemoryEstimate {
            dom_bytes,
            layout_bytes,
            history_bytes,
            history_snapshot_bytes: history_bytes,
            sleep_snapshot_bytes,
            runtime_bytes,
            total_bytes,
        }
    }

    /// Estimate memory used by the image cache.
    pub fn estimate_image_cache(cache: &ImageCache) -> usize {
        cache.memory_usage()
    }

    /// Estimate memory used by the resource cache.
    /// Uses the cache's complete owned-byte accounting.
    pub fn estimate_resource_cache(cache: &crate::network::ResourceCache) -> usize {
        cache.total_bytes()
    }

    /// Estimate total browser memory across all tabs and caches.
    pub fn estimate_browser(
        tabs: &[&Tab],
        image_cache: &ImageCache,
        resource_cache: &crate::network::ResourceCache,
    ) -> BrowserMemoryEstimate {
        let tab_estimates: Vec<TabMemoryEstimate> =
            tabs.iter().map(|t| Self::estimate_tab(t)).collect();

        let tabs_total = tab_estimates.iter().fold(0usize, |total, estimate| {
            total.saturating_add(estimate.total_bytes)
        });
        let image_cache_bytes = Self::estimate_image_cache(image_cache);
        let resource_cache_bytes = Self::estimate_resource_cache(resource_cache);
        let total_bytes = tabs_total
            .saturating_add(image_cache_bytes)
            .saturating_add(resource_cache_bytes);

        BrowserMemoryEstimate {
            tabs: tab_estimates,
            image_cache_bytes,
            resource_cache_bytes,
            total_bytes,
        }
    }

    /// Convert bytes to megabytes (for display).
    pub fn bytes_to_mb(bytes: usize) -> f32 {
        bytes as f32 / (1024.0 * 1024.0)
    }

    /// Format bytes as a human-readable string (e.g., "45.2 MB", "1.3 GB").
    pub fn format_bytes(bytes: usize) -> String {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;

        let bytes_f = bytes as f64;
        if bytes_f >= GB {
            format!("{:.1} GB", bytes_f / GB)
        } else if bytes_f >= MB {
            format!("{:.1} MB", bytes_f / MB)
        } else if bytes_f >= KB {
            format!("{:.1} KB", bytes_f / KB)
        } else {
            format!("{} B", bytes)
        }
    }
}

/// Count total DOM nodes recursively with depth limit to prevent stack overflow.
/// Maximum depth: 1000 levels (prevents extremely nested HTML from causing stack overflow)
fn count_dom_nodes(element: &Element) -> usize {
    count_dom_nodes_with_depth(element, 0)
}

fn count_dom_nodes_with_depth(element: &Element, depth: usize) -> usize {
    if depth > 1000 {
        // Prevent stack overflow from extremely nested HTML
        return 1; // Count current node but stop recursing
    }
    1 + element
        .children
        .iter()
        .map(|child| count_dom_nodes_with_depth(child, depth + 1))
        .sum::<usize>()
}

/// Count total layout nodes recursively with depth limit to prevent stack overflow.
/// Maximum depth: 1000 levels (prevents extremely nested layout trees from causing stack overflow)
fn count_layout_nodes(node: &LayoutNode) -> usize {
    count_layout_nodes_with_depth(node, 0)
}

fn count_layout_nodes_with_depth(node: &LayoutNode, depth: usize) -> usize {
    if depth > 1000 {
        // Prevent stack overflow from extremely nested layout trees
        return 1; // Count current node but stop recursing
    }
    1 + node
        .children
        .iter()
        .map(|child| count_layout_nodes_with_depth(child, depth + 1))
        .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css_parser::parse_css;
    use crate::layout;

    #[test]
    fn test_count_dom_nodes_simple() {
        let dom = crate::parser::parse_html("<html><body><p>Hello</p></body></html>");
        let count = count_dom_nodes(&dom);
        // Parser produces: html > body > p = 3 nodes (no synthetic root wrapper)
        assert!(count >= 3, "Expected at least 3 nodes, got {}", count);
    }

    #[test]
    fn test_estimate_dom_nonzero() {
        let dom = crate::parser::parse_html(
            "<html><body><div><p>Hello</p><p>World</p></div></body></html>",
        );
        let bytes = MemoryTracker::estimate_dom(&dom);
        assert!(bytes > 0);
        // 5 nodes (html + body + div + p + p) * 210 bytes = 1050
        assert!(bytes >= 1050, "Expected >= 1050 bytes, got {}", bytes);
    }

    #[test]
    fn test_estimate_layout() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Title</h1><p>Content</p></body></html>");
        let rules = parse_css("h1 { font-size: 24px; } p { font-size: 16px; }");
        if let Some(layout_tree) = layout::create_layout_tree(&dom, &rules, 800) {
            let bytes = MemoryTracker::estimate_layout(&layout_tree);
            assert!(bytes > 0);
        }
    }

    #[test]
    fn test_estimate_tab() {
        let dom = crate::parser::parse_html("<html><body><h1>Test</h1></body></html>");
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        let estimate = MemoryTracker::estimate_tab(&tab);
        assert!(estimate.dom_bytes > 0);
        assert!(estimate.total_bytes >= estimate.dom_bytes);
    }

    #[test]
    fn test_estimate_tab_with_layout() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content here</p></body></html>");
        let rules = parse_css("h1 { font-size: 24px; }");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom.clone(),
            "Test".to_string(),
        );
        if let Some(layout_tree) = layout::create_layout_tree(&dom, &rules, 800) {
            tab.layout = Some(layout_tree);
        }
        let estimate = MemoryTracker::estimate_tab(&tab);
        assert!(estimate.layout_bytes > 0);
        assert!(estimate.total_bytes > estimate.dom_bytes);
    }

    #[test]
    fn test_estimate_browser() {
        let dom1 = crate::parser::parse_html("<html><body><h1>Tab 1</h1></body></html>");
        let dom2 = crate::parser::parse_html(
            "<html><body><h1>Tab 2</h1><p>More content</p></body></html>",
        );
        let tab1 = Tab::new(1, "https://a.com".to_string(), dom1, "A".to_string());
        let tab2 = Tab::new(2, "https://b.com".to_string(), dom2, "B".to_string());
        let tabs: Vec<&Tab> = vec![&tab1, &tab2];
        let image_cache = ImageCache::new();
        let resource_cache = crate::network::ResourceCache::new();
        let estimate = MemoryTracker::estimate_browser(&tabs, &image_cache, &resource_cache);
        assert_eq!(estimate.tabs.len(), 2);
        assert!(estimate.total_bytes > 0);
    }

    #[test]
    fn test_bytes_to_mb() {
        assert_eq!(MemoryTracker::bytes_to_mb(0), 0.0);
        assert!((MemoryTracker::bytes_to_mb(1024 * 1024) - 1.0).abs() < 0.01);
        assert!((MemoryTracker::bytes_to_mb(5242880) - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(MemoryTracker::format_bytes(0), "0 B");
        assert_eq!(MemoryTracker::format_bytes(512), "512 B");
        assert!(MemoryTracker::format_bytes(1536).contains("KB"));
        assert!(MemoryTracker::format_bytes(5_242_880).contains("MB"));
        assert!(MemoryTracker::format_bytes(2_147_483_648).contains("GB"));
    }

    #[test]
    fn test_empty_dom_estimate() {
        let dom = Element::new("root");
        let bytes = MemoryTracker::estimate_dom(&dom);
        assert_eq!(bytes, BYTES_PER_DOM_NODE); // just the root node
    }

    #[test]
    fn test_history_estimate() {
        let dom = crate::parser::parse_html("<html><body><h1>Test</h1></body></html>");
        let mut tab = Tab::new(1, "https://a.com".to_string(), dom.clone(), "A".to_string());
        // Seed entry = 1 history entry
        assert_eq!(tab.history_len(), 1);

        // Navigate to a new page
        tab.push_history(crate::tab::HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &dom,
        ));
        assert_eq!(tab.history_len(), 2);

        let estimate = MemoryTracker::estimate_tab(&tab);
        assert_eq!(estimate.history_bytes, tab.history_retained_bytes());
        assert!(estimate.history_bytes > 0);
    }

    #[test]
    fn history_estimate_uses_retained_snapshot_bytes() {
        let small = crate::tab::HistoryEntry::new(
            "https://example.test/small".to_string(),
            "Small".to_string(),
            &crate::parser::parse_html("<p>x</p>"),
        );
        let large = crate::tab::HistoryEntry::new(
            "https://example.test/large".to_string(),
            "Large".to_string(),
            &crate::parser::parse_html(&format!(
                "<main>{}</main>",
                "meaningful text ".repeat(20_000)
            )),
        );
        assert!(large.retained_bytes() > small.retained_bytes());
    }

    #[test]
    fn memory_budget_levels_are_deterministic() {
        let budget = MemoryBudget::from_mb(400, 500);
        assert_eq!(
            budget.level_for(399 * 1024 * 1024),
            MemoryPressureLevel::Normal
        );
        assert_eq!(
            budget.level_for(400 * 1024 * 1024),
            MemoryPressureLevel::Moderate
        );
        assert_eq!(
            budget.level_for(500 * 1024 * 1024),
            MemoryPressureLevel::Critical
        );
    }
}

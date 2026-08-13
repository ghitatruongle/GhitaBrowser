// Tab manager and history

use crate::layout::LayoutNode;
use crate::parser::Element;
use std::collections::HashMap;
use std::io::Write;

/// A snapshot of page state for history navigation.
/// The DOM is stored in compressed form to save memory.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    /// Compressed DOM data (serialized and optionally compressed)
    compressed_dom: Option<Vec<u8>>,
    /// Whether the compressed data is available
    has_dom: bool,
}

impl HistoryEntry {
    /// Create a new history entry from a DOM tree
    pub fn new(url: String, title: String, dom: &Element) -> Self {
        let compressed_dom = Self::compress_dom(dom);
        let has_dom = compressed_dom.is_some();
        Self {
            url,
            title,
            compressed_dom,
            has_dom,
        }
    }

    /// Get the DOM tree (decompress if needed)
    pub fn get_dom(&self) -> Option<Element> {
        self.compressed_dom
            .as_ref()
            .and_then(|data| decompress_dom_bytes(data).ok())
    }

    /// Check if this entry has a decompressable DOM
    pub fn has_dom(&self) -> bool {
        self.has_dom
    }

    /// Compress a DOM tree into a compact binary representation
    fn compress_dom(dom: &Element) -> Option<Vec<u8>> {
        // Don't compress trivial DOMs
        if dom.children.is_empty() && dom.tag == "root" {
            return None;
        }

        // Serialize to JSON first
        let json = serde_json::to_vec(dom).ok()?;

        // Compress with gzip for better compression ratio
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&json).ok()?;
        let compressed = encoder.finish().ok()?;

        // Only use compressed data if it's actually smaller
        if compressed.len() < json.len() {
            Some(compressed)
        } else {
            // Fall back to uncompressed if compression doesn't help
            Some(json)
        }
    }
}

/// Decompress DOM snapshot bytes back into a DOM tree.
///
/// Handles BOTH formats produced by `compress_dom`:
/// - gzip-compressed JSON (when gzip was smaller), identified by the
///   gzip magic header `0x1f 0x8b`
/// - raw JSON (fallback when compression didn't help)
///
/// Returns an error if the data is neither valid gzip nor valid JSON.
fn decompress_dom_bytes(data: &[u8]) -> Result<Element, Box<dyn std::error::Error>> {
    const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];

    let json = if data.starts_with(GZIP_MAGIC) {
        // gzip-compressed JSON
        let mut decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(data));
        std::io::read_to_string(&mut decoder)
            .map_err(|e| format!("Failed to decompress DOM data: {}", e))?
    } else {
        // Plain JSON (fallback format)
        String::from_utf8(data.to_vec()).map_err(|e| format!("Failed to read DOM JSON: {}", e))?
    };

    // Lift serde's 128-container recursion limit so DOMs nested deeper (up to
    // the parser's MAX_DOM_DEPTH) round-trip through history/sleep snapshots.
    // Depth is pre-bounded by json_nesting_depth below, so serde's unbounded
    // mode is never reached with truly pathological input.
    if json_nesting_depth(&json) > SNAPSHOT_MAX_JSON_DEPTH {
        return Err(format!(
            "DOM snapshot too deeply nested (>{}) to restore safely",
            SNAPSHOT_MAX_JSON_DEPTH
        )
        .into());
    }
    let mut de = serde_json::Deserializer::from_str(&json);
    de.disable_recursion_limit();
    let dom: Element = serde::Deserialize::deserialize(&mut de)
        .map_err(|e| format!("Failed to parse decompressed DOM JSON: {}", e))?;

    Ok(flatten_deep(dom, crate::parser::MAX_DOM_DEPTH))
}

/// Maximum JSON container nesting a DOM snapshot may have before we refuse to
/// restore it. Serde's unbounded mode recurses on the real stack, so anything
/// past this is rejected up front — an attacker with a crafted snapshot can't
/// make the browser overflow its stack while deserializing.
const SNAPSHOT_MAX_JSON_DEPTH: usize = 1024;

/// Measure the maximum `{`/`[` nesting depth of a JSON string, iteratively and
/// without allocating. String contents (and backslash escapes) are skipped, so
/// a `{` or `[` inside a string value is not counted.
fn json_nesting_depth(json: &str) -> usize {
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for b in json.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max_depth
}

/// Iteratively flatten a DOM tree so no node is nested deeper than `max_depth`.
///
/// Legacy snapshots written before the parser's depth cap existed can be
/// arbitrarily deep; using recursion there (drop, layout, counting) would
/// overflow the stack. This walk is iterative end-to-end, so it is safe at
/// any input depth: nodes deeper than the cap are re-attached to their
/// nearest ancestor at the capped depth instead of nested further.
fn flatten_deep(root: Element, max_depth: usize) -> Element {
    // Phase 1: flat, pre-order list of (element, depth) — iterative.
    let mut flat: Vec<(Element, usize)> = Vec::new();
    let mut st: Vec<(Element, usize)> = vec![(root, 0)];
    while let Some((mut el, depth)) = st.pop() {
        let children = std::mem::take(&mut el.children);
        for child in children.into_iter().rev() {
            st.push((child, depth + 1));
        }
        flat.push((el, depth));
    }

    // Phase 2: rebuild with the depth cap — iterative.
    let mut stack: Vec<Element> = Vec::new();
    let mut depths: Vec<usize> = Vec::new();
    for (el, raw_depth) in flat {
        let eff_depth = raw_depth.min(max_depth);
        // Attach completed subtrees: any top whose depth >= eff_depth is
        // closed into the node that will be the new sibling's parent.
        while !depths.is_empty() && *depths.last().unwrap() >= eff_depth {
            let child = stack.pop().unwrap();
            depths.pop();
            if let Some(parent) = stack.last_mut() {
                parent.add_child(child);
            }
        }
        stack.push(el);
        depths.push(eff_depth);
    }
    while stack.len() > 1 {
        let child = stack.pop().unwrap();
        depths.pop();
        if let Some(parent) = stack.last_mut() {
            parent.add_child(child);
        }
    }
    stack.pop().unwrap_or_else(|| Element::new("html"))
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
    /// Pinned tabs remain at the beginning of tab strip
    pub is_pinned: bool,
    /// Memory Saver: Inactive tab is hibernated to save RAM (Chrome feature)
    pub is_sleeping: bool,
    /// Unix timestamp of when the tab was last selected
    pub last_active_timestamp: i64,
    /// Unix timestamp of when the tab was put to sleep (None if not sleeping)
    pub slept_at: Option<i64>,
    /// True when the tab is producing sound (media playback). Used by memory-pressure
    /// discard to avoid killing tabs the user is actively listening to.
    pub is_audible: bool,
    /// User mute state. Audible media must consult this independently of
    /// whether the page is currently producing sound.
    pub is_muted: bool,
    /// Optional browser-owned tab-group identifier.
    pub group_id: Option<u64>,
    /// True when this tab was discarded by the memory-pressure monitor.
    /// Discarded tabs show a reload icon and are restored on next click.
    pub is_discarded: bool,
    /// Compressed DOM data for sleeping tabs. When a tab is put to sleep,
    /// the DOM is serialized into this compact binary format to preserve
    /// it for faster wake (no network refetch) while using less memory.
    pub compressed_dom: Option<Vec<u8>>,
}

/// Result of waking a sleeping tab.
#[derive(Debug, PartialEq)]
pub enum WakeResult {
    /// Tab was not sleeping — no action taken.
    NotSleeping,
    /// DOM was restored from compressed data — no network reload needed.
    /// Caller should re-layout and re-render the restored DOM.
    RestoredFromCache,
    /// No compressed data available — caller should reload from the given URL.
    NeedsReload(String),
}

/// Maximum number of entries kept per tab's history stack. Each entry holds a
/// full serialized DOM snapshot; unbounded growth lets a long session leak
/// memory without bound.
const MAX_HISTORY_ENTRIES: usize = 60;

impl Tab {
    pub fn new(id: usize, url: String, dom: Element, title: String) -> Self {
        let entry = HistoryEntry::new(url.clone(), title.clone(), &dom);
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
            is_pinned: false,
            is_sleeping: false,
            last_active_timestamp: chrono::Utc::now().timestamp(),
            slept_at: None,
            is_audible: false,
            is_muted: false,
            group_id: None,
            is_discarded: false,
            compressed_dom: None,
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
        // Bound the stack: drop the oldest entry when at the cap so long
        // sessions don't accumulate unbounded serialized DOM snapshots.
        if self.history.len() > MAX_HISTORY_ENTRIES {
            self.history.remove(0);
        }
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
                self.dom = entry.get_dom().unwrap_or_else(|| Element::new("root"));
                self.layout = None; // Will be recomputed on render
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
            self.dom = entry.get_dom().unwrap_or_else(|| Element::new("root"));
            self.layout = None; // Will be recomputed on render
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
            self.dom = entry.get_dom().unwrap_or_else(|| Element::new("root"));
            self.layout = None; // Will be recomputed on render
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

    /// Return the number of entries in this tab's history stack.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Bytes retained by the sleeping-tab DOM snapshot (if any). Used by the
    /// memory estimator so the estimate for a sleeping tab reflects the
    /// snapshot it actually keeps alive, not just the empty root DOM.
    pub(crate) fn compressed_snapshot_bytes(&self) -> usize {
        self.compressed_dom.as_ref().map_or(0, |d| d.len())
    }

    /// Hibernate the tab to save memory (Chrome Memory Saver).
    /// Compresses the DOM tree into a compact binary format and drops
    /// the in-memory tree and layout tree. Preserves:
    /// - URL, title, incognito flag
    /// - History stack (needed for back/forward)
    /// - Pinned status, last_active_timestamp
    /// - Compressed DOM for fast wake (no network refetch)
    ///
    /// Sets `is_sleeping = true` and records sleep time.
    ///
    /// Returns the estimated bytes freed (DOM + layout size minus compressed size).
    pub fn sleep(&mut self) -> usize {
        if self.is_sleeping {
            return 0;
        }

        // Estimate memory before dropping (for reporting)
        let dom_nodes = crate::count_elements(&self.dom);
        let layout_nodes = self
            .layout
            .as_ref()
            .map(crate::layout::count_layout_nodes)
            .unwrap_or(0);

        // ~210 bytes per DOM node, ~320 bytes per layout node (from memory_tracker)
        let original_size = dom_nodes * 210 + layout_nodes * 320;

        // Compress the DOM before dropping it
        self.compressed_dom = self.compress_dom();

        let compressed_size = self.compressed_dom.as_ref().map_or(0, |d| d.len());
        let freed_bytes = original_size.saturating_sub(compressed_size);

        // Drop the heavy in-memory data
        self.dom = Element::new("root");
        self.layout = None;
        self.is_sleeping = true;
        self.slept_at = Some(chrono::Utc::now().timestamp());

        freed_bytes
    }

    /// Wake a sleeping tab. Attempts to restore from compressed DOM first
    /// (fast, no network). Falls back to network reload if no compressed data.
    pub fn wake(&mut self) -> WakeResult {
        if !self.is_sleeping {
            return WakeResult::NotSleeping;
        }

        self.is_sleeping = false;
        self.slept_at = None;
        self.last_active_timestamp = chrono::Utc::now().timestamp();
        self.layout = None;

        // Try to restore from compressed DOM (fast path — no network)
        if let Some(ref compressed) = self.compressed_dom {
            if let Ok(dom) = Self::decompress_dom(compressed) {
                self.dom = dom;
                self.compressed_dom = None;
                return WakeResult::RestoredFromCache;
            }
        }

        // No compressed data or decompression failed — need network reload
        self.dom = Element::new("root");
        self.compressed_dom = None;

        WakeResult::NeedsReload(self.url.clone())
    }

    /// Check if this tab should be considered for sleeping.
    /// Returns true if:
    /// - Not already sleeping
    /// - Not discarded (discarded tabs are even lighter and must not sleep)
    /// - Not audible (tabs playing media must not be hibernated)
    /// - Not pinned (pinned tabs are excluded by default)
    /// - Not an internal page (ghita:// pages don't consume much)
    /// - Not showing an error page
    pub fn can_sleep(&self) -> bool {
        if self.is_sleeping {
            return false;
        }
        if self.is_discarded {
            return false;
        }
        if self.is_audible {
            return false;
        }
        if self.is_pinned {
            return false;
        }
        if self.is_error {
            return false;
        }
        // Don't sleep internal pages — they're lightweight
        if self.url.starts_with("ghita://") {
            return false;
        }
        // Don't sleep tabs with no loaded content
        if self.url.is_empty() || self.url == "about:blank" {
            return false;
        }
        true
    }

    /// Returns how long ago the tab was last active, in seconds.
    /// Returns 0 if the tab is currently active.
    pub fn seconds_since_active(&self) -> i64 {
        let now = chrono::Utc::now().timestamp();
        now - self.last_active_timestamp
    }

    /// Mark the tab as just activated (updates last_active_timestamp).
    pub fn mark_active(&mut self) {
        self.last_active_timestamp = chrono::Utc::now().timestamp();
    }

    /// Compress the current DOM tree into a compact binary representation.
    /// Uses JSON serialization via serde with gzip compression for efficiency.
    /// Returns None if the DOM is empty or serialization fails.
    fn compress_dom(&self) -> Option<Vec<u8>> {
        // Don't compress trivial DOMs (just root with no children)
        if self.dom.children.is_empty() && self.dom.tag == "root" {
            return None;
        }

        // Serialize to JSON first
        let json = match serde_json::to_vec(&self.dom) {
            Ok(data) => data,
            Err(e) => {
                log::warn!("Failed to compress DOM for {}: {}", self.url, e);
                return None;
            }
        };

        // Compress with gzip
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        match encoder.write_all(&json) {
            Ok(_) => match encoder.finish() {
                Ok(compressed) => {
                    // Only use compressed data if it's actually smaller
                    if compressed.len() < json.len() {
                        Some(compressed)
                    } else {
                        log::warn!(
                            "Failed to compress DOM for {}: compression not beneficial",
                            self.url
                        );
                        Some(json)
                    }
                }
                Err(e) => {
                    log::warn!("Failed to finish gzip encoding for {}: {}", self.url, e);
                    Some(json)
                }
            },
            Err(e) => {
                log::warn!("Failed to write to gzip encoder for {}: {}", self.url, e);
                Some(json)
            }
        }
    }

    /// Decompress a binary representation back into a DOM tree.
    fn decompress_dom(data: &[u8]) -> Result<Element, Box<dyn std::error::Error>> {
        decompress_dom_bytes(data)
    }

    /// Calculate a discard priority score for memory-pressure tab discarding.
    ///
    /// Scoring formula:
    ///   base = (seconds_since_active / 60) * 10   (10 points per minute of inactivity)
    ///   pinned: -100 (strongly protect pinned tabs)
    ///   audible: -200 (never discard tabs playing audio/video)
    ///   sleeping: +50 (already sleeping, discarding frees more)
    ///   discarded: -1000 (never discard an already-discarded tab)
    ///   active: -10000 (never discard the active tab)
    pub fn discard_score(&self, active_tab_id: Option<usize>) -> i64 {
        if self.is_discarded {
            return -1000;
        }
        if Some(self.id) == active_tab_id {
            return -10000;
        }

        let mut score: i64 = 0;

        // Inactivity: 10 points per minute
        let minutes_inactive = self.seconds_since_active() / 60;
        score += minutes_inactive * 10;

        // Penalties (negative = protect from discard)
        if self.is_pinned {
            score -= 100;
        }
        if self.is_audible {
            score -= 200;
        }

        // Bonus: sleeping tabs are cheaper to discard (already dropped DOM)
        if self.is_sleeping {
            score += 50;
        }

        // Internal pages are lightweight — slight preference to discard
        if self.url.starts_with("ghita://") {
            score += 20;
        }

        score
    }

    /// Mark this tab as discarded by the memory-pressure monitor.
    /// Drops DOM and layout (like sleep) but sets is_discarded instead.
    pub fn discard(&mut self) -> usize {
        if self.is_discarded {
            return 0;
        }

        let dom_nodes = crate::count_elements(&self.dom);
        let layout_nodes = self
            .layout
            .as_ref()
            .map(crate::layout::count_layout_nodes)
            .unwrap_or(0);
        let snapshot_bytes = self.compressed_dom.as_ref().map_or(0, |d| d.len());
        let freed_bytes = dom_nodes * 210 + layout_nodes * 320 + snapshot_bytes;

        self.dom = Element::new("root");
        self.layout = None;
        // The sleeping snapshot can be as large as the DOM it was meant to
        // replace — discarding must release it too, or the "memory saved"
        // figure is fiction and the stale snapshot stays resident forever.
        self.compressed_dom = None;
        self.is_sleeping = false;
        self.is_discarded = true;
        self.slept_at = None;

        freed_bytes
    }

    /// Restore a discarded tab. Returns the URL to reload.
    pub fn undiscard(&mut self) -> Option<String> {
        if !self.is_discarded {
            return None;
        }

        self.is_discarded = false;
        self.last_active_timestamp = chrono::Utc::now().timestamp();
        self.dom = Element::new("root");
        self.layout = None;

        Some(self.url.clone())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TabGroup {
    pub id: u64,
    pub name: String,
    pub color: String,
    pub collapsed: bool,
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
    groups: HashMap<u64, TabGroup>,
    next_group_id: u64,
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
            groups: HashMap::new(),
            next_group_id: 1,
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
            if let Some(tab) = self.tabs.get_mut(&id) {
                // Inactivity drives Memory Saver sleep and memory-pressure
                // discard selection; without this, "time since last viewed"
                // would be measured from tab creation instead of last use.
                tab.mark_active();
            }
        }
    }

    /// Activate a tab by its position in the tab bar
    pub fn set_active_by_index(&mut self, index: usize) {
        if let Some(&id) = self.tab_order.get(index) {
            self.active_tab_id = Some(id);
            if let Some(tab) = self.tabs.get_mut(&id) {
                tab.mark_active();
            }
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
        self.groups.clear();
        self.next_group_id = 1;
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

    /// Position of active tab in tab bar (0-indexed)
    pub fn active_index(&self) -> usize {
        self.active_tab_id
            .and_then(|id| self.tab_order.iter().position(|&tid| tid == id))
            .unwrap_or(0)
    }

    /// Iterate tabs in UI order
    pub fn iter_tabs(&self) -> Vec<&Tab> {
        self.tab_order
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .collect()
    }

    /// Alias for iter_tabs: iterate tabs in UI order
    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tab_order.iter().filter_map(|id| self.tabs.get(id))
    }

    pub fn groups(&self) -> &HashMap<u64, TabGroup> {
        &self.groups
    }

    pub fn pin_tab_by_index(&mut self, index: usize, pinned: bool) -> bool {
        let Some(tab_id) = self.tab_order.get(index).copied() else {
            return false;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return false;
        };
        if tab.is_pinned == pinned {
            return true;
        }
        tab.is_pinned = pinned;
        self.tab_order.remove(index);
        let pinned_count = self
            .tab_order
            .iter()
            .filter(|id| self.tabs.get(id).is_some_and(|tab| tab.is_pinned))
            .count();
        // Both transitions land at the pin partition boundary: newly pinned
        // tabs follow existing pins, and newly unpinned tabs precede the first
        // existing unpinned tab.
        self.tab_order.insert(pinned_count, tab_id);
        true
    }

    pub fn toggle_mute_by_index(&mut self, index: usize) -> Option<bool> {
        let tab_id = *self.tab_order.get(index)?;
        let tab = self.tabs.get_mut(&tab_id)?;
        tab.is_muted = !tab.is_muted;
        Some(tab.is_muted)
    }

    pub fn create_group(&mut self, name: &str, color: &str) -> Result<u64, String> {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("Tab group name must contain 1-64 bytes".to_string());
        }
        if color.len() > 32 {
            return Err("Tab group color exceeds 32 bytes".to_string());
        }
        let id = self.next_group_id;
        self.next_group_id = self.next_group_id.saturating_add(1);
        self.groups.insert(
            id,
            TabGroup {
                id,
                name: name.to_string(),
                color: color.to_string(),
                collapsed: false,
            },
        );
        Ok(id)
    }

    pub fn assign_tab_to_group_by_index(&mut self, index: usize, group_id: Option<u64>) -> bool {
        if group_id.is_some_and(|id| !self.groups.contains_key(&id)) {
            return false;
        }
        let Some(tab_id) = self.tab_order.get(index).copied() else {
            return false;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return false;
        };
        tab.group_id = group_id;
        true
    }

    pub fn reorder_tab(&mut self, from_index: usize, to_index: usize) -> bool {
        if from_index >= self.tab_order.len() || to_index >= self.tab_order.len() {
            return false;
        }
        let moving_id = self.tab_order[from_index];
        let moving_pinned = self.tabs.get(&moving_id).is_some_and(|tab| tab.is_pinned);
        let target_id = self.tab_order[to_index];
        let target_pinned = self.tabs.get(&target_id).is_some_and(|tab| tab.is_pinned);
        if moving_pinned != target_pinned {
            return false;
        }
        let tab_id = self.tab_order.remove(from_index);
        self.tab_order.insert(to_index, tab_id);
        true
    }

    pub fn session_snapshot(&self) -> crate::storage::BrowserSession {
        crate::storage::BrowserSession {
            active_index: self.active_index(),
            tabs: self
                .iter()
                .filter(|tab| {
                    !tab.incognito
                        && matches!(
                            url::Url::parse(&tab.url).ok().map(|url| url.scheme().to_string()),
                            Some(scheme) if matches!(scheme.as_str(), "http" | "https" | "file")
                        )
                })
                .map(|tab| crate::storage::SessionTab {
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                    pinned: tab.is_pinned,
                    muted: tab.is_muted,
                    group_id: tab.group_id,
                })
                .collect(),
            groups: self
                .groups
                .values()
                .map(|group| crate::storage::SessionTabGroup {
                    id: group.id,
                    name: group.name.clone(),
                    color: group.color.clone(),
                    collapsed: group.collapsed,
                })
                .collect(),
        }
    }

    pub fn restore_session(&mut self, session: &crate::storage::BrowserSession) -> usize {
        self.close_all_tabs();
        for group in &session.groups {
            self.next_group_id = self.next_group_id.max(group.id.saturating_add(1));
            self.groups.insert(
                group.id,
                TabGroup {
                    id: group.id,
                    name: group.name.clone(),
                    color: group.color.clone(),
                    collapsed: group.collapsed,
                },
            );
        }
        for saved in session.tabs.iter().take(100) {
            let id = self.add_tab(
                &saved.url,
                Element::new("root"),
                if saved.title.trim().is_empty() {
                    &saved.url
                } else {
                    &saved.title
                },
            );
            if let Some(tab) = self.tabs.get_mut(&id) {
                tab.is_pinned = saved.pinned;
                tab.is_muted = saved.muted;
                tab.group_id = saved.group_id.filter(|id| self.groups.contains_key(id));
            }
        }
        self.tab_order
            .sort_by_key(|id| !self.tabs.get(id).is_some_and(|tab| tab.is_pinned));
        if !self.tab_order.is_empty() {
            let index = session.active_index.min(self.tab_order.len() - 1);
            self.set_active_by_index(index);
        }
        self.tab_order.len()
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

        let entry_b = HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &Element::new("body"),
        );
        tab.push_history(entry_b);

        let entry_c = HistoryEntry::new(
            "https://c.com".to_string(),
            "C".to_string(),
            &Element::new("body"),
        );
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
        let entry = HistoryEntry::new(
            "https://a.com".to_string(),
            "A (reloaded)".to_string(),
            &Element::new("body"),
        );
        tab.push_history(entry);
        assert_eq!(tab.history.len(), 1);
        assert_eq!(tab.history[0].title, "A (reloaded)");

        // A different URL is appended normally
        let entry_b = HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &Element::new("body"),
        );
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
        let entry_a = HistoryEntry::new(
            "https://a.com".to_string(),
            "A".to_string(),
            &Element::new("body"),
        );
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
            tab.push_history(HistoryEntry::new(
                url.to_string(),
                url.to_string(),
                &Element::new("body"),
            ));
        }
        // Back to b.com, then navigate to d.com — forward entries must drop
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://b.com");
        assert!(tab.can_go_forward());
        tab.push_history(HistoryEntry::new(
            "https://d.com".to_string(),
            "D".to_string(),
            &Element::new("body"),
        ));
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

    // ===== v1.2.0: Tab Hibernation Tests =====

    #[test]
    fn test_tab_sleep_drops_dom_and_layout() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content here</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Example".to_string(),
        );

        // Verify tab has content before sleep
        assert!(!tab.is_sleeping);
        assert!(tab.dom.find_tag("h1").is_some());
        assert_eq!(tab.url, "https://example.com");

        // Put tab to sleep
        let freed = tab.sleep();
        assert!(tab.is_sleeping);
        assert!(freed > 0, "Sleep should free some bytes");

        // DOM should be replaced with empty root
        assert!(tab.dom.find_tag("h1").is_none());
        assert_eq!(tab.dom.tag, "root");

        // Layout should be dropped
        assert!(tab.layout.is_none());

        // URL and title preserved
        assert_eq!(tab.url, "https://example.com");
        assert_eq!(tab.title, "Example");

        // slept_at should be set
        assert!(tab.slept_at.is_some());
    }

    #[test]
    fn test_tab_sleep_is_idempotent() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let freed1 = tab.sleep();
        assert!(freed1 > 0);
        assert!(tab.is_sleeping);

        // Second sleep should return 0 (already sleeping)
        let freed2 = tab.sleep();
        assert_eq!(freed2, 0);
    }

    #[test]
    fn test_tab_wake_restores_from_compressed() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        tab.sleep();
        assert!(tab.is_sleeping);

        // Wake the tab — should restore from compressed DOM
        let result = tab.wake();
        assert!(
            matches!(result, WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );
        assert!(!tab.is_sleeping);
        assert!(tab.slept_at.is_none());

        // DOM is restored from compressed data (fast path — no network needed)
        assert_ne!(tab.dom.tag, "root");
        assert!(tab.dom.find_tag("html").is_some());
        assert!(tab.layout.is_none());
    }

    #[test]
    fn test_tab_wake_noop_when_not_sleeping() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        // Wake on a non-sleeping tab returns NotSleeping
        let result = tab.wake();
        assert!(
            matches!(result, WakeResult::NotSleeping),
            "Should return NotSleeping"
        );
        assert!(!tab.is_sleeping);
    }

    #[test]
    fn test_can_sleep_regular_tab() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        assert!(tab.can_sleep());
    }

    #[test]
    fn test_can_sleep_pinned_tab() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.is_pinned = true;
        assert!(!tab.can_sleep());
    }

    #[test]
    fn test_can_sleep_internal_page() {
        let dom = Element::new("body");
        let tab = Tab::new(1, "ghita://newtab".to_string(), dom, "New Tab".to_string());
        assert!(!tab.can_sleep());
    }

    #[test]
    fn test_can_sleep_already_sleeping() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.sleep();
        assert!(!tab.can_sleep());
    }

    #[test]
    fn test_can_sleep_error_page() {
        let dom = Element::new("body");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Error".to_string(),
        );
        tab.is_error = true;
        assert!(!tab.can_sleep());
    }

    #[test]
    fn test_mark_active_updates_timestamp() {
        let dom = Element::new("body");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        let initial_ts = tab.last_active_timestamp;

        // Small delay to ensure timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(10));
        tab.mark_active();

        assert!(tab.last_active_timestamp >= initial_ts);
    }

    #[test]
    fn test_sleep_wake_cycle_preserves_history() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(1, "https://a.com".to_string(), dom.clone(), "A".to_string());

        // Navigate to build history
        tab.push_history(HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &dom,
        ));
        tab.push_history(HistoryEntry::new(
            "https://c.com".to_string(),
            "C".to_string(),
            &dom,
        ));
        assert_eq!(tab.history_len(), 3);

        // Sleep
        tab.sleep();
        assert!(tab.is_sleeping);

        // History should be preserved
        assert_eq!(tab.history_len(), 3);

        // Wake — should restore from compressed DOM
        let wake_result = tab.wake();
        assert!(!tab.is_sleeping);
        assert!(
            matches!(wake_result, WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );

        // History still preserved
        assert_eq!(tab.history_len(), 3);
    }

    // ===== v1.2.0: Memory Pressure Discard Tests =====

    #[test]
    fn test_discarded_tab_has_lowest_score() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        let normal_score = tab.discard_score(Some(2));

        tab.discard();
        let discarded_score = tab.discard_score(Some(2));

        assert!(
            discarded_score < normal_score,
            "Discarded tab should have lower score: discarded={} normal={}",
            discarded_score,
            normal_score
        );
    }

    #[test]
    fn test_active_tab_has_very_low_score() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let active_score = tab.discard_score(Some(1)); // this tab is active
        let inactive_score = tab.discard_score(Some(2)); // this tab is inactive

        assert!(
            active_score < inactive_score,
            "Active tab should have much lower score: active={} inactive={}",
            active_score,
            inactive_score
        );
        assert!(
            active_score < -5000,
            "Active tab should be heavily protected"
        );
    }

    #[test]
    fn test_pinned_tab_protected() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.is_pinned = true;

        let pinned_score = tab.discard_score(Some(2));

        // Make a comparable unpinned tab that's been inactive
        let dom2 = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab2 = Tab::new(
            2,
            "https://example.com".to_string(),
            dom2,
            "Test".to_string(),
        );
        tab2.last_active_timestamp -= 600; // 10 minutes inactive

        let unpinned_score = tab2.discard_score(Some(3));

        assert!(
            pinned_score < unpinned_score,
            "Pinned tab should be more protected: pinned={} unpinned={}",
            pinned_score,
            unpinned_score
        );
    }

    #[test]
    fn test_audible_tab_protected() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.is_audible = true;

        let audible_score = tab.discard_score(Some(2));

        let dom2 = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab2 = Tab::new(
            2,
            "https://example.com".to_string(),
            dom2,
            "Test".to_string(),
        );
        tab2.last_active_timestamp -= 600;

        let silent_score = tab2.discard_score(Some(3));

        assert!(
            audible_score < silent_score,
            "Audible tab should be more protected: audible={} silent={}",
            audible_score,
            silent_score
        );
    }

    #[test]
    fn test_sleeping_tab_preferred_for_discard() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.last_active_timestamp -= 600; // 10 minutes inactive

        let awake_score = tab.discard_score(Some(2));

        tab.sleep();
        let sleeping_score = tab.discard_score(Some(2));

        assert!(
            sleeping_score > awake_score,
            "Sleeping tab should have higher discard score: sleeping={} awake={}",
            sleeping_score,
            awake_score
        );
    }

    #[test]
    fn test_discard_drops_dom() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        assert!(tab.dom.find_tag("h1").is_some());
        assert!(!tab.is_discarded);

        let freed = tab.discard();
        assert!(freed > 0);
        assert!(tab.is_discarded);
        assert!(!tab.is_sleeping);
        assert!(tab.dom.find_tag("h1").is_none());
        assert_eq!(tab.dom.tag, "root");
    }

    #[test]
    fn test_undiscard_restores_url() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        tab.discard();
        assert!(tab.is_discarded);

        let url = tab.undiscard();
        assert_eq!(url, Some("https://example.com".to_string()));
        assert!(!tab.is_discarded);
    }

    #[test]
    fn test_undiscard_noop_when_not_discarded() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let result = tab.undiscard();
        assert!(result.is_none());
    }

    #[test]
    fn test_discard_idempotent() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let freed1 = tab.discard();
        assert!(freed1 > 0);

        let freed2 = tab.discard();
        assert_eq!(freed2, 0, "Second discard should free nothing");
    }

    #[test]
    fn test_scores_sorted_correctly() {
        // Create multiple tabs with different characteristics
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");

        let mut tab1 = Tab::new(1, "https://a.com".to_string(), dom.clone(), "A".to_string());
        tab1.last_active_timestamp -= 1200; // 20 min inactive (highest score)

        let mut tab2 = Tab::new(2, "https://b.com".to_string(), dom.clone(), "B".to_string());
        tab2.is_pinned = true; // protected

        let mut tab3 = Tab::new(3, "https://c.com".to_string(), dom.clone(), "C".to_string());
        tab3.is_audible = true; // protected

        let score1 = tab1.discard_score(Some(4)); // inactive, unpinned
        let score2 = tab2.discard_score(Some(4)); // pinned
        let score3 = tab3.discard_score(Some(4)); // audible

        assert!(
            score1 > score2,
            "Inactive unpinned should score higher than pinned"
        );
        assert!(
            score1 > score3,
            "Inactive unpinned should score higher than audible"
        );
    }

    // ===== v1.2.0: DOM Compression Tests =====

    #[test]
    fn test_sleep_compresses_dom() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content here</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        assert!(tab.compressed_dom.is_none());

        tab.sleep();

        // After sleep, compressed DOM should be populated
        assert!(tab.compressed_dom.is_some(), "Sleep should compress DOM");
        assert!(tab.is_sleeping);
    }

    #[test]
    fn test_wake_restores_from_compressed_dom() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content here</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        tab.sleep();
        assert!(tab.is_sleeping);
        assert!(tab.compressed_dom.is_some());

        // Wake should restore DOM from compressed data
        let result = tab.wake();
        assert!(
            matches!(result, WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );
        assert!(!tab.is_sleeping);

        // DOM should be restored (not empty root)
        assert_ne!(tab.dom.tag, "root");
        assert!(tab.dom.find_tag("h1").is_some());
        assert!(
            tab.compressed_dom.is_none(),
            "Compressed data should be cleared after wake"
        );
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        let dom = crate::parser::parse_html(
            "<html><body><div class=\"main\"><p>Hello</p><p>World</p></div></body></html>",
        );
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        // Compress
        let compressed = tab.compress_dom();
        assert!(compressed.is_some());
        let data = compressed.unwrap();
        assert!(!data.is_empty());

        // Decompress
        let restored = Tab::decompress_dom(&data);
        assert!(restored.is_ok());

        let restored_dom = restored.unwrap();
        assert_eq!(restored_dom.tag, tab.dom.tag);
        assert_eq!(restored_dom.children.len(), tab.dom.children.len());
    }

    #[test]
    fn test_compress_empty_dom_returns_none() {
        let dom = Element::new("root");
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let compressed = tab.compress_dom();
        assert!(
            compressed.is_none(),
            "Empty root DOM should not be compressed"
        );
    }

    #[test]
    fn test_sleep_wake_with_complex_dom() {
        let html = r#"
        <html>
            <head><title>Test Page</title></head>
            <body>
                <header><h1>Welcome</h1></header>
                <main>
                    <p>Paragraph 1</p>
                    <p>Paragraph 2</p>
                    <ul>
                        <li>Item 1</li>
                        <li>Item 2</li>
                        <li>Item 3</li>
                    </ul>
                </main>
                <footer>Footer content</footer>
            </body>
        </html>
        "#;
        let dom = crate::parser::parse_html(html);
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        // Count nodes before sleep
        let nodes_before = crate::count_elements(&tab.dom);
        assert!(nodes_before > 5);

        tab.sleep();
        assert!(tab.is_sleeping);
        assert!(tab.compressed_dom.is_some());

        let wake_result = tab.wake();
        assert!(!tab.is_sleeping);
        assert!(
            matches!(wake_result, WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );

        // DOM should be restored with same structure
        let nodes_after = crate::count_elements(&tab.dom);
        assert_eq!(
            nodes_before, nodes_after,
            "DOM structure should be preserved"
        );
    }

    #[test]
    fn test_history_get_dom_roundtrip_gzip() {
        // Build a DOM large enough that gzip wins and the snapshot is stored
        // as gzip-compressed JSON (the path that used to return None).
        let html = format!(
            "<html><body>{}</body></html>",
            (0..500)
                .map(|i| format!(
                    "<div class=\"item-{}\" data-idx=\"{}\">Repeated text block number {} with padding padding padding</div>",
                    i, i, i
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let dom = crate::parser::parse_html(&html);
        let entry = HistoryEntry::new("https://example.com".to_string(), "T".to_string(), &dom);

        let restored = entry.get_dom().expect("get_dom must decompress gzip data");
        assert_eq!(restored.tag, dom.tag);
        assert_eq!(restored.children.len(), dom.children.len());
    }

    #[test]
    fn test_history_get_dom_roundtrip_plain_json() {
        // Tiny DOM: compression doesn't help, so the snapshot is raw JSON —
        // the legacy fallback format get_dom must also accept.
        let html = "<html><body><h1>Hi</h1></body></html>";
        let dom = crate::parser::parse_html(html);
        let entry = HistoryEntry::new("https://example.com".to_string(), "T".to_string(), &dom);

        let restored = entry.get_dom().expect("plain JSON snapshot must parse");
        assert!(restored.find_tag("h1").is_some());
    }

    #[test]
    fn test_go_back_restores_real_dom_after_gzip() {
        // End-to-end: push a gzip-compressed history entry, go back, and the
        // DOM must be the real page, not an empty root.
        let html = format!(
            "<html><body>{}</body></html>",
            (0..400)
                .map(|i| format!("<p>Item number {} with enough text content</p>", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let dom = crate::parser::parse_html(&html);
        let mut tab = Tab::new(1, "https://a.com".to_string(), dom, "A".to_string());
        let page_b = crate::parser::parse_html("<html><body><h1>Page B</h1></body></html>");
        tab.push_history(HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &page_b,
        ));

        assert!(tab.go_back());
        assert_eq!(tab.url, "https://a.com");
        assert!(
            tab.dom.find_tag("p").is_some() || tab.dom.children.len() > 1,
            "Back must restore the real DOM, got tag={} children={}",
            tab.dom.tag,
            tab.dom.children.len()
        );
    }

    #[test]
    fn test_flatten_deep_caps_depth() {
        // Build a 10k-deep chain without the parser (simulating a legacy
        // snapshot), then verify flatten_deep caps it iteratively.
        let mut deep = Element::new("div");
        for _ in 0..10_000 {
            let mut child = Element::new("div");
            child.text = "x".to_string();
            let old = std::mem::replace(&mut deep, Element::new("div"));
            child.add_child(old);
            deep = child;
        }
        let flattened = flatten_deep(deep, crate::parser::MAX_DOM_DEPTH);
        // Depth must be capped: walk iteratively with an explicit stack.
        let mut stack: Vec<(usize, &Element)> = vec![(0, &flattened)];
        let mut max_depth = 0usize;
        while let Some((d, el)) = stack.pop() {
            max_depth = max_depth.max(d);
            for c in &el.children {
                stack.push((d + 1, c));
            }
        }
        assert!(
            max_depth <= crate::parser::MAX_DOM_DEPTH,
            "flatten_deep left depth {} (cap {})",
            max_depth,
            crate::parser::MAX_DOM_DEPTH
        );
    }
}

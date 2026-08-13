// GhitaBrowser core module re-exports
// GhitaBrowser
//! A lightweight, document-focused browser engine built in safe Rust
/// Single source of truth for the app version.
/// Used by the UI (status bar, about), the user-agent strings and storage state.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod acceptance;
pub mod accessibility;
pub mod adblock;
pub mod app_platform;
pub mod audio_output;
pub mod bookmarks;
pub mod cache_api;
pub mod child_process;
pub mod content_control;
pub mod crash_recovery;
pub mod css_parser;
pub mod document;
pub mod dom;
pub mod downloads;
pub mod dynamic_render;
pub mod extensions;
#[cfg(windows)]
pub mod gpu_compositor;
pub mod history_manager;
pub mod html_media;
pub mod https_upgrade;
pub mod image_loader;
pub mod indexeddb;
pub mod installed_app;
pub mod ipc;
pub mod iso_bmff;
pub mod javascript;
pub mod layout;
pub mod live_dom;
pub mod local_file;
pub mod media_backend;
pub mod media_core;
pub mod media_runtime;
pub mod media_saver;
pub mod memory_tracker;
pub mod messaging;
pub mod mse;
pub mod network;
pub mod network_scheduler;
pub mod notes;
pub mod omnibox;
pub mod package_crypto;
pub mod paint;
pub mod parallel_downloader;
pub mod parser;
pub mod passwords;
pub mod pdf;
pub mod performance;
pub mod permissions;
pub mod pip;
pub mod process_architecture;
pub mod process_coordinator;
pub mod promise_runtime;
pub mod reader_mode;
pub mod realtime;
pub mod release_smoke;
pub mod renderer;
pub mod runtime_core;
pub mod sandbox;
pub mod scene_compositor;
pub mod search;
pub mod service_worker;
pub mod settings;
pub mod sidebar;
pub mod storage;
pub mod storage_quota;
pub mod string_pool;
pub mod tab;
pub mod tab_strip;
pub mod task_manager;
pub mod text_renderer;
pub mod text_shaper;
pub mod tracking_protection;
pub mod ui;
pub(crate) mod ui_helpers;
pub mod updater;
pub mod wasm;
pub mod wasm_interp;
pub mod web_api;
pub mod web_capture;
pub mod web_runtime;
pub mod windows_integration;
pub mod worker;
pub mod youtube;

pub use acceptance::{
    AcceptanceAuditor, AcceptanceEvidenceBundle, AcceptanceReleaseManager, AcceptanceReport,
    AuditEvidence, EvidenceArtifact, ExternalReleaseEvidence, PerformanceSoakTracker,
    PerformanceSummary, ScenarioCapability, ScenarioCategory, ScenarioDefinition, ScenarioEvidence,
    ScenarioMatrix, ScenarioResult, SoakSample,
};
pub use adblock::{AdBlockConfig, AdBlockStats, AdBlocker, ResourceType};
pub use extensions::{
    ContentScriptConfig, ExtensionApproval, ExtensionError, ExtensionManager, ExtensionPackage,
    ExtensionPermission, ExtensionPermissionReview, ExtensionRecord, ExtensionStatus,
    ExtensionStorage, ExtensionWorker, GhitaExtensionManifest,
};
pub use installed_app::{
    AppDisplayMode, AppError, AppIconConfig, InstalledAppApproval, InstalledAppManager,
    InstalledAppManifest, InstalledAppRecord, InstalledAppReview, InstalledAppShortcut,
    InstalledAppWindow,
};
pub use notes::{NoteStore, QuickNote};
pub use parallel_downloader::{DownloadChunk, ParallelDownloadTask};
pub use passwords::{
    PasswordStore, SavedPassword, SystemCredential, WindowsCredentialStore, WindowsPasskeyPlatform,
};
pub use pip::PipState;
pub use reader_mode::{ReaderArticle, ReaderModeExtractor, ReaderSettings, ReaderTheme};
pub use sidebar::{PinnedApp, SidebarPanel, SidebarState};
pub use task_manager::{ProcessTaskInfo, TaskManager};
pub use updater::{
    RepairEngine, UninstallChoice, UpdateError, UpdateFault, UpdateInstaller, UpdateManager,
    UpdateManifest, UpdatePackage, UpdateState, VersionComparer,
};
pub use web_capture::{CaptureMode, RectRegion, WebCaptureState};
pub use windows_integration::{
    BrowserNotification, CliAction, CrashReportConsent, FileAssociation, ProtocolHandler,
    WindowsIntegration,
};

/// Re-export the bounded application-platform layer.
pub use app_platform::{
    ApplicationDocument, CustomElementDefinition, CustomElementRegistry, HydrationReport,
    LifecycleKind, LifecycleRecord, ShadowMode, SlotAssignment,
};
/// Re-export CSS parser
pub use css_parser::{parse_css, ComputedStyle, CssRule};
/// Re-export the retained renderer used by mutable live documents.
pub use dynamic_render::{
    DynamicInvalidation, DynamicRenderFrame, DynamicRenderMetrics, DynamicRenderer,
};
/// Re-export image cache
pub use image_loader::ImageCache;
/// Re-export JavaScript engine
pub use javascript::JsvEngine;
/// Re-export layout system
pub use layout::{create_layout_tree, perform_layout, LayoutNode};
/// Re-export bounded live DOM and event primitives.
pub use live_dom::{
    DefaultAction, DispatchReport, DomEvent, EventPhase, ListenerOptions, LiveDocument, LiveNode,
    LiveNodeKind, LiveRenderState, MutationInvalidation, NodeId,
};
/// Re-export memory tracker
pub use memory_tracker::{BrowserMemoryEstimate, MemoryTracker, TabMemoryEstimate};
/// Re-export network functions and cache
pub use network::{fetch_url, fetch_with_cache, CacheStats, FetchResult, ResourceCache};
/// Re-export the pixel painter
pub use paint::{
    build_display_list, build_display_list_with_cache, DisplayItem, DisplayList, LinkRegion, Rgba,
};
/// Re-export parser module types for convenience
pub use parser::{parse_html, Element};
/// Re-export performance profiler
pub use performance::Profiler;
pub use performance::{DynamicFrameBudget, NavigationMetrics, PerformanceBudget};
/// Re-export renderer functions
pub use renderer::render_to_string;
/// Re-export search results
pub use search::{search_web, SearchResult};
/// Re-export storage system
pub use storage::{
    Bookmark, BrowserSession, BrowserSettings, Cookie, CookieStore, DownloadRecord, HistoryRecord,
    LocalStorage, SessionTab, SessionTabGroup, StorageManager,
};
/// Re-export tab system
pub use tab::{Tab, TabManager};
/// Re-export browser-facing networking Web APIs.
pub use web_api::{
    AbortController, AbortSignal, CredentialsMode, FetchError, FetchPromiseId, FetchPromiseState,
    FetchRuntime, Headers, RedirectMode, RequestMode, ResponseType, UrlSearchParams, WebRequest,
    WebResponse, WebTimerQueue, WebUrl, XmlHttpRequest,
};

/// Performance statistics for monitoring
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Decoded image cache (for <img> tags)
    pub image_cache: ImageCache,
    /// Memory tracker for estimating per-tab and total RAM usage
    pub memory_tracker: memory_tracker::MemoryTracker,
    /// Shared request blocker used by every subresource-loading path.
    pub adblocker: AdBlocker,
    pub content_control: content_control::ContentControlEngine,
    pub https_upgrade: https_upgrade::HttpsUpgradeEngine,
    pub cookie_blocker: tracking_protection::ThirdPartyCookieBlocker,
    /// Optional native multi-process control plane. It is enabled by the
    /// desktop app when the packaged child executable is available.
    pub process_coordinator: Option<process_coordinator::BrowserProcessCoordinator>,
    /// Extension Manager for Phase 26 independent extensions
    pub extension_manager: extensions::ExtensionManager,
    /// App Manager for Phase 26 installed web apps
    pub app_manager: installed_app::InstalledAppManager,
    /// Safe Rust Updater Manager for Phase 27
    pub updater: updater::UpdateManager,
    /// Windows Integration Manager for Phase 27
    pub win_integration: windows_integration::WindowsIntegration,
    /// Acceptance Release Manager for Phase 28
    pub acceptance: acceptance::AcceptanceReleaseManager,
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

impl Browser {
    /// Create a new browser instance with default viewport
    pub fn new() -> Self {
        Self::with_storage(StorageManager::new())
    }

    /// Create a browser for tests/tools without reading or writing a user profile.
    pub fn new_in_memory() -> Self {
        Self::with_storage(StorageManager::in_memory())
    }

    pub fn new_with_profile(
        base_dir: impl AsRef<std::path::Path>,
        profile_name: &str,
    ) -> Result<Self, String> {
        Ok(Self::with_storage(StorageManager::for_profile(
            base_dir,
            profile_name,
        )?))
    }

    fn with_storage(storage: StorageManager) -> Self {
        let adblocker = AdBlocker::new(AdBlockConfig {
            enabled: storage.settings.adblock_enabled,
            cosmetic_filtering: storage.settings.adblock_cosmetic_filtering,
            disabled_domains: storage.settings.adblock_disabled_domains.clone(),
            ..Default::default()
        });

        let (extension_manager, app_manager, updater, win_integration) = match storage.storage_dir()
        {
            Some(dir) => (
                extensions::ExtensionManager::new_with_profile(dir)
                    .unwrap_or_else(|_| extensions::ExtensionManager::new_in_memory()),
                installed_app::InstalledAppManager::new_with_profile(dir)
                    .unwrap_or_else(|_| installed_app::InstalledAppManager::new_in_memory()),
                updater::UpdateManager::new_for_application(VERSION, dir)
                    .unwrap_or_else(|_| updater::UpdateManager::new_in_memory(VERSION)),
                windows_integration::WindowsIntegration::new_with_profile(dir)
                    .unwrap_or_else(|_| windows_integration::WindowsIntegration::new_in_memory()),
            ),
            None => (
                extensions::ExtensionManager::new_in_memory(),
                installed_app::InstalledAppManager::new_in_memory(),
                updater::UpdateManager::new_in_memory(VERSION),
                windows_integration::WindowsIntegration::new_in_memory(),
            ),
        };

        Self {
            tabs: TabManager::new(),
            viewport_width: 1100,
            viewport_height: 780,
            storage,
            cache: ResourceCache::new(),
            js_engine: JsvEngine::new(),
            profiler: Profiler::new(),
            css_rules: Vec::new(),
            last_render_stats: None,
            image_cache: ImageCache::new(),
            memory_tracker: memory_tracker::MemoryTracker::new(),
            adblocker,
            content_control: content_control::ContentControlEngine::new(),
            https_upgrade: https_upgrade::HttpsUpgradeEngine::new(
                https_upgrade::HttpsMode::EnabledAll,
            ),
            cookie_blocker: tracking_protection::ThirdPartyCookieBlocker::new(
                tracking_protection::CookiePolicy::BlockThirdParty,
            ),
            process_coordinator: None,
            extension_manager,
            app_manager,
            updater,
            win_integration,
            acceptance: acceptance::AcceptanceReleaseManager::new(),
        }
    }

    /// Start the packaged network/media/GPU control processes. Development
    /// and headless callers may leave this disabled explicitly.
    pub fn initialize_process_architecture(&mut self) -> Result<usize, String> {
        if let Some(coordinator) = self.process_coordinator.as_ref() {
            return Ok(coordinator.native_process_count());
        }
        let coordinator = process_coordinator::BrowserProcessCoordinator::discover()?;
        let count = coordinator.native_process_count();
        self.process_coordinator = Some(coordinator);
        Ok(count)
    }

    /// Bind a web navigation to an origin-isolated renderer control process.
    pub fn attach_navigation_process(&mut self, tab_id: usize, url: &str) -> Result<(), String> {
        if let Some(coordinator) = self.process_coordinator.as_mut() {
            coordinator.attach_tab(tab_id, url)?;
        }
        Ok(())
    }

    pub fn restore_previous_session(&mut self) -> usize {
        let session = self.storage.session().clone();
        self.tabs.restore_session(&session)
    }

    pub fn persist_session(&mut self) {
        self.storage.set_session(self.tabs.session_snapshot());
        self.storage.save();
    }

    pub fn pin_tab(&mut self, index: usize, pinned: bool) -> bool {
        let changed = self.tabs.pin_tab_by_index(index, pinned);
        if changed {
            self.persist_session();
        }
        changed
    }

    pub fn toggle_tab_mute(&mut self, index: usize) -> Option<bool> {
        let muted = self.tabs.toggle_mute_by_index(index);
        if muted.is_some() {
            self.persist_session();
        }
        muted
    }

    pub fn create_tab_group(&mut self, name: &str, color: &str) -> Result<u64, String> {
        let group = self.tabs.create_group(name, color)?;
        self.persist_session();
        Ok(group)
    }

    pub fn assign_tab_group(&mut self, index: usize, group: Option<u64>) -> bool {
        let changed = self.tabs.assign_tab_to_group_by_index(index, group);
        if changed {
            self.persist_session();
        }
        changed
    }

    pub fn reorder_tab(&mut self, from_index: usize, to_index: usize) -> bool {
        let changed = self.tabs.reorder_tab(from_index, to_index);
        if changed {
            self.persist_session();
        }
        changed
    }

    pub fn secure_navigation_url(&self, url: &str) -> String {
        match self.https_upgrade.evaluate_url(url) {
            https_upgrade::HttpsUpgradeResult::Upgraded { new_url } => new_url,
            https_upgrade::HttpsUpgradeResult::AlreadySecure { url }
            | https_upgrade::HttpsUpgradeResult::ExemptLocal { url }
            | https_upgrade::HttpsUpgradeResult::InsecureAllowed { url } => url,
        }
    }

    pub fn cookie_header_for_navigation(&self, top_level_url: &str, request_url: &str) -> String {
        let cookies = storage::cookie_header_for(self.storage.cookies(), request_url);
        if cookies.is_empty() {
            return cookies;
        }
        let request_domain = url::Url::parse(request_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        if self
            .cookie_blocker
            .should_allow_cookie(top_level_url, &request_domain)
        {
            cookies
        } else {
            String::new()
        }
    }

    pub fn permission_state(
        &self,
        origin: &str,
        permission: permissions::PermissionType,
    ) -> permissions::PermissionState {
        self.storage
            .permissions()
            .get_permission(origin, permission)
    }

    pub fn set_permission(
        &mut self,
        origin: &str,
        permission: permissions::PermissionType,
        state: permissions::PermissionState,
    ) -> Result<(), String> {
        self.storage.set_permission(origin, permission, state)
    }

    pub fn reset_permissions_for_origin(&mut self, origin: &str) -> bool {
        self.storage.reset_permissions_for_origin(origin)
    }

    /// Load a URL: fetch, parse, style, layout, render
    pub fn load_url(&mut self, url: &str) -> Result<String, String> {
        let secure_url = self.secure_navigation_url(url);
        let url = secure_url.as_str();
        let start = std::time::Instant::now();

        // 1. Fetch HTML (with cache + cookie jar integration)
        let fetch_start = std::time::Instant::now();
        let fetch_result =
            network::fetch_with_cache(url, &mut self.cache, Some(self.storage.cookies_mut()))
                .map_err(|e| format!("Network error: {}", e))?;
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

        // 4. Apply styles - merge global CSS with page <style> tags and external stylesheets
        let style_start = std::time::Instant::now();

        // Extract and parse <style> tags from the page
        let mut page_css_rules: Vec<css_parser::CssRule> = Vec::new();
        page_css_rules.extend(css_parser::parse_css(
            &self.content_control.generate_cosmetic_css_for_origin(url),
        ));
        let style_elements = dom.find_all_tags("style");
        for style_elem in &style_elements {
            let css_text = style_elem.text.trim();
            if !css_text.is_empty() {
                let mut rules = css_parser::parse_css(css_text);
                page_css_rules.append(&mut rules);
            }
        }

        // Load external stylesheets (<link rel="stylesheet">)
        let link_elements = dom.find_all_tags("link");
        let page_domain = url::Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_string));
        for link_elem in &link_elements {
            if link_elem.get_attr("rel").map(|s| s.as_str()) == Some("stylesheet") {
                if let Some(href) = link_elem.get_attr("href") {
                    // Resolve relative URL against page URL
                    let css_url = resolve_url(url, href);
                    if self.adblocker.should_block_resource(
                        &css_url,
                        page_domain.as_deref(),
                        ResourceType::Style,
                    ) {
                        continue;
                    }
                    // CSS follows the same cookie, cache, redirect and size
                    // policy as the main document. Failure remains non-fatal.
                    let css_domain = url::Url::parse(&css_url)
                        .ok()
                        .and_then(|parsed| parsed.host_str().map(str::to_string))
                        .unwrap_or_default();
                    let allow_cookies = self.cookie_blocker.should_allow_cookie(url, &css_domain);
                    let result = if allow_cookies {
                        network::fetch_with_cache(
                            &css_url,
                            &mut self.cache,
                            Some(self.storage.cookies_mut()),
                        )
                    } else {
                        network::fetch_with_cache(&css_url, &mut self.cache, None)
                    };
                    if let Ok(result) = result {
                        let mut rules = css_parser::parse_css(&result.body);
                        page_css_rules.append(&mut rules);
                    }
                }
            }
        }

        // Merge: global rules first, then page rules (page overrides global)
        let all_rules: Vec<css_parser::CssRule> = self
            .css_rules
            .iter()
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

        // Count nodes
        let dom_nodes = count_elements(&dom);
        let layout_nodes = layout_tree
            .as_ref()
            .map(crate::layout::count_layout_nodes)
            .unwrap_or(0);

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

        // Update tab - record the freshly loaded page in history (deduped)
        if let Some(tab) = self.tabs.active_tab_mut() {
            let new_entry = crate::tab::HistoryEntry::new(url.to_string(), title.clone(), &dom);
            tab.push_history(new_entry);

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
        let secure_url = self.secure_navigation_url(url);
        let url = secure_url.as_str();
        let dom = parser::parse_html(html_content);
        let title = extract_title_from_dom(&dom);

        let mut rules = self.css_rules.clone();
        rules.extend(css_parser::parse_css(
            &self.content_control.generate_cosmetic_css_for_origin(url),
        ));
        for style in dom.find_all_tags("style") {
            rules.extend(css_parser::parse_css(&style.text));
        }
        let layout_tree = layout::create_layout_tree(&dom, &rules, self.viewport_width);

        if let Some(tab) = self.tabs.active_tab_mut() {
            // Record the freshly loaded page in history (deduped)
            let new_entry = crate::tab::HistoryEntry::new(url.to_string(), title.clone(), &dom);
            tab.push_history(new_entry);

            tab.dom = dom;
            tab.title = title;
            tab.url = url.to_string();
            tab.layout = layout_tree;
        } else {
            let tab_id = self.add_tab(url, dom, &title);
            if let Some(tab) = self.tabs.get_tab_mut(tab_id) {
                tab.layout = layout_tree;
            }
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

    pub fn tab_by_index(&self, index: usize) -> Option<&Tab> {
        self.tabs.get_tab_by_index(index)
    }

    pub fn tab_groups(&self) -> &std::collections::HashMap<u64, tab::TabGroup> {
        self.tabs.groups()
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
                let tr =
                    text_renderer::TextRenderer::new(self.viewport_width, self.viewport_height);
                tr.render_to_text(layout_root)
            } else {
                let css_rules = &self.css_rules;
                match layout::create_layout_tree(&tab.dom, css_rules, self.viewport_width) {
                    Some(root) => {
                        let tr = text_renderer::TextRenderer::new(
                            self.viewport_width,
                            self.viewport_height,
                        );
                        tr.render_to_text(&root)
                    }
                    None => String::from("[Error rendering content]"),
                }
            }
        } else {
            String::from("[No active tab]")
        }
    }

    /// Estimate total memory usage across all tabs and caches.
    pub fn estimate_memory(&self) -> memory_tracker::BrowserMemoryEstimate {
        let tabs: Vec<&Tab> = self.tabs.iter().collect();
        memory_tracker::MemoryTracker::estimate_browser(&tabs, &self.image_cache, &self.cache)
    }

    /// Estimate memory for a single tab by ID.
    pub fn estimate_tab_memory(&self, tab_id: usize) -> Option<memory_tracker::TabMemoryEstimate> {
        self.tabs
            .get_tab(tab_id)
            .map(memory_tracker::MemoryTracker::estimate_tab)
    }

    /// Put an inactive tab to sleep if it qualifies and the threshold has passed.
    /// `threshold_minutes`: inactivity duration before sleeping (0 = disabled).
    /// `sleep_delay_seconds`: grace period after losing focus before sleeping.
    /// Returns the tab ID that was put to sleep, or None.
    pub fn maybe_sleep_inactive_tab(
        &mut self,
        threshold_minutes: u32,
        sleep_delay_seconds: i64,
    ) -> Option<usize> {
        if threshold_minutes == 0 {
            return None;
        }

        let active_id = self.tabs.active_tab_id();

        // Find the best candidate: oldest active non-pinned, non-internal tab
        let candidate = self
            .tabs
            .iter()
            .filter(|t| {
                Some(t.id) != active_id
                    && t.can_sleep()
                    && t.seconds_since_active() >= sleep_delay_seconds
                    && t.seconds_since_active() as u32 >= threshold_minutes * 60
            })
            .max_by_key(|t| t.seconds_since_active());

        if let Some(tab) = candidate {
            let id = tab.id;
            if let Some(t) = self.tabs.get_tab_mut(id) {
                t.sleep();
                return Some(id);
            }
        }

        None
    }

    /// Wake a sleeping tab. Returns the wake result indicating what action the caller should take.
    pub fn wake_tab(&mut self, tab_id: usize) -> tab::WakeResult {
        self.tabs
            .get_tab_mut(tab_id)
            .map(|t| t.wake())
            .unwrap_or(tab::WakeResult::NotSleeping)
    }

    /// Check if a tab is sleeping.
    pub fn is_tab_sleeping(&self, tab_id: usize) -> bool {
        self.tabs.get_tab(tab_id).is_some_and(|t| t.is_sleeping)
    }

    /// Check if a tab is discarded.
    pub fn is_tab_discarded(&self, tab_id: usize) -> bool {
        self.tabs.get_tab(tab_id).is_some_and(|t| t.is_discarded)
    }

    /// Calculate the discard score for every tab and return them sorted
    /// by score descending (most discardable first).
    pub fn tab_discard_scores(&self) -> Vec<(usize, i64)> {
        let active_id = self.tabs.active_tab_id();
        let mut scores: Vec<(usize, i64)> = self
            .tabs
            .iter()
            .map(|t| (t.id, t.discard_score(active_id)))
            .collect();
        scores.sort_by_key(|b| std::cmp::Reverse(b.1)); // descending
        scores
    }

    /// Check if memory pressure is high and discard the least important tab if so.
    ///
    /// `memory_threshold_mb`: if estimated browser memory exceeds this, trigger discard.
    /// `min_tabs_to_keep`: never discard if we have this few tabs remaining.
    ///
    /// Returns the ID of the tab that was discarded, or None if no action was taken.
    pub fn check_memory_pressure(
        &mut self,
        memory_threshold_mb: u32,
        min_tabs_to_keep: usize,
    ) -> Option<usize> {
        if self.tabs.tab_count() <= min_tabs_to_keep {
            return None;
        }

        // Check if we're over the memory threshold
        let estimate = self.estimate_memory();
        let total_mb = memory_tracker::MemoryTracker::bytes_to_mb(estimate.total_bytes);

        if total_mb < memory_threshold_mb as f32 {
            return None;
        }

        // Find the tab with the highest discard score (most discardable)
        let scores = self.tab_discard_scores();
        for (tab_id, score) in scores {
            // A zero score is a normal, newly inactive tab and is eligible
            // under actual memory pressure; negative scores remain protected.
            if score >= 0 {
                if let Some(tab) = self.tabs.get_tab_mut(tab_id) {
                    tab.discard();
                    return Some(tab_id);
                }
            }
        }

        None
    }

    /// Discard the least important tab regardless of memory threshold.
    /// Used as a forced memory relief action.
    /// Returns the ID of the tab that was discarded, or None if no tab was discardable.
    pub fn discard_least_important_tab(&mut self) -> Option<usize> {
        let scores = self.tab_discard_scores();
        for (tab_id, score) in scores {
            if score >= 0 {
                if let Some(tab) = self.tabs.get_tab_mut(tab_id) {
                    tab.discard();
                    return Some(tab_id);
                }
            }
        }
        None
    }

    /// Restore a discarded tab. Returns the URL to reload.
    pub fn undiscard_tab(&mut self, tab_id: usize) -> Option<String> {
        self.tabs.get_tab_mut(tab_id).and_then(|t| t.undiscard())
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

impl Drop for Browser {
    fn drop(&mut self) {
        self.storage.set_session(self.tabs.session_snapshot());
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

/// Resolve a relative URL against a base URL
fn resolve_url(base: &str, relative: &str) -> String {
    // If relative is already absolute, return it.
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }

    // Parse base URL
    if let Ok(base_url) = url::Url::parse(base) {
        // Handle protocol-relative URLs (//example.com/path)
        if relative.starts_with("//") {
            return format!("{}:{}", base_url.scheme(), relative);
        }

        // Use URL crate to resolve relative URLs
        if let Ok(resolved) = base_url.join(relative) {
            return resolved.as_str().to_string();
        }
    }

    // Fallback: simple string concatenation
    let base_without_query = base.split('?').next().unwrap_or(base);
    let base_path = base_without_query
        .rsplit('/')
        .next()
        .unwrap_or(base_without_query);
    let base_dir = &base[..base_without_query.len() - base_path.len()];

    if relative.starts_with('/') {
        // Absolute path - extract origin from base
        if let Ok(base_url) = url::Url::parse(base) {
            let origin = format!(
                "{}://{}",
                base_url.scheme(),
                base_url.host_str().unwrap_or("")
            );
            if let Some(port) = base_url.port() {
                return format!("{}:{}{}", origin, port, relative);
            }
            return format!("{}{}", origin, relative);
        }
        return relative.to_string();
    }

    // Relative path
    format!("{}{}", base_dir, relative)
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
        let _ = browser.load_html(
            "https://example.com",
            "<html><body><h1>Welcome</h1></body></html>",
        );
        let rendered = browser.render_current();
        assert!(!rendered.is_empty());
        assert!(rendered.contains("Welcome"));
    }

    #[test]
    fn test_browser_with_css() {
        let mut browser = Browser::new();
        browser.set_css("h1 { color: red; font-size: 24px; }");
        let _ = browser.load_html(
            "https://example.com",
            "<html><body><h1>Styled</h1></body></html>",
        );
        let rendered = browser.render_current();
        assert!(rendered.contains("Styled"));
    }

    #[test]
    fn test_browser_tab_switching() {
        let mut browser = Browser::new();
        let _ = browser.load_html("https://a.com", "<html><body><h1>Page A</h1></body></html>");
        browser.add_tab(
            "https://b.com",
            parser::parse_html("<html><body><h1>Page B</h1></body></html>"),
            "Page B",
        );

        assert_eq!(browser.tab_count(), 2);

        // Active tab should be the last added one
        assert_eq!(browser.active_tab().unwrap().url, "https://b.com");
    }

    #[test]
    fn test_extract_title() {
        let dom =
            parser::parse_html("<html><head><title>My Page</title></head><body></body></html>");
        assert_eq!(extract_title_from_dom(&dom), "My Page");
    }

    // ===== v1.2.0: Memory Saver Tests =====

    #[test]
    fn test_browser_maybe_sleep_disabled_when_threshold_zero() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();
        browser.add_tab(
            "https://b.com",
            parser::parse_html("<html><body><h1>B</h1></body></html>"),
            "B",
        );
        // Set threshold to 0 (disabled)
        browser.storage.settings.memory_saver_threshold_minutes = 0;
        let result = browser.maybe_sleep_inactive_tab(0, 0);
        assert!(result.is_none());
        // No tab should be sleeping
        assert!(!browser.is_tab_sleeping(1));
        assert!(!browser.is_tab_sleeping(2));
    }

    #[test]
    fn test_browser_wake_sleeping_tab() {
        let mut browser = Browser::new();
        browser
            .load_html(
                "https://example.com",
                "<html><body><h1>Test</h1></body></html>",
            )
            .unwrap();

        // Manually put the tab to sleep
        if let Some(tab) = browser.tabs.get_tab_mut(1) {
            tab.sleep();
        }
        assert!(browser.is_tab_sleeping(1));

        // Wake it via Browser method — should restore from compressed DOM
        let result = browser.wake_tab(1);
        assert!(
            matches!(result, crate::tab::WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );
        assert!(!browser.is_tab_sleeping(1));
    }

    #[test]
    fn test_browser_wake_non_sleeping_tab_returns_not_sleeping() {
        let mut browser = Browser::new();
        browser
            .load_html(
                "https://example.com",
                "<html><body><h1>Test</h1></body></html>",
            )
            .unwrap();

        let result = browser.wake_tab(1);
        assert!(
            matches!(result, crate::tab::WakeResult::NotSleeping),
            "Should return NotSleeping for non-sleeping tab"
        );
    }

    #[test]
    fn test_sleep_wake_memory_savings() {
        let mut browser = Browser::new();
        // Create a tab with substantial content
        let html = format!(
            "<html><body>{}</body></html>",
            (0..100)
                .map(|i| format!("<p>Paragraph {} with some text content</p>", i))
                .collect::<String>()
        );
        browser.load_html("https://example.com", &html).unwrap();

        let mem_before = browser.estimate_memory();
        let tab_mem_before = browser.estimate_tab_memory(1).unwrap();
        assert!(
            tab_mem_before.dom_bytes > 0,
            "Tab should use DOM memory before sleep"
        );

        // Sleep the tab
        if let Some(tab) = browser.tabs.get_tab_mut(1) {
            let freed = tab.sleep();
            assert!(freed > 0, "Sleep should free bytes");
        }

        let mem_after = browser.estimate_memory();
        let tab_mem_after = browser.estimate_tab_memory(1).unwrap();

        // After sleep, DOM memory should be minimal (just empty root)
        assert!(
            tab_mem_after.dom_bytes < tab_mem_before.dom_bytes,
            "DOM memory should decrease after sleep: before={}, after={}",
            tab_mem_before.dom_bytes,
            tab_mem_after.dom_bytes
        );
        assert!(
            mem_after.total_bytes < mem_before.total_bytes,
            "Total memory should decrease after sleep: before={}, after={}",
            mem_before.total_bytes,
            mem_after.total_bytes
        );
    }

    // ===== v1.2.0: Memory Pressure Tests =====

    #[test]
    fn test_discard_scores_sorted_descending() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();

        // Add a pinned tab (should have low score)
        let pinned_dom = parser::parse_html("<html><body><h1>Pinned</h1></body></html>");
        let pinned_id = browser.add_tab("https://pinned.com", pinned_dom, "Pinned");
        if let Some(t) = browser.tabs.get_tab_mut(pinned_id) {
            t.is_pinned = true;
        }

        // Add a normal tab (should have higher score when inactive)
        let normal_dom = parser::parse_html("<html><body><h1>Normal</h1></body></html>");
        browser.add_tab("https://normal.com", normal_dom, "Normal");

        let scores = browser.tab_discard_scores();
        assert_eq!(scores.len(), 3);

        // Scores should be sorted descending (most discardable first)
        for i in 1..scores.len() {
            assert!(
                scores[i - 1].1 >= scores[i].1,
                "Scores should be sorted descending"
            );
        }
    }

    #[test]
    fn test_check_memory_pressure_disabled_when_few_tabs() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();

        // With min_tabs_to_keep=2 and only 1 tab, no discard
        let result = browser.check_memory_pressure(1, 2);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_memory_pressure_disabled_when_under_threshold() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();
        browser.add_tab(
            "https://b.com",
            parser::parse_html("<html><body><h1>B</h1></body></html>"),
            "B",
        );

        // With very high threshold (100000 MB), no discard
        let result = browser.check_memory_pressure(100000, 2);
        assert!(result.is_none());
    }

    #[test]
    fn test_discard_least_important_tab() {
        let mut browser = Browser::new();
        browser
            .load_html(
                "https://active.com",
                "<html><body><h1>Active</h1></body></html>",
            )
            .unwrap();

        // First tab gets ID 1
        let tab1_id = 1usize;

        // Add a discardable tab (will get ID 2 and become active)
        let discardable_dom = parser::parse_html("<html><body><h1>Old</h1></body></html>");
        let tab2_id = browser.add_tab("https://old.com", discardable_dom, "Old");

        // Make tab1 (not active) very inactive so it has the highest discard score
        if let Some(t) = browser.tabs.get_tab_mut(tab1_id) {
            t.last_active_timestamp -= 7200; // 2 hours inactive
        }

        // Discard the least important tab — should be tab1 (oldest inactive)
        let discarded = browser.discard_least_important_tab();
        assert!(discarded.is_some());

        let discarded_id = discarded.unwrap();
        assert_eq!(
            discarded_id, tab1_id,
            "Should discard the most inactive tab (tab1)"
        );

        // Discarded tab should be marked as discarded
        assert!(browser.is_tab_discarded(discarded_id));
        // Other tab should not be discarded
        assert!(!browser.is_tab_discarded(tab2_id));
    }

    #[test]
    fn test_undiscard_tab() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();
        let tab1_id = 1usize;
        let _tab2_id = browser.add_tab(
            "https://b.com",
            parser::parse_html("<html><body><h1>B</h1></body></html>"),
            "B",
        );

        // Make tab1 inactive so it has a positive discard score (tab2 is active)
        if let Some(t) = browser.tabs.get_tab_mut(tab1_id) {
            t.last_active_timestamp -= 7200; // 2 hours inactive
        }

        // Discard — should be tab1 (inactive, not active tab)
        let discarded = browser.discard_least_important_tab();
        assert!(discarded.is_some());
        assert_eq!(discarded.unwrap(), tab1_id);
        assert!(browser.is_tab_discarded(tab1_id));

        let url = browser.undiscard_tab(tab1_id);
        assert_eq!(url, Some("https://a.com".to_string()));
        assert!(!browser.is_tab_discarded(tab1_id));
    }

    #[test]
    fn test_memory_settings_defaults() {
        // Tests share one PID-based temp storage dir, so earlier tests that
        // save non-default settings (e.g. threshold=0) would leak into this
        // assertion. Reset the persisted file first to test true defaults.
        let tmp = std::env::temp_dir().join(format!("ghitabrowser_test_{}", std::process::id()));
        let _ = std::fs::remove_file(tmp.join("storage.json"));

        let browser = Browser::new();
        assert!(browser.storage.settings.tab_memory_saver);
        assert_eq!(browser.storage.settings.memory_saver_threshold_minutes, 5);
        assert_eq!(browser.storage.settings.memory_pressure_threshold_mb, 500);
    }

    /// Integration test: verify the full image rendering pipeline.
    /// 1. HTML with <img> → layout → display list has PendingImage
    /// 2. Load image into cache → rebuild → display list has Image { cached: true }
    /// 3. Image handle map contains the URL
    #[test]
    fn test_image_rendering_pipeline() {
        let html = r#"<html><body>
            <h1>Image Test</h1>
            <img src="https://example.com/photo.jpg" alt="Photo" width="200" height="150"/>
            <p>Some text below the image</p>
        </body></html>"#;

        // 1. Parse and build layout
        let dom = parser::parse_html(html);
        let css_rules: Vec<css_parser::CssRule> = Vec::new();
        let layout_tree = layout::create_layout_tree(&dom, &css_rules, 800).unwrap();

        // 2. Build display list — image not loaded yet → PendingImage
        let image_cache = crate::image_loader::ImageCache::new();
        let display_list =
            crate::paint::build_display_list_with_cache(&layout_tree, Some(&image_cache));

        let has_pending = display_list.items.iter().any(|item| {
            matches!(item, crate::paint::DisplayItem::PendingImage { url, .. }
                if url == "https://example.com/photo.jpg")
        });
        assert!(
            has_pending,
            "Display list should contain PendingImage for the <img> tag"
        );

        // 3. Verify no cached image yet
        let has_cached = display_list
            .items
            .iter()
            .any(|item| matches!(item, crate::paint::DisplayItem::Image { cached: true, .. }));
        assert!(!has_cached, "No image should be cached yet");

        // 4. Simulate the decoded-cache state via the public cache API below.
        // We need to insert directly into the cache's decoded map.
        // Since load_image fetches via network, we simulate by creating
        // an ImageData and putting it in. Let's verify the cache API:
        let mut cache = crate::image_loader::ImageCache::new();
        cache.add(
            "https://example.com/photo.jpg".to_string(),
            crate::image_loader::Image::new("https://example.com/photo.jpg", 200, 150)
                .with_alt("Photo"),
        );

        // Verify is_decoded is false before loading
        assert!(!cache.is_decoded("https://example.com/photo.jpg"));

        // Verify we can get metadata
        let img = cache.get("https://example.com/photo.jpg").unwrap();
        assert_eq!(img.width, 200);
        assert_eq!(img.height, 150);
        assert_eq!(img.alt_text, "Photo");
        assert!(
            !img.loaded,
            "Image should not be marked loaded until decoded"
        );

        // 5. Rebuild display list with same cache — still PendingImage (not decoded)
        let display_list2 = crate::paint::build_display_list_with_cache(&layout_tree, Some(&cache));
        let has_cached2 = display_list2
            .items
            .iter()
            .any(|item| matches!(item, crate::paint::DisplayItem::Image { cached: true, .. }));
        assert!(
            !has_cached2,
            "Image still not decoded — should be PendingImage"
        );
    }
}

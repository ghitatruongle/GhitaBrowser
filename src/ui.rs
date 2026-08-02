// src/ui.rs - Chrome-style GUI built with Iced, powered by the Rust engine (v0.6.1)


use iced::widget::{
    button, canvas, column, container, horizontal_space, row, scrollable, text, text_input,
    vertical_space,
};
use iced::{
    keyboard, mouse, Application, Color, Command, Element, Length, Settings, Shadow, Theme,
};
use log::info;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::paint::{DisplayItem, DisplayList};
use crate::parser::parse_html;
use crate::search::{search_web, SearchResult};
use crate::Browser;

// ===== Chrome color palettes (dark + light), sampled from Google Chrome =====

#[derive(Clone, Copy)]
struct Pal {
    /// Tab strip background (window frame area)
    tab_strip: Color,
    /// Hovered inactive tab
    tab_hover: Color,
    /// Active tab + toolbar background
    toolbar: Color,
    /// Omnibox pill background
    omnibox: Color,
    /// Omnibox background when focused
    omnibox_focus: Color,
    /// Dropdown menu background
    menu_bg: Color,
    /// Hovered menu item
    menu_hover: Color,
    /// Page content background
    content_bg: Color,
    /// Primary text
    text: Color,
    /// Secondary/dim text
    text_dim: Color,
    /// Chrome accent blue
    accent: Color,
    /// On-accent text
    on_accent: Color,
    /// Destructive red
    danger: Color,
    /// Thin separators
    divider: Color,
    /// HTTPS padlock green
    secure: Color,
}

/// Chrome dark theme (#202124 / #35363A / #8AB4F8)
const DARK_PAL: Pal = Pal {
    tab_strip: Color::from_rgb(0.125, 0.129, 0.141), // #202124
    tab_hover: Color::from_rgb(0.173, 0.180, 0.196), // #2C2E32
    toolbar: Color::from_rgb(0.208, 0.212, 0.227),   // #35363A
    omnibox: Color::from_rgb(0.125, 0.129, 0.141),   // #202124
    omnibox_focus: Color::from_rgb(0.188, 0.192, 0.204), // #303134
    menu_bg: Color::from_rgb(0.161, 0.165, 0.176),   // #292A2D
    menu_hover: Color::from_rgb(0.235, 0.243, 0.263), // #3C3E43
    content_bg: Color::from_rgb(0.125, 0.129, 0.141), // #202124
    text: Color::from_rgb(0.910, 0.918, 0.929),      // #E8EAED
    text_dim: Color::from_rgb(0.604, 0.627, 0.651),  // #9AA0A6
    accent: Color::from_rgb(0.541, 0.706, 0.973),    // #8AB4F8
    on_accent: Color::from_rgb(0.125, 0.129, 0.141),
    danger: Color::from_rgb(0.949, 0.545, 0.510), // #F28B82
    divider: Color::from_rgb(0.235, 0.251, 0.263), // #3C4043
    secure: Color::from_rgb(0.506, 0.788, 0.584), // #81C995
};

/// Chrome light theme (#DEE1E6 / #FFFFFF / #1A73E8)
const LIGHT_PAL: Pal = Pal {
    tab_strip: Color::from_rgb(0.871, 0.882, 0.902), // #DEE1E6
    tab_hover: Color::from_rgb(0.816, 0.827, 0.847), // #D0D3D8
    toolbar: Color::from_rgb(1.0, 1.0, 1.0),         // #FFFFFF
    omnibox: Color::from_rgb(0.945, 0.953, 0.957),   // #F1F3F4
    omnibox_focus: Color::from_rgb(1.0, 1.0, 1.0),   // #FFFFFF
    menu_bg: Color::from_rgb(1.0, 1.0, 1.0),         // #FFFFFF
    menu_hover: Color::from_rgb(0.945, 0.953, 0.957), // #F1F3F4
    content_bg: Color::from_rgb(1.0, 1.0, 1.0),      // #FFFFFF
    text: Color::from_rgb(0.125, 0.129, 0.141),      // #202124
    text_dim: Color::from_rgb(0.373, 0.388, 0.408),  // #5F6368
    accent: Color::from_rgb(0.102, 0.451, 0.910),    // #1A73E8
    on_accent: Color::from_rgb(1.0, 1.0, 1.0),
    danger: Color::from_rgb(0.851, 0.188, 0.145), // #D93025
    divider: Color::from_rgb(0.855, 0.863, 0.878), // #DADCE0
    secure: Color::from_rgb(0.094, 0.502, 0.220), // #188038
};

// Ghita "Fire" brand palette for the New Tab page wordmark —
// a warm orange→red gradient, distinct from Google's rainbow
const GH_ORANGE: Color = Color::from_rgb(1.0, 0.549, 0.259); // #FF8C42
const GH_AMBER: Color = Color::from_rgb(0.969, 0.702, 0.169); // #F7B32B
const GH_RED: Color = Color::from_rgb(0.949, 0.314, 0.133); // #F25022
const GH_CRIMSON: Color = Color::from_rgb(0.839, 0.271, 0.271); // #D64545
const GH_EMBER: Color = Color::from_rgb(0.651, 0.227, 0.169); // #A63A2B

// ===== Custom widget styles =====

/// Flat Chrome-style button: solid color, hover highlight, custom corner radii
struct ChromeButtonStyle {
    bg: Color,
    hover: Color,
    txt: Color,
    radius: [f32; 4],
}

impl button::StyleSheet for ChromeButtonStyle {
    type Style = Theme;

    fn active(&self, _style: &Theme) -> button::Appearance {
        button::Appearance {
            shadow_offset: iced::Vector::default(),
            background: Some(iced::Background::Color(self.bg)),
            text_color: self.txt,
            border: iced::Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: self.radius.into(),
            },
            shadow: Shadow::default(),
        }
    }

    fn hovered(&self, style: &Theme) -> button::Appearance {
        button::Appearance {
            background: Some(iced::Background::Color(self.hover)),
            ..self.active(style)
        }
    }
}

/// Helper to build a Chrome-style button theme
fn chrome_btn(bg: Color, hover: Color, txt: Color, radius: [f32; 4]) -> iced::theme::Button {
    iced::theme::Button::Custom(Box::new(ChromeButtonStyle {
        bg,
        hover,
        txt,
        radius,
    }))
}

/// Rounded-pill omnibox / search field style
struct OmniboxStyle {
    bg: Color,
    focus_bg: Color,
    txt: Color,
    dim: Color,
    accent: Color,
    radius: f32,
}

impl text_input::StyleSheet for OmniboxStyle {
    type Style = Theme;

    fn active(&self, _style: &Theme) -> text_input::Appearance {
        text_input::Appearance {
            background: iced::Background::Color(self.bg),
            border: iced::Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: self.radius.into(),
            },
            icon_color: self.dim,
        }
    }

    fn focused(&self, _style: &Theme) -> text_input::Appearance {
        text_input::Appearance {
            background: iced::Background::Color(self.focus_bg),
            border: iced::Border {
                color: self.accent,
                width: 2.0,
                radius: self.radius.into(),
            },
            icon_color: self.dim,
        }
    }

    fn hovered(&self, style: &Theme) -> text_input::Appearance {
        self.active(style)
    }

    fn disabled(&self, style: &Theme) -> text_input::Appearance {
        self.active(style)
    }

    fn placeholder_color(&self, _style: &Theme) -> Color {
        self.dim
    }

    fn value_color(&self, _style: &Theme) -> Color {
        self.txt
    }

    fn disabled_color(&self, _style: &Theme) -> Color {
        self.dim
    }

    fn selection_color(&self, _style: &Theme) -> Color {
        Color {
            a: 0.35,
            ..self.accent
        }
    }
}

fn omnibox_style(pal: &Pal, radius: f32) -> iced::theme::TextInput {
    iced::theme::TextInput::Custom(Box::new(OmniboxStyle {
        bg: pal.omnibox,
        focus_bg: pal.omnibox_focus,
        txt: pal.text,
        dim: pal.text_dim,
        accent: pal.accent,
        radius,
    }))
}

// ===== Widget IDs for real keyboard focus (Ctrl+L / Ctrl+F) =====

const OMNIBOX_ID: &str = "omnibox";
const FIND_ID: &str = "find-box";
const NTP_SEARCH_ID: &str = "ntp-search";

// Chrome zoom steps (Ctrl +/-)
const ZOOM_STEPS: [u16; 17] = [
    25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500,
];

fn zoom_step_in(current: u16) -> u16 {
    ZOOM_STEPS
        .iter()
        .copied()
        .find(|&z| z > current)
        .unwrap_or(500)
}

fn zoom_step_out(current: u16) -> u16 {
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|&z| z < current)
        .unwrap_or(25)
}

/// Which DevTools pane is visible
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevPane {
    Console,
    Storage,
    Cache,
}

/// Per-tab state for the in-app web search results page (ghita://search)
#[derive(Debug, Clone, Default)]
struct TabSearchState {
    query: String,
    results: Vec<SearchResult>,
    loading: bool,
    error: Option<String>,
}

/// Main application state - connected to the real Browser engine
pub struct GhitaBrowserApp {
    /// The core browser engine
    browser: Browser,

    // Omnibox & content state
    url_input: String,
    ntp_search: String,
    rendered_content: String,
    status_msg: String,
    render_stats_text: String,
    is_loading: bool,
    last_load_time: Option<u64>,

    // Chrome UI state
    show_menu: bool,
    show_suggestions: bool,
    show_bookmarks_bar: bool,
    find_bar_open: bool,
    find_query: String,
    zoom_percent: u16,
    history_query: String,
    homepage_input: String,

    // Web search state (per-tab, ghita://search results page)
    search_state: HashMap<usize, TabSearchState>,

    // Async load coordination: every fetch/search bumps `load_seq`; the
    // response carries the sequence that started it, and is discarded if a
    // newer load was started for the same tab in the meantime.
    load_seq: u64,
    pending_loads: HashMap<usize, u64>,

    // Pixel renderer state
    display_list: Arc<DisplayList>,
    canvas_cache: canvas::Cache,

    // DevTools
    show_devtools: bool,
    dev_pane: DevPane,
    js_console_text: String,
    js_input_text: String,

    // Theme
    is_dark_theme: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Omnibox / navigation
    UrlChanged(String),
    Navigate,
    OpenUrl(String),
    GoBack,
    GoForward,
    Reload,
    Home,
    FocusUrl,

    // New Tab page search box
    NtpSearchChanged(String),
    NtpSearchSubmit,

    // Web search (ghita://search results page)
    SearchResultsLoaded {
        results: Vec<SearchResult>,
        query: String,
        tab_id: usize,
        seq: u64,
    },
    SearchError {
        err: String,
        query: String,
        tab_id: usize,
        seq: u64,
    },

    // Tabs
    SelectTab(usize),
    NewTab,
    NewIncognitoTab,
    CloseTab(usize),
    CloseCurrentTab,
    ReopenClosedTab,
    NextTab,
    PrevTab,
    SelectTabNumber(usize),

    // Three-dot menu & internal pages
    ToggleMenu,
    OpenInternalPage(String),

    // Bookmarks
    ToggleBookmark,
    ToggleBookmarksBar,
    RemoveBookmark(String),

    // Find in page
    ToggleFindBar,
    FindQueryChanged(String),

    // Zoom
    ZoomIn,
    ZoomOut,
    ZoomReset,

    // History page
    HistoryQueryChanged(String),
    RemoveHistoryItem(String),
    ClearHistory,

    // Downloads
    SavePageAs,
    DownloadFinished(Result<crate::storage::DownloadRecord, String>),
    ClearDownloads,

    // Settings page
    SetThemeDark(bool),
    SetSearchEngine(String),
    HomepageChanged(String),
    ClearBrowsingData,
    SetPixelRendering(bool),

    // DevTools
    ToggleDevTools,
    SetDevPane(DevPane),
    JsCodeChanged(String),
    ExecuteJs,

    // Misc
    ToggleTheme,
    EscapePressed,

    // Internal
    PageLoaded {
        result: crate::network::FetchResult,
        tab_id: usize,
        seq: u64,
    },
    LoadError {
        err: String,
        url: String,
        tab_id: usize,
        seq: u64,
    },
}

impl Application for GhitaBrowserApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let browser = Browser::new();

        // Restore user settings (Chrome-style preferences)
        let settings = browser.storage.settings.clone();
        let is_dark_theme = settings.theme != "light";
        let show_bookmarks_bar = settings.show_bookmarks_bar;
        let zoom_percent = if settings.default_zoom == 0 {
            100
        } else {
            settings.default_zoom
        };
        let homepage_input = settings.homepage.clone();

        let mut app = Self {
            browser,
            url_input: String::new(),
            ntp_search: String::new(),
            rendered_content: String::new(),
            status_msg: "Ready".to_string(),
            render_stats_text: String::new(),
            is_loading: false,
            last_load_time: None,
            show_menu: false,
            show_suggestions: false,
            show_bookmarks_bar,
            find_bar_open: false,
            find_query: String::new(),
            zoom_percent,
            history_query: String::new(),
            homepage_input,
            search_state: HashMap::new(),
            load_seq: 0,
            pending_loads: HashMap::new(),
            display_list: Arc::new(DisplayList::default()),
            canvas_cache: canvas::Cache::new(),
            show_devtools: false,
            dev_pane: DevPane::Console,
            js_console_text: String::new(),
            js_input_text: String::new(),
            is_dark_theme,
        };

        // Chrome starts on the New Tab page
        app.open_internal("ghita://newtab", true);

        // Start with keyboard focus in the omnibox so typing works immediately
        (app, Command::perform(async {}, |_| Message::FocusUrl))
    }

    fn title(&self) -> String {
        self.browser
            .active_tab()
            .map(|t| format!("{} - GhitaBrowser", t.title))
            .unwrap_or_else(|| "GhitaBrowser".to_string())
    }

    fn theme(&self) -> Theme {
        if self.is_dark_theme {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::UrlChanged(url) => {
                self.show_suggestions = !url.trim().is_empty();
                self.url_input = url;
            }
            Message::Navigate => {
                let raw = self.url_input.trim().to_string();
                if raw.is_empty() {
                    return Command::none();
                }
                let target = self.resolve_omnibox(&raw);
                return self.navigate(target);
            }
            Message::OpenUrl(url) => {
                return self.navigate(url);
            }
            Message::GoBack => {
                self.browser.go_back();
                self.invalidate_active_tab_loads();
                self.after_tab_change("Navigated back");
            }
            Message::GoForward => {
                self.browser.go_forward();
                self.invalidate_active_tab_loads();
                self.after_tab_change("Navigated forward");
            }
            Message::Reload => {
                if let Some(tab) = self.browser.active_tab() {
                    let url = tab.url.clone();
                    if url.starts_with("ghita://search") {
                        return self.start_search(&url);
                    }
                    if url.starts_with("http://") || url.starts_with("https://") {
                        return self.start_fetch(url);
                    }
                    self.after_tab_change("Reloaded");
                }
            }
            Message::Home => {
                let home = self.browser.storage.settings.homepage.clone();
                return self.navigate(home);
            }
            Message::FocusUrl => {
                self.show_menu = false;
                return Command::batch([
                    text_input::focus(text_input::Id::new(OMNIBOX_ID)),
                    text_input::select_all(text_input::Id::new(OMNIBOX_ID)),
                ]);
            }
            Message::NtpSearchChanged(q) => {
                self.ntp_search = q;
            }
            Message::NtpSearchSubmit => {
                let raw = self.ntp_search.trim().to_string();
                if raw.is_empty() {
                    return Command::none();
                }
                self.ntp_search.clear();
                let target = self.resolve_omnibox(&raw);
                return self.navigate(target);
            }
            Message::SearchResultsLoaded {
                results,
                query,
                tab_id,
                seq,
            } => {
                // Discard results from a search superseded by a newer load
                if self.pending_loads.get(&tab_id) != Some(&seq) {
                    return Command::none();
                }
                let st = self.search_state.entry(tab_id).or_default();
                st.results = results;
                st.query = query;
                st.loading = false;
                st.error = None;
                let result_count = st.results.len();
                self.is_loading = false;
                self.status_msg = format!("{} results", result_count);
                if self.browser.tabs.active_tab_id() == Some(tab_id) {
                    self.show_suggestions = false;
                    self.sync_from_active_tab();
                }
            }
            Message::SearchError {
                err,
                query,
                tab_id,
                seq,
            } => {
                // Discard errors from a search superseded by a newer load
                if self.pending_loads.get(&tab_id) != Some(&seq) {
                    return Command::none();
                }
                let st = self.search_state.entry(tab_id).or_default();
                st.error = Some(err.clone());
                st.query = query;
                st.loading = false;
                self.is_loading = false;
                self.status_msg = format!("Search failed: {}", err);
                if self.browser.tabs.active_tab_id() == Some(tab_id) {
                    self.show_suggestions = false;
                    self.sync_from_active_tab();
                }
            }
            Message::SelectTab(index) => {
                self.browser.tabs.set_active_by_index(index);
                self.after_tab_change("");
            }
            Message::NewTab => {
                self.open_internal("ghita://newtab", true);
                return text_input::focus(text_input::Id::new(OMNIBOX_ID));
            }
            Message::NewIncognitoTab => {
                self.open_internal("ghita://incognito", true);
                if let Some(tab) = self.browser.active_tab_mut() {
                    tab.incognito = true;
                }
                self.status_msg = "Incognito tab opened — history will not be saved".to_string();
            }
            Message::CloseTab(index) => {
                if let Some(tab) = self.browser.tabs.get_tab_by_index(index) {
                    let id = tab.id;
                    self.browser.tabs.remove_tab(id);
                    self.search_state.remove(&id);
                    self.pending_loads.remove(&id);
                    self.ensure_tab();
                    self.after_tab_change("Tab closed");
                }
            }
            Message::CloseCurrentTab => {
                if let Some(tab) = self.browser.active_tab() {
                    let id = tab.id;
                    self.browser.tabs.remove_tab(id);
                    self.search_state.remove(&id);
                    self.pending_loads.remove(&id);
                    self.ensure_tab();
                    self.after_tab_change("Tab closed");
                }
            }
            Message::ReopenClosedTab => {
                if let Some((url, _title)) = self.browser.tabs.pop_closed_tab() {
                    self.open_internal("ghita://newtab", true);
                    return self.start_fetch(url);
                }
                self.status_msg = "No recently closed tabs".to_string();
            }
            Message::NextTab => {
                self.browser.tabs.activate_next();
                self.after_tab_change("");
            }
            Message::PrevTab => {
                self.browser.tabs.activate_prev();
                self.after_tab_change("");
            }
            Message::SelectTabNumber(n) => {
                let count = self.browser.tab_count();
                if count > 0 {
                    // Ctrl+9 selects the last tab, like Chrome
                    let idx = if n >= count { count - 1 } else { n };
                    self.browser.tabs.set_active_by_index(idx);
                    self.after_tab_change("");
                }
            }
            Message::ToggleMenu => {
                self.show_menu = !self.show_menu;
                self.show_suggestions = false;
            }
            Message::OpenInternalPage(page) => {
                self.show_menu = false;
                self.open_internal(&page, true);
            }
            Message::ToggleBookmark => {
                let target = self
                    .browser
                    .active_tab()
                    .map(|t| (t.url.clone(), t.title.clone()));
                if let Some((url, title)) = target {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let added = self.browser.storage.toggle_bookmark(&url, &title);
                        self.status_msg = if added {
                            "Bookmark added".to_string()
                        } else {
                            "Bookmark removed".to_string()
                        };
                    } else {
                        self.status_msg = "This page cannot be bookmarked".to_string();
                    }
                }
            }
            Message::ToggleBookmarksBar => {
                self.show_bookmarks_bar = !self.show_bookmarks_bar;
                self.browser.storage.settings.show_bookmarks_bar = self.show_bookmarks_bar;
                self.show_menu = false;
            }
            Message::RemoveBookmark(url) => {
                self.browser.storage.remove_bookmark(&url);
                self.status_msg = "Bookmark removed".to_string();
            }
            Message::ToggleFindBar => {
                self.find_bar_open = !self.find_bar_open;
                self.show_menu = false;
                if self.find_bar_open {
                    return text_input::focus(text_input::Id::new(FIND_ID));
                }
                self.find_query.clear();
            }
            Message::FindQueryChanged(q) => {
                self.find_query = q;
            }
            Message::ZoomIn => {
                self.zoom_percent = zoom_step_in(self.zoom_percent);
                self.browser.storage.settings.default_zoom = self.zoom_percent;
                self.canvas_cache.clear();
            }
            Message::ZoomOut => {
                self.zoom_percent = zoom_step_out(self.zoom_percent);
                self.browser.storage.settings.default_zoom = self.zoom_percent;
                self.canvas_cache.clear();
            }
            Message::ZoomReset => {
                self.zoom_percent = 100;
                self.browser.storage.settings.default_zoom = 100;
                self.canvas_cache.clear();
            }
            Message::HistoryQueryChanged(q) => {
                self.history_query = q;
            }
            Message::RemoveHistoryItem(url) => {
                self.browser.storage.remove_history_entry(&url);
            }
            Message::ClearHistory => {
                self.browser.storage.clear_history();
                self.status_msg = "Browsing history cleared".to_string();
            }
            Message::SavePageAs => {
                self.show_menu = false;
                let url = self
                    .browser
                    .active_tab()
                    .map(|t| t.url.clone())
                    .unwrap_or_default();
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    self.status_msg = "Only web pages can be downloaded".to_string();
                    return Command::none();
                }
                self.status_msg = format!("Downloading {}...", url);
                return Command::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let (bytes, name, _ct) =
                                crate::network::download_url(&url).map_err(|e| e.to_string())?;
                            // Sanitize: keep only the final path component so a malicious
                            // Content-Disposition can't traverse dirs or write an absolute path.
                            let name = std::path::Path::new(&name)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "download".to_string());
                            let dir = dirs::download_dir()
                                .or_else(dirs::data_local_dir)
                                .unwrap_or_else(std::env::temp_dir);
                            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                            // Chrome-style unique naming: "file (1).ext"
                            let mut path = dir.join(&name);
                            let mut counter = 1;
                            while path.exists() {
                                let stem = std::path::Path::new(&name)
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("download");
                                let ext = std::path::Path::new(&name)
                                    .extension()
                                    .and_then(|s| s.to_str())
                                    .map(|e| format!(".{}", e))
                                    .unwrap_or_default();
                                path = dir.join(format!("{} ({}){}", stem, counter, ext));
                                counter += 1;
                            }
                            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
                            Ok(crate::storage::DownloadRecord {
                                url,
                                file_name: path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or(&name)
                                    .to_string(),
                                path: path.to_string_lossy().to_string(),
                                size_bytes: bytes.len() as u64,
                                completed_at: chrono::Utc::now().timestamp(),
                                success: true,
                            })
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Task error: {}", e)))
                    },
                    Message::DownloadFinished,
                );
            }
            Message::DownloadFinished(result) => match result {
                Ok(rec) => {
                    self.status_msg = format!(
                        "Downloaded {} ({})",
                        rec.file_name,
                        fmt_bytes(rec.size_bytes)
                    );
                    self.browser.storage.add_download(rec);
                }
                Err(e) => {
                    self.status_msg = format!("Download failed: {}", e);
                }
            },
            Message::ClearDownloads => {
                self.browser.storage.clear_downloads();
                self.status_msg = "Downloads list cleared".to_string();
            }
            Message::SetThemeDark(dark) => {
                self.is_dark_theme = dark;
                self.browser.storage.settings.theme = if dark {
                    "dark".to_string()
                } else {
                    "light".to_string()
                };
            }
            Message::ToggleTheme => {
                let dark = !self.is_dark_theme;
                self.is_dark_theme = dark;
                self.browser.storage.settings.theme = if dark {
                    "dark".to_string()
                } else {
                    "light".to_string()
                };
            }
            Message::SetSearchEngine(engine) => {
                self.browser.storage.settings.search_engine = engine;
            }
            Message::SetPixelRendering(on) => {
                self.browser.storage.settings.pixel_rendering = on;
                self.canvas_cache.clear();
                self.status_msg = if on {
                    "Pixel renderer enabled (Chrome-like painting)".to_string()
                } else {
                    "Text-mode renderer enabled".to_string()
                };
            }
            Message::HomepageChanged(home) => {
                self.homepage_input = home.clone();
                if !home.trim().is_empty() {
                    self.browser.storage.settings.homepage = home.trim().to_string();
                }
            }
            Message::ClearBrowsingData => {
                self.browser.storage.clear_history();
                self.browser.storage.cookies_mut().clear_all();
                self.browser.cache.clear();
                self.status_msg = "Browsing data cleared (history, cookies, cache)".to_string();
            }
            Message::ToggleDevTools => {
                self.show_devtools = !self.show_devtools;
                self.show_menu = false;
                if self.show_devtools {
                    self.js_console_text = self.browser.js_engine.console_output.join("\n");
                    self.status_msg = "DevTools opened".to_string();
                } else {
                    self.status_msg = "DevTools closed".to_string();
                }
            }
            Message::SetDevPane(pane) => {
                self.dev_pane = pane;
                if pane == DevPane::Console {
                    self.js_console_text = self.browser.js_engine.console_output.join("\n");
                }
            }
            Message::JsCodeChanged(code) => {
                self.js_input_text = code;
            }
            Message::ExecuteJs => {
                let code = self.js_input_text.clone();
                if !code.is_empty() {
                    match self.browser.js_engine.execute_script(&code) {
                        Ok(val) => {
                            let output = val.to_display_string();
                            self.browser
                                .js_engine
                                .console_output
                                .push(format!("> {} = {}", code, output));
                            self.status_msg = format!("JS: {} = {}", code, output);
                        }
                        Err(e) => {
                            self.browser
                                .js_engine
                                .console_output
                                .push(format!("> {}  // Error: {}", code, e));
                            self.status_msg = format!("JS Error: {}", e);
                        }
                    }
                    self.js_console_text = self.browser.js_engine.console_output.join("\n");
                    self.js_input_text = String::new();
                }
            }
            Message::EscapePressed => {
                if self.show_menu {
                    self.show_menu = false;
                } else if self.show_suggestions {
                    self.show_suggestions = false;
                } else if self.find_bar_open {
                    self.find_bar_open = false;
                    self.find_query.clear();
                } else if self.show_devtools {
                    self.show_devtools = false;
                }
            }
            Message::PageLoaded {
                result,
                tab_id,
                seq,
            } => {
                // Discard responses for loads superseded by a newer navigation
                // or search in the same tab (e.g. user typed a new URL while
                // the old one was still in flight).
                if self.pending_loads.get(&tab_id) != Some(&seq) {
                    return Command::none();
                }
                let url = result.url.clone();
                let html = result.body.clone();
                let fetch_time = result.fetch_time_ms;

                // Warm the resource cache so repeated visits reuse this response
                self.browser.cache.insert(
                    &url,
                    result.clone(),
                    crate::network::cache_ttl_secs(&result.headers),
                );

                // Persist Set-Cookie headers from the response into the jar,
                // so subsequent requests to the same host send the cookies
                if let Ok(parsed) = url::Url::parse(&url) {
                    if let Some(host) = parsed.host_str() {
                        for header in &result.set_cookie_headers {
                            let cookie =
                                crate::storage::Cookie::from_set_cookie_header(header, host);
                            if !cookie.name.is_empty() {
                                self.browser.storage.cookies_mut().add_cookie(cookie);
                            }
                        }
                    }
                }

                self.is_loading = true;
                self.status_msg = format!("Parsing {}...", url);

                let start = Instant::now();

                // 1. Parse HTML
                let parse_start = Instant::now();
                let dom = parse_html(&html);
                let parse_time = parse_start.elapsed().as_millis() as u64;

                // 2. Extract title
                let title = {
                    if let Some(title_elem) = dom.find_tag("title") {
                        title_elem.text.trim().to_string()
                    } else if let Some(h1_elem) = dom.find_tag("h1") {
                        h1_elem.text.trim().to_string()
                    } else {
                        url.clone()
                    }
                };

                // 3. Extract and parse <style> tags
                let style_start = Instant::now();
                let mut page_css_rules: Vec<crate::css_parser::CssRule> = Vec::new();
                let style_elements = dom.find_all_tags("style");
                for style_elem in &style_elements {
                    let css_text = style_elem.text.trim();
                    if !css_text.is_empty() {
                        let mut rules = crate::css_parser::parse_css(css_text);
                        page_css_rules.append(&mut rules);
                    }
                }
                let all_rules: Vec<crate::css_parser::CssRule> = self
                    .browser
                    .css_rules
                    .iter()
                    .cloned()
                    .chain(page_css_rules)
                    .collect();
                let style_time = style_start.elapsed().as_millis() as u64;

                // 4. Create layout
                let layout_start = Instant::now();
                let layout_tree = crate::layout::create_layout_tree(
                    &dom,
                    &all_rules,
                    self.browser.viewport_width(),
                );
                let layout_time = layout_start.elapsed().as_millis() as u64;

                // 5. Render to text
                let render_start = Instant::now();
                let rendered = if let Some(ref root) = layout_tree {
                    let tr = crate::text_renderer::TextRenderer::new(
                        self.browser.viewport_width(),
                        self.browser.viewport_height(),
                    );
                    tr.render_to_text(root)
                } else {
                    String::from("[Empty page]")
                };
                let render_time = render_start.elapsed().as_millis() as u64;

                // 6. Count DOM nodes
                fn count_elements(el: &crate::parser::Element) -> usize {
                    1 + el.children.iter().map(count_elements).sum::<usize>()
                }
                let dom_nodes = count_elements(&dom);
                let layout_nodes = layout_tree
                    .as_ref()
                    .map(|root| crate::layout::count_layout_nodes(root))
                    .unwrap_or(0);

                let total_time = start.elapsed().as_millis() as u64;

                self.browser.last_render_stats = Some(crate::RenderStats {
                    parse_time_ms: parse_time,
                    style_time_ms: style_time,
                    layout_time_ms: layout_time,
                    render_time_ms: render_time,
                    total_time_ms: total_time,
                    dom_nodes,
                    layout_nodes,
                });

                // 7. Update the originating tab (may differ from the active tab now)
                let incognito = self
                    .browser
                    .tabs
                    .get_tab(tab_id)
                    .map(|t| t.incognito)
                    .unwrap_or(false);
                let target_is_active = self.browser.tabs.active_tab_id() == Some(tab_id);
                if let Some(tab) = self.browser.tabs.get_tab_mut(tab_id) {
                    // Record the freshly loaded page in session history.
                    // push_history dedups consecutive same-URL loads (reloads
                    // and duplicate notifications), and error pages never
                    // enter history (see Tab::go_back's is_error handling).
                    let loaded_entry = crate::tab::HistoryEntry {
                        url: url.clone(),
                        title: title.clone(),
                        dom: dom.clone(),
                        layout: layout_tree.clone(),
                    };
                    tab.push_history(loaded_entry);
                    tab.is_error = false;
                    tab.dom = dom;
                    tab.title = title.clone();
                    tab.url = url.clone();
                    // Keep the fresh layout on the tab for pixel painting & tab switching
                    tab.layout = layout_tree.clone();
                } else {
                    // The tab was closed before the load finished — discard the result
                    self.is_loading = false;
                    return Command::none();
                }

                // 8. Record global browsing history (Chrome-style, skipped in incognito)
                if !incognito {
                    self.browser.storage.add_history(&url, &title);
                }

                self.last_load_time = Some(fetch_time + total_time);
                self.is_loading = false;
                self.render_stats_text = format!(
                    "Fetch: {}ms | Parse: {}ms | Style: {}ms | Layout: {}ms | Render: {}ms | Total: {}ms | {} DOM nodes",
                    fetch_time, parse_time, style_time, layout_time, render_time,
                    total_time, dom_nodes
                );
                self.status_msg = format!("Loaded {} | {}ms", url, fetch_time + total_time);

                // 9. Update the visible UI only if the loaded tab is still active
                if target_is_active {
                    self.rendered_content = rendered;
                    self.display_list = Arc::new(
                        layout_tree
                            .as_ref()
                            .map(|root| crate::paint::build_display_list_with_cache(root, Some(&self.browser.image_cache)))
                            .unwrap_or_default(),
                    );
                    self.canvas_cache.clear();
                    self.show_suggestions = false;
                    self.url_input = url.clone();
                }
            }
            Message::LoadError {
                err,
                url,
                tab_id,
                seq,
            } => {
                // Discard errors for loads superseded by a newer navigation
                // or search in the same tab.
                if self.pending_loads.get(&tab_id) != Some(&seq) {
                    return Command::none();
                }
                self.is_loading = false;
                self.status_msg = format!("Error loading {}: {}", url, err);

                // Turn the failure into an error "page" on the originating tab, so the
                // user actually sees it (even when the load started from an internal page).
                let error_html = format!(
                    "<html><head><title>This site can't be reached</title></head>\
                     <body><h1>This site can't be reached</h1>\
                     <p>{}</p><p>Error: {}</p>\
                     <p>Try checking the address, your connection, or reload (F5).</p>\
                     </body></html>",
                    url, err
                );
                let dom = parse_html(&error_html);
                let target_is_active = self.browser.tabs.active_tab_id() == Some(tab_id);
                if let Some(tab) = self.browser.tabs.get_tab_mut(tab_id) {
                    // Error pages are never recorded in history; the tab keeps
                    // its current history position so Back returns to the last
                    // good page (see Tab::go_back's is_error handling).
                    tab.dom = dom;
                    tab.title = "This site can't be reached".to_string();
                    tab.url = url;
                    tab.layout = None;
                    tab.is_error = true;
                }
                // Refresh the view (content + pixel list) when the failed tab is active
                if target_is_active {
                    self.sync_from_active_tab();
                }
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let pal = self.palette();

        let mut layers = column![self.build_tab_strip(pal), self.build_toolbar(pal),];

        // Thin Chrome-style loading strip under the toolbar
        if self.is_loading {
            layers = layers.push(
                container(text("").size(1))
                    .style(move |_: &Theme| container::Appearance {
                        background: Some(iced::Background::Color(pal.accent)),
                        ..Default::default()
                    })
                    .width(Length::Fill)
                    .height(Length::Fixed(3.0)),
            );
        }

        if self.show_suggestions && !self.url_input.trim().is_empty() {
            layers = layers.push(self.build_suggestions(pal));
        }
        if self.show_menu {
            layers = layers.push(self.build_menu(pal));
        }
        if self.show_bookmarks_bar {
            layers = layers.push(self.build_bookmarks_bar(pal));
        }
        if self.find_bar_open {
            layers = layers.push(self.build_find_bar(pal));
        }

        let content = self.build_content(pal);
        let main: Element<'_, Message> = if self.show_devtools {
            row![content, self.build_devtools_panel(pal)].into()
        } else {
            content
        };

        layers = layers.push(main);
        layers = layers.push(self.build_status_bar(pal));

        Element::from(layers)
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        keyboard::on_key_press(handle_keyboard)
    }
}

// ===== Keyboard Shortcuts (Chrome bindings) =====

fn handle_keyboard(
    key: iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Option<Message> {
    use iced::keyboard::key::Named;
    use iced::keyboard::Key;

    // Function keys first
    if let Key::Named(named) = &key {
        match named {
            Named::F5 => return Some(Message::Reload),
            Named::F6 => return Some(Message::FocusUrl),
            Named::F12 => return Some(Message::ToggleDevTools),
            _ => {}
        }
    }

    if modifiers.control() && modifiers.shift() {
        return match key {
            Key::Character(c) if c == "t" || c == "T" => Some(Message::ReopenClosedTab),
            Key::Character(c) if c == "n" || c == "N" => Some(Message::NewIncognitoTab),
            Key::Character(c) if c == "b" || c == "B" => Some(Message::ToggleBookmarksBar),
            Key::Character(c) if c == "o" || c == "O" => {
                Some(Message::OpenInternalPage("ghita://bookmarks".to_string()))
            }
            Key::Character(c) if c == "i" || c == "I" => Some(Message::ToggleDevTools),
            Key::Named(Named::Tab) => Some(Message::PrevTab),
            _ => None,
        };
    }

    if modifiers.control() {
        return match key {
            Key::Character(c) if c == "l" || c == "L" => Some(Message::FocusUrl),
            Key::Character(c) if c == "t" || c == "T" => Some(Message::NewTab),
            Key::Character(c) if c == "w" || c == "W" => Some(Message::CloseCurrentTab),
            Key::Character(c) if c == "r" || c == "R" => Some(Message::Reload),
            Key::Character(c) if c == "d" || c == "D" => Some(Message::ToggleBookmark),
            Key::Character(c) if c == "f" || c == "F" => Some(Message::ToggleFindBar),
            Key::Character(c) if c == "h" || c == "H" => {
                Some(Message::OpenInternalPage("ghita://history".to_string()))
            }
            Key::Character(c) if c == "j" || c == "J" => {
                Some(Message::OpenInternalPage("ghita://downloads".to_string()))
            }
            Key::Character(c) if c == "=" || c == "+" => Some(Message::ZoomIn),
            Key::Character(c) if c == "-" => Some(Message::ZoomOut),
            Key::Character(c) if c == "0" => Some(Message::ZoomReset),
            Key::Character(c) if ("1"..="8").contains(&c.as_str()) => c
                .as_str()
                .parse::<usize>()
                .ok()
                .map(|n| Message::SelectTabNumber(n - 1)),
            Key::Character(c) if c == "9" => Some(Message::SelectTabNumber(usize::MAX)),
            Key::Named(Named::Tab) => Some(Message::NextTab),
            _ => None,
        };
    }

    if modifiers.alt() {
        return match key {
            Key::Named(Named::ArrowLeft) => Some(Message::GoBack),
            Key::Named(Named::ArrowRight) => Some(Message::GoForward),
            Key::Named(Named::Home) => Some(Message::Home),
            _ => None,
        };
    }

    match key {
        Key::Named(Named::Escape) => Some(Message::EscapePressed),
        _ => None,
    }
}

// ===== Navigation helpers =====

impl GhitaBrowserApp {
    fn palette(&self) -> &'static Pal {
        if self.is_dark_theme {
            &DARK_PAL
        } else {
            &LIGHT_PAL
        }
    }

    /// Chrome omnibox behavior: URL, internal page, or search query
    fn resolve_omnibox(&self, raw: &str) -> String {
        let input = raw.trim();
        if input.starts_with("ghita://") || input.starts_with("about:") {
            return input.to_string();
        }
        if input.starts_with("http://") || input.starts_with("https://") {
            return input.to_string();
        }
        let looks_like_url =
            !input.contains(' ') && (input.contains('.') || input.starts_with("localhost"));
        if looks_like_url {
            // localhost runs plain HTTP by default, everything else is HTTPS
            if input == "localhost" || input.starts_with("localhost:") {
                format!("http://{}", input)
            } else {
                format!("https://{}", input)
            }
        } else {
            search_page_url(input)
        }
    }

    /// Build a search URL from the configured search engine
    fn search_url(&self, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        match self.browser.storage.settings.search_engine.as_str() {
            "bing" => format!("https://www.bing.com/search?q={}", encoded),
            "duckduckgo" => format!("https://duckduckgo.com/?q={}", encoded),
            _ => format!("https://www.google.com/search?q={}", encoded),
        }
    }

    /// Human-readable name of the active search engine
    fn search_engine_name(&self) -> &'static str {
        match self.browser.storage.settings.search_engine.as_str() {
            "bing" => "Bing",
            "duckduckgo" => "DuckDuckGo",
            _ => "Google",
        }
    }

    /// Navigate the current tab to a resolved target (internal page or web URL)
    fn navigate(&mut self, target: String) -> Command<Message> {
        self.show_menu = false;
        self.show_suggestions = false;
        if target.starts_with("ghita://search") {
            self.open_internal(&target, false);
            return self.start_search(&target);
        }
        if target.starts_with("ghita://") || target.starts_with("about:") {
            self.open_internal(&target, false);
            Command::none()
        } else {
            self.start_fetch(target)
        }
    }

    /// Invalidate any fetch/search still in flight for the active tab so its
    /// late response can't overwrite a page the user explicitly navigated to
    /// (back/forward, internal pages, ...).
    fn invalidate_active_tab_loads(&mut self) {
        if let Some(tab_id) = self.browser.tabs.active_tab_id() {
            self.load_seq = self.load_seq.wrapping_add(1);
            let seq = self.load_seq;
            self.pending_loads.insert(tab_id, seq);
        }
    }

    /// Kick off an async network fetch — UI stays responsive
    fn start_fetch(&mut self, url: String) -> Command<Message> {
        self.is_loading = true;
        self.status_msg = format!("Loading {}...", url);
        self.url_input = url.clone();
        self.render_stats_text = String::new();
        self.show_suggestions = false;

        // Bind the load to the tab that started it, so switching tabs mid-load
        // never applies content or history to the wrong tab. The sequence
        // number lets stale responses (from a superseded navigation) be dropped.
        let tab_id = self.browser.tabs.active_tab_id().unwrap_or(0);
        self.load_seq = self.load_seq.wrapping_add(1);
        let seq = self.load_seq;
        self.pending_loads.insert(tab_id, seq);
        let fetch_url = url.clone();
        let err_url = url;
        // Cookie-aware fetch: inject the stored cookie jar, and hand the raw
        // result back so the handler can persist Set-Cookie headers and warm
        // the resource cache (mirrors Browser::load_url).
        let cookie_store = self.browser.storage.cookies().clone();
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut store = cookie_store;
                    crate::network::fetch_with_cookies(&fetch_url, &mut store)
                        .map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(format!("Task error: {}", e)))
            },
            move |result| match result {
                Ok(result) => Message::PageLoaded {
                    result,
                    tab_id,
                    seq,
                },
                Err(e) => Message::LoadError {
                    err: e,
                    url: err_url,
                    tab_id,
                    seq,
                },
            },
        )
    }

    /// Kick off an async web search for a ghita://search page — UI stays responsive
    fn start_search(&mut self, page_url: &str) -> Command<Message> {
        let query = search_query_from_url(page_url).unwrap_or_default();
        let tab_id = self.browser.tabs.active_tab_id().unwrap_or(0);
        let query_clone = query.clone();

        // Invalidate any in-flight load/search for this tab; stale results
        // (an earlier query in the same tab) will be dropped by the seq check.
        self.load_seq = self.load_seq.wrapping_add(1);
        let seq = self.load_seq;
        self.pending_loads.insert(tab_id, seq);

        let st = self.search_state.entry(tab_id).or_default();
        st.query = query.clone();
        st.results.clear();
        st.loading = true;
        st.error = None;

        self.is_loading = true;
        self.status_msg = format!("Searching for \"{}\"...", query);

        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || search_web(&query))
                    .await
                    .unwrap_or_else(|e| Err(format!("Task error: {}", e)))
            },
            move |result| match result {
                Ok(results) => Message::SearchResultsLoaded {
                    results,
                    query: query_clone.clone(),
                    tab_id,
                    seq,
                },
                Err(err) => Message::SearchError {
                    err,
                    query: query_clone,
                    tab_id,
                    seq,
                },
            },
        )
    }

    /// Open a ghita:// internal page in the current tab or a new tab
    fn open_internal(&mut self, url: &str, new_tab: bool) {
        let url = if url == "about:blank" {
            "ghita://newtab"
        } else {
            url
        };
        let (title, html) = internal_page_meta(url);
        let dom = parse_html(&html);

        if new_tab || self.browser.active_tab().is_none() {
            self.browser.add_tab(url, dom, &title);
        } else if let Some(tab) = self.browser.active_tab_mut() {
            // Internal pages are part of session history; push_history dedups
            // re-opening the same page (e.g. clicking Settings repeatedly).
            let entry = crate::tab::HistoryEntry {
                url: url.to_string(),
                title: title.clone(),
                dom: dom.clone(),
                layout: None,
            };
            tab.push_history(entry);
            tab.is_error = false;
            tab.dom = dom;
            tab.title = title.clone();
            tab.url = url.to_string();
        }

        // An internal navigation supersedes any fetch still in flight for this
        // tab, so its response can no longer overwrite this page.
        if !new_tab {
            self.invalidate_active_tab_loads();
        }

        self.is_loading = false;
        self.show_menu = false;
        self.sync_from_active_tab();
        self.status_msg = title;
    }

    /// Make sure at least one tab exists (Chrome never shows zero tabs)
    fn ensure_tab(&mut self) {
        if self.browser.tab_count() == 0 {
            self.open_internal("ghita://newtab", true);
        }
    }

    /// Refresh omnibox + content after the active tab changed
    fn sync_from_active_tab(&mut self) {
        let url = self
            .browser
            .active_tab()
            .map(|t| t.url.clone())
            .unwrap_or_default();
        self.url_input = if is_blank_page(&url) {
            String::new()
        } else {
            url
        };
        self.rendered_content = self.browser.render_current();
        self.rebuild_display_list();
        self.show_suggestions = false;
    }

    /// Rebuild the pixel display list for the active tab
    fn rebuild_display_list(&mut self) {
        let list = if let Some(tab) = self.browser.active_tab() {
            if is_internal_page(&tab.url) {
                DisplayList::default()
            } else if let Some(ref root) = tab.layout {
                crate::paint::build_display_list_with_cache(root, Some(&self.browser.image_cache))
            } else {
                // No cached layout (e.g. restored history entry): re-layout from the DOM
                crate::layout::create_layout_tree(
                    &tab.dom,
                    &self.browser.css_rules,
                    self.browser.viewport_width(),
                )
                .map(|root| crate::paint::build_display_list_with_cache(&root, Some(&self.browser.image_cache)))
                .unwrap_or_default()
            }
        } else {
            DisplayList::default()
        };
        self.display_list = Arc::new(list);
        self.canvas_cache.clear();
    }

    fn after_tab_change(&mut self, status: &str) {
        self.sync_from_active_tab();
        self.is_loading = false;
        if !status.is_empty() {
            self.status_msg = status.to_string();
        }
    }
}

fn is_blank_page(url: &str) -> bool {
    url.is_empty() || url == "ghita://newtab" || url == "ghita://incognito" || url == "about:blank"
}

fn is_internal_page(url: &str) -> bool {
    url.starts_with("ghita://") || url.starts_with("about:")
}

/// Title + placeholder DOM HTML for internal ghita:// pages
fn internal_page_meta(url: &str) -> (String, String) {
    let title = match url {
        "ghita://newtab" => "New Tab",
        "ghita://incognito" => "New Incognito Tab",
        "ghita://history" => "History",
        "ghita://bookmarks" => "Bookmarks",
        "ghita://downloads" => "Downloads",
        "ghita://settings" => "Settings",
        "ghita://about" => "About GhitaBrowser",
        _ if url.starts_with("ghita://search") => {
            let q = search_query_from_url(url).unwrap_or_else(|| "web".to_string());
            // Escape the user-supplied query before it is embedded in the
            // generated HTML, so quotes/angle brackets can't break the page
            // (or smuggle markup into it).
            let safe_q = html_escape(&q);
            return (
                format!("Search: {}", q),
                format!(
                    "<html><head><title>Search: {}</title></head><body><h1>Search: {}</h1></body></html>",
                    safe_q, safe_q
                ),
            );
        }
        _ => "New Tab",
    };
    let html = format!(
        "<html><head><title>{}</title></head><body><h1>{}</h1></body></html>",
        title, title
    );
    (title.to_string(), html)
}

/// Escape text for safe insertion into generated HTML (F8)
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Build the in-app search results URL for a query (ghita://search?q=...)
fn search_page_url(query: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    format!("ghita://search?q={}", encoded)
}

/// Extract the query from a ghita://search?q=... URL
fn search_query_from_url(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == "q")
        .map(|(_, v)| v.into_owned())
}

// ===== UI builders =====

impl GhitaBrowserApp {
    /// Chrome tab strip: rounded tabs, favicon, close button, "+" button
    fn build_tab_strip(&self, pal: &'static Pal) -> Element<'_, Message> {
        let mut strip = row![].spacing(1).padding([6, 8, 0, 8]);

        let tab_info: Vec<(usize, String, String, bool)> = self
            .browser
            .tabs
            .iter_tabs()
            .into_iter()
            .map(|t| (t.id, t.title.clone(), t.url.clone(), t.incognito))
            .collect();
        let active_id = self.browser.tabs.active_tab_id();

        for (i, (id, title, url, incognito)) in tab_info.iter().enumerate() {
            let is_active = Some(*id) == active_id;

            let (bg, txt) = if is_active {
                (pal.toolbar, pal.text)
            } else {
                (pal.tab_strip, pal.text_dim)
            };
            let hover = if is_active {
                pal.toolbar
            } else {
                pal.tab_hover
            };

            // Favicon-ish glyph: incognito / internal / regular page
            let icon = if *incognito {
                "🕶"
            } else if is_internal_page(url) {
                "🦀"
            } else {
                "🌐"
            };

            let label = row![
                text(icon).size(11),
                text(truncate_label(title, 18)).size(12),
            ]
            .spacing(6)
            .align_items(iced::Alignment::Center);

            let tab_btn = button(label)
                .on_press(Message::SelectTab(i))
                .padding([6, 4, 6, 10])
                .style(chrome_btn(bg, hover, txt, [8.0, 0.0, 0.0, 0.0]));

            let close_btn = button(text("✕").size(10))
                .on_press(Message::CloseTab(i))
                .padding([8, 8, 8, 4])
                .style(chrome_btn(bg, hover, pal.text_dim, [0.0, 8.0, 0.0, 0.0]));

            strip = strip.push(row![tab_btn, close_btn].spacing(0));
        }

        // "+" new tab button
        let new_tab_btn = button(text("+").size(16))
            .on_press(Message::NewTab)
            .padding([2, 10])
            .style(chrome_btn(
                pal.tab_strip,
                pal.tab_hover,
                pal.text_dim,
                [8.0, 8.0, 8.0, 8.0],
            ));
        strip = strip.push(new_tab_btn);

        container(strip)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.tab_strip)),
                ..Default::default()
            })
            .width(Length::Fill)
            .into()
    }

    /// Chrome toolbar: nav buttons + omnibox (padlock & star inside) + downloads + menu
    fn build_toolbar(&self, pal: &'static Pal) -> Element<'_, Message> {
        let can_go_back = self
            .browser
            .active_tab()
            .map(|t| t.can_go_back())
            .unwrap_or(false);
        let can_go_forward = self
            .browser
            .active_tab()
            .map(|t| t.can_go_forward())
            .unwrap_or(false);

        let back_btn = button(text("←").size(16))
            .on_press_maybe(if can_go_back {
                Some(Message::GoBack)
            } else {
                None
            })
            .padding([4, 9])
            .style(if can_go_back {
                chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4])
            } else {
                chrome_btn(pal.toolbar, pal.toolbar, pal.divider, [16.0; 4])
            });

        let fwd_btn = button(text("→").size(16))
            .on_press_maybe(if can_go_forward {
                Some(Message::GoForward)
            } else {
                None
            })
            .padding([4, 9])
            .style(if can_go_forward {
                chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4])
            } else {
                chrome_btn(pal.toolbar, pal.toolbar, pal.divider, [16.0; 4])
            });

        let reload_btn = button(text("⟳").size(16))
            .on_press(Message::Reload)
            .padding([4, 9])
            .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4]));

        let home_btn = button(text("⌂").size(16))
            .on_press(Message::Home)
            .padding([4, 9])
            .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4]));

        // Security chip inside the omnibox
        let active_url = self
            .browser
            .active_tab()
            .map(|t| t.url.clone())
            .unwrap_or_default();
        let padlock: Element<'_, Message> = if active_url.starts_with("https://") {
            text("🔒")
                .size(12)
                .style(iced::theme::Text::from(pal.secure))
                .into()
        } else if active_url.starts_with("http://") {
            text("⚠")
                .size(12)
                .style(iced::theme::Text::from(pal.danger))
                .into()
        } else if is_internal_page(&active_url) {
            text("🦀").size(12).into()
        } else {
            text("🔍")
                .size(12)
                .style(iced::theme::Text::from(pal.text_dim))
                .into()
        };

        let placeholder = format!("Search {} or type a URL", self.search_engine_name());
        let url_field = text_input(&placeholder, &self.url_input)
            .id(text_input::Id::new(OMNIBOX_ID))
            .on_input(Message::UrlChanged)
            .on_submit(Message::Navigate)
            .size(13)
            .padding([6, 8])
            .width(Length::Fill)
            .style(omnibox_style(pal, 14.0));

        // Bookmark star (Ctrl+D), Chrome-style, inside the omnibox capsule
        let bookmarked = self.browser.storage.is_bookmarked(&active_url);
        let star_btn = button(text(if bookmarked { "★" } else { "☆" }).size(14).style(
            iced::theme::Text::from(if bookmarked { pal.accent } else { pal.text_dim }),
        ))
        .on_press(Message::ToggleBookmark)
        .padding([4, 8])
        .style(chrome_btn(
            pal.omnibox,
            pal.menu_hover,
            pal.text_dim,
            [0.0, 14.0, 14.0, 0.0],
        ));

        let omnibox = container(
            row![
                container(padlock).padding([0, 4, 0, 10]),
                url_field,
                star_btn,
            ]
            .spacing(0)
            .align_items(iced::Alignment::Center),
        )
        .style(move |_: &Theme| container::Appearance {
            background: Some(iced::Background::Color(pal.omnibox)),
            border: iced::Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 14.0.into(),
            },
            ..Default::default()
        })
        .width(Length::Fill);

        let downloads_btn = button(text("⬇").size(15))
            .on_press(Message::OpenInternalPage("ghita://downloads".to_string()))
            .padding([4, 9])
            .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4]));

        let profile_btn = button(text("👤").size(14))
            .on_press(Message::OpenInternalPage("ghita://settings".to_string()))
            .padding([4, 9])
            .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4]));

        let menu_btn = button(text("⋮").size(16))
            .on_press(Message::ToggleMenu)
            .padding([4, 11])
            .style(if self.show_menu {
                chrome_btn(pal.menu_hover, pal.menu_hover, pal.text, [16.0; 4])
            } else {
                chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4])
            });

        let bar = row![
            back_btn,
            fwd_btn,
            reload_btn,
            home_btn,
            omnibox,
            downloads_btn,
            profile_btn,
            menu_btn,
        ]
        .spacing(4)
        .padding([6, 10])
        .align_items(iced::Alignment::Center);

        container(bar)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.toolbar)),
                ..Default::default()
            })
            .width(Length::Fill)
            .into()
    }

    /// Omnibox dropdown: search suggestion + matching history/bookmarks
    fn build_suggestions(&self, pal: &'static Pal) -> Element<'_, Message> {
        let query = self.url_input.trim().to_string();
        let q_lower = query.to_lowercase();

        let mut items = column![].spacing(0);

        // First row: search with the default engine (like Chrome)
        let search_label = format!(
            "Search {} for \"{}\"",
            self.search_engine_name(),
            truncate_label(&query, 50)
        );
        items = items.push(
            button(
                row![
                    text("🔍").size(12),
                    text(search_label)
                        .size(13)
                        .style(iced::theme::Text::from(pal.text)),
                ]
                .spacing(10)
                .align_items(iced::Alignment::Center),
            )
            .on_press(Message::OpenUrl(search_page_url(&query)))
            .padding([7, 16])
            .width(Length::Fill)
            .style(chrome_btn(pal.menu_bg, pal.menu_hover, pal.text, [0.0; 4])),
        );

        // Matching history entries + bookmarks (up to 6)
        let mut seen: Vec<String> = Vec::new();
        let mut matches: Vec<(String, String, &'static str)> = Vec::new();
        for h in self.browser.storage.history() {
            if matches.len() >= 5 {
                break;
            }
            if (h.url.to_lowercase().contains(&q_lower)
                || h.title.to_lowercase().contains(&q_lower))
                && !seen.contains(&h.url)
            {
                seen.push(h.url.clone());
                matches.push((h.title.clone(), h.url.clone(), "🕐"));
            }
        }
        for b in self.browser.storage.bookmarks() {
            if matches.len() >= 6 {
                break;
            }
            if (b.url.to_lowercase().contains(&q_lower)
                || b.title.to_lowercase().contains(&q_lower))
                && !seen.contains(&b.url)
            {
                seen.push(b.url.clone());
                matches.push((b.title.clone(), b.url.clone(), "★"));
            }
        }

        for (title, url, icon) in matches {
            items = items.push(
                button(
                    row![
                        text(icon).size(12),
                        text(truncate_label(&title, 40))
                            .size(13)
                            .style(iced::theme::Text::from(pal.text)),
                        text(truncate_label(&url, 60))
                            .size(12)
                            .style(iced::theme::Text::from(pal.text_dim)),
                    ]
                    .spacing(10)
                    .align_items(iced::Alignment::Center),
                )
                .on_press(Message::OpenUrl(url))
                .padding([7, 16])
                .width(Length::Fill)
                .style(chrome_btn(pal.menu_bg, pal.menu_hover, pal.text, [0.0; 4])),
            );
        }

        container(items)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.menu_bg)),
                border: iced::Border {
                    color: pal.divider,
                    width: 1.0,
                    radius: [0.0, 0.0, 8.0, 8.0].into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .into()
    }

    /// Chrome three-dot dropdown menu
    fn build_menu(&self, pal: &'static Pal) -> Element<'_, Message> {
        let item = |label: &str, shortcut: &str, msg: Message| -> Element<'_, Message> {
            button(
                row![
                    text(label.to_string())
                        .size(13)
                        .style(iced::theme::Text::from(pal.text)),
                    horizontal_space(),
                    text(shortcut.to_string())
                        .size(11)
                        .style(iced::theme::Text::from(pal.text_dim)),
                ]
                .align_items(iced::Alignment::Center),
            )
            .on_press(msg)
            .padding([7, 16])
            .width(Length::Fill)
            .style(chrome_btn(pal.menu_bg, pal.menu_hover, pal.text, [0.0; 4]))
            .into()
        };

        let divider = || -> Element<'_, Message> {
            container(text("").size(1))
                .style(move |_: &Theme| container::Appearance {
                    background: Some(iced::Background::Color(pal.divider)),
                    ..Default::default()
                })
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .into()
        };

        // Zoom control row (- 100% +)
        let zoom_row: Element<'_, Message> = row![
            text("Zoom")
                .size(13)
                .style(iced::theme::Text::from(pal.text)),
            horizontal_space(),
            button(text("−").size(13))
                .on_press(Message::ZoomOut)
                .padding([2, 10])
                .style(chrome_btn(pal.menu_bg, pal.menu_hover, pal.text, [4.0; 4])),
            text(format!("{}%", self.zoom_percent))
                .size(12)
                .style(iced::theme::Text::from(pal.text_dim)),
            button(text("+").size(13))
                .on_press(Message::ZoomIn)
                .padding([2, 10])
                .style(chrome_btn(pal.menu_bg, pal.menu_hover, pal.text, [4.0; 4])),
        ]
        .spacing(8)
        .padding([4, 16])
        .align_items(iced::Alignment::Center)
        .into();

        let menu = column![
            item("New tab", "Ctrl+T", Message::NewTab),
            item(
                "New Incognito tab",
                "Ctrl+Shift+N",
                Message::NewIncognitoTab
            ),
            item(
                "Reopen closed tab",
                "Ctrl+Shift+T",
                Message::ReopenClosedTab
            ),
            divider(),
            item(
                "History",
                "Ctrl+H",
                Message::OpenInternalPage("ghita://history".to_string())
            ),
            item(
                "Downloads",
                "Ctrl+J",
                Message::OpenInternalPage("ghita://downloads".to_string())
            ),
            item(
                "Bookmarks",
                "Ctrl+Shift+O",
                Message::OpenInternalPage("ghita://bookmarks".to_string())
            ),
            item(
                "Show bookmarks bar",
                "Ctrl+Shift+B",
                Message::ToggleBookmarksBar
            ),
            divider(),
            zoom_row,
            item("Find in page...", "Ctrl+F", Message::ToggleFindBar),
            item("Save page as...", "", Message::SavePageAs),
            divider(),
            item(
                "Settings",
                "",
                Message::OpenInternalPage("ghita://settings".to_string())
            ),
            item("Developer tools", "F12", Message::ToggleDevTools),
            item(
                "About GhitaBrowser",
                "",
                Message::OpenInternalPage("ghita://about".to_string())
            ),
        ]
        .spacing(0);

        let panel = container(menu)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.menu_bg)),
                border: iced::Border {
                    color: pal.divider,
                    width: 1.0,
                    radius: 8.0.into(),
                },
                shadow: Shadow::default(),
                ..Default::default()
            })
            .padding([6, 0])
            .width(Length::Fixed(300.0));

        row![horizontal_space(), panel].padding([0, 8]).into()
    }

    /// Chrome bookmarks bar
    fn build_bookmarks_bar(&self, pal: &'static Pal) -> Element<'_, Message> {
        let mut bar = row![]
            .spacing(2)
            .padding([3, 10])
            .align_items(iced::Alignment::Center);

        let bookmarks: Vec<(String, String)> = self
            .browser
            .storage
            .bookmarks()
            .iter()
            .map(|b| (b.title.clone(), b.url.clone()))
            .collect();

        if bookmarks.is_empty() {
            bar = bar.push(
                text("For quick access, place your bookmarks here — press ☆ or Ctrl+D")
                    .size(11)
                    .style(iced::theme::Text::from(pal.text_dim)),
            );
        } else {
            for (title, url) in bookmarks {
                bar = bar.push(
                    button(
                        row![
                            text("★")
                                .size(10)
                                .style(iced::theme::Text::from(pal.accent)),
                            text(truncate_label(&title, 18))
                                .size(12)
                                .style(iced::theme::Text::from(pal.text)),
                        ]
                        .spacing(5)
                        .align_items(iced::Alignment::Center),
                    )
                    .on_press(Message::OpenUrl(url))
                    .padding([3, 8])
                    .style(chrome_btn(
                        pal.toolbar,
                        pal.menu_hover,
                        pal.text,
                        [10.0; 4],
                    )),
                );
            }
        }

        container(bar)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.toolbar)),
                border: iced::Border {
                    color: pal.divider,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .into()
    }

    /// Find-in-page bar (Ctrl+F), Chrome puts it top-right
    fn build_find_bar(&self, pal: &'static Pal) -> Element<'_, Message> {
        let match_count = if self.find_query.is_empty() {
            String::new()
        } else {
            let n = self
                .rendered_content
                .to_lowercase()
                .matches(&self.find_query.to_lowercase())
                .count();
            format!("{} match{}", n, if n == 1 { "" } else { "es" })
        };

        let panel = container(
            row![
                text_input("Find in page", &self.find_query)
                    .id(text_input::Id::new(FIND_ID))
                    .on_input(Message::FindQueryChanged)
                    .size(13)
                    .padding([5, 10])
                    .width(Length::Fixed(220.0))
                    .style(omnibox_style(pal, 4.0)),
                text(match_count)
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
                button(text("✕").size(12))
                    .on_press(Message::ToggleFindBar)
                    .padding([4, 8])
                    .style(chrome_btn(
                        pal.menu_bg,
                        pal.menu_hover,
                        pal.text_dim,
                        [4.0; 4]
                    )),
            ]
            .spacing(10)
            .padding(8)
            .align_items(iced::Alignment::Center),
        )
        .style(move |_: &Theme| container::Appearance {
            background: Some(iced::Background::Color(pal.menu_bg)),
            border: iced::Border {
                color: pal.divider,
                width: 1.0,
                radius: [0.0, 0.0, 8.0, 8.0].into(),
            },
            ..Default::default()
        });

        row![horizontal_space(), panel]
            .padding([0, 40, 0, 0])
            .into()
    }

    /// Dispatch to internal pages or the web view
    fn build_content(&self, pal: &'static Pal) -> Element<'_, Message> {
        let url = self
            .browser
            .active_tab()
            .map(|t| t.url.clone())
            .unwrap_or_default();

        let inner: Element<'_, Message> = match url.as_str() {
            "ghita://newtab" | "about:blank" | "" => self.build_newtab_page(pal),
            "ghita://incognito" => self.build_incognito_page(pal),
            "ghita://history" => self.build_history_page(pal),
            "ghita://bookmarks" => self.build_bookmarks_page(pal),
            "ghita://downloads" => self.build_downloads_page(pal),
            "ghita://settings" => self.build_settings_page(pal),
            "ghita://about" => self.build_about_page(pal),
            _ if url.starts_with("ghita://search") => self.build_search_page(pal),
            _ => self.build_web_view(pal),
        };

        container(inner)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.content_bg)),
                text_color: Some(pal.text),
                ..Default::default()
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Rendered web page: real pixel canvas (Chrome-like) with text-mode fallback
    fn build_web_view(&self, pal: &'static Pal) -> Element<'_, Message> {
        let pixel_mode = self.browser.storage.settings.pixel_rendering;

        if pixel_mode && !self.display_list.is_empty() {
            let zoom = self.zoom_percent as f32 / 100.0;
            let base_url = self
                .browser
                .active_tab()
                .map(|t| t.url.clone())
                .unwrap_or_default();

            // Real pixel painting: backgrounds, borders, styled glyphs, clickable links
            let page = canvas::Canvas::new(PageCanvas {
                list: self.display_list.clone(),
                cache: &self.canvas_cache,
                zoom,
                base_url,
            })
            .width(Length::Fixed(self.display_list.width * zoom))
            .height(Length::Fixed(self.display_list.height * zoom));

            // Center the page sheet like Chrome does with fixed-width documents
            let mut items: Vec<Element<'_, Message>> =
                vec![container(page).width(Length::Fill).center_x().into()];
            if !self.render_stats_text.is_empty() {
                items.push(
                    container(
                        text(&self.render_stats_text)
                            .size(10)
                            .style(iced::theme::Text::from(pal.text_dim)),
                    )
                    .width(Length::Fill)
                    .center_x()
                    .into(),
                );
            }

            return scrollable(column(items).spacing(4))
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        // Legacy text-mode renderer (also used for error pages)
        let content_size = (13.0 * self.zoom_percent as f32 / 100.0).max(6.0);

        let mut items: Vec<Element<'_, Message>> = vec![text(&self.rendered_content)
            .size(content_size)
            .style(iced::theme::Text::from(pal.text))
            .into()];

        if !self.render_stats_text.is_empty() {
            items.push(vertical_space().height(10).into());
            items.push(
                text(&self.render_stats_text)
                    .size(10)
                    .style(iced::theme::Text::from(pal.text_dim))
                    .into(),
            );
        }

        scrollable(column(items).spacing(4).padding(16))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

// ===== Pixel page canvas (the real graphics renderer) =====

/// Convert an engine RGBA color to an Iced color
fn to_color(c: crate::paint::Rgba) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

/// Resolve a (possibly relative) href against the current page URL
fn resolve_href(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("ghita://") {
        return href.to_string();
    }
    if href.starts_with('#') {
        return base.to_string();
    }
    url::Url::parse(base)
        .ok()
        .and_then(|b| b.join(href).ok())
        .map(|u| u.to_string())
        .unwrap_or_else(|| href.to_string())
}

/// Canvas program that paints the display list with real pixels
/// and hit-tests link regions for Chrome-style clickable navigation.
struct PageCanvas<'a> {
    list: Arc<DisplayList>,
    cache: &'a canvas::Cache,
    zoom: f32,
    base_url: String,
}

impl<'a> canvas::Program<Message> for PageCanvas<'a> {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if let Some(pos) = cursor.position_in(bounds) {
                let x = pos.x / self.zoom;
                let y = pos.y / self.zoom;
                if let Some(href) = self.list.link_at(x, y) {
                    let resolved = resolve_href(&self.base_url, href);
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::OpenUrl(resolved)),
                    );
                }
            }
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            frame.scale(self.zoom);

            for item in &self.list.items {
                match item {
                    DisplayItem::Rect { x, y, w, h, color } => {
                        if color.a > 0.0 {
                            frame.fill_rectangle(
                                iced::Point::new(*x, *y),
                                iced::Size::new(*w, *h),
                                to_color(*color),
                            );
                        }
                    }
                    DisplayItem::Border {
                        x,
                        y,
                        w,
                        h,
                        width,
                        color,
                    } => {
                        let c = to_color(*color);
                        let bw = width.max(0.5);
                        // Four thin rects: top, bottom, left, right
                        frame.fill_rectangle(iced::Point::new(*x, *y), iced::Size::new(*w, bw), c);
                        frame.fill_rectangle(
                            iced::Point::new(*x, *y + *h - bw),
                            iced::Size::new(*w, bw),
                            c,
                        );
                        frame.fill_rectangle(iced::Point::new(*x, *y), iced::Size::new(bw, *h), c);
                        frame.fill_rectangle(
                            iced::Point::new(*x + *w - bw, *y),
                            iced::Size::new(bw, *h),
                            c,
                        );
                    }
                    DisplayItem::TextRun {
                        x,
                        y,
                        size,
                        color,
                        content,
                        bold,
                        italic,
                        underline,
                        monospace,
                    } => {
                        let mut font = if *monospace {
                            iced::Font::MONOSPACE
                        } else {
                            iced::Font::default()
                        };
                        if *bold {
                            font.weight = iced::font::Weight::Bold;
                        }
                        if *italic {
                            font.style = iced::font::Style::Italic;
                        }
                        frame.fill_text(canvas::Text {
                            content: content.clone(),
                            position: iced::Point::new(*x, *y),
                            color: to_color(*color),
                            size: iced::Pixels(*size),
                            font,
                            shaping: iced::widget::text::Shaping::Advanced,
                            ..canvas::Text::default()
                        });
                        if *underline {
                            let text_width = crate::layout::estimate_text_width(content, *size as f64) as f32;
                            frame.fill_rectangle(
                                iced::Point::new(*x, *y + size * 1.18),
                                iced::Size::new(text_width, 1.0),
                                to_color(*color),
                            );
                        }
                    }
                    DisplayItem::Image {
                        x,
                        y,
                        w,
                        h,
                        url: _,
                        alt,
                        cached,
                    } => {
                        let bg = if *cached {
                            iced::Color::from_rgb(0.78, 0.88, 0.78) // light green = loaded
                        } else {
                            iced::Color::from_rgb(0.85, 0.85, 0.85) // gray = not loaded yet
                        };
                        frame.fill_rectangle(
                            iced::Point::new(*x, *y),
                            iced::Size::new(*w, *h),
                            bg,
                        );
                        // Show a small label inside the image box
                        let label = if *cached {
                            format!("📷 {}", alt)
                        } else {
                            format!("🖼 {}", alt)
                        };
                        frame.fill_text(canvas::Text {
                            content: label,
                            position: iced::Point::new(*x + 2.0, *y + 2.0),
                            color: iced::Color::from_rgb(0.3, 0.3, 0.3),
                            size: iced::Pixels(12.0),
                            font: iced::Font::default(),
                            shaping: iced::widget::text::Shaping::Advanced,
                            ..canvas::Text::default()
                        });
                    }
                }
            }
        });

        vec![geometry]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if let Some(pos) = cursor.position_in(bounds) {
            if self
                .list
                .link_at(pos.x / self.zoom, pos.y / self.zoom)
                .is_some()
            {
                return mouse::Interaction::Pointer;
            }
        }
        mouse::Interaction::default()
    }
}

// ===== Internal pages (ghita://) =====

impl GhitaBrowserApp {
    /// Chrome New Tab page: wordmark, search box, most-visited tiles
    fn build_newtab_page(&self, pal: &'static Pal) -> Element<'_, Message> {
        // Colored wordmark in the brand "Fire" palette
        let wordmark = row![
            text("G").size(44).style(iced::theme::Text::from(GH_ORANGE)),
            text("h").size(44).style(iced::theme::Text::from(GH_AMBER)),
            text("i").size(44).style(iced::theme::Text::from(GH_RED)),
            text("t")
                .size(44)
                .style(iced::theme::Text::from(GH_CRIMSON)),
            text("a").size(44).style(iced::theme::Text::from(GH_EMBER)),
            text("Browser")
                .size(44)
                .style(iced::theme::Text::from(pal.text_dim)),
        ]
        .spacing(1)
        .align_items(iced::Alignment::Center);

        let placeholder = format!("Search {} or type a URL", self.search_engine_name());
        let search_box = container(
            text_input(&placeholder, &self.ntp_search)
                .id(text_input::Id::new(NTP_SEARCH_ID))
                .on_input(Message::NtpSearchChanged)
                .on_submit(Message::NtpSearchSubmit)
                .size(14)
                .padding([10, 20])
                .style(omnibox_style(pal, 22.0)),
        )
        .width(Length::Fixed(560.0));

        // Most visited tiles (top sites from real history)
        let top_sites = self.browser.storage.top_sites(8);
        let mut tiles_col = column![].spacing(12).align_items(iced::Alignment::Center);
        for chunk in top_sites.chunks(4) {
            let mut tile_row = row![].spacing(12);
            for site in chunk {
                let initial = site
                    .title
                    .chars()
                    .next()
                    .unwrap_or('•')
                    .to_uppercase()
                    .to_string();
                let tile = button(
                    column![
                        text(initial)
                            .size(22)
                            .style(iced::theme::Text::from(pal.accent)),
                        text(truncate_label(&site.title, 14))
                            .size(11)
                            .style(iced::theme::Text::from(pal.text)),
                    ]
                    .spacing(6)
                    .align_items(iced::Alignment::Center),
                )
                .on_press(Message::OpenUrl(site.url.clone()))
                .padding([14, 10])
                .width(Length::Fixed(112.0))
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [8.0; 4]));
                tile_row = tile_row.push(tile);
            }
            tiles_col = tiles_col.push(tile_row);
        }
        if top_sites.is_empty() {
            tiles_col = tiles_col.push(
                text("Your most visited sites will appear here")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
            );
        }

        let page = column![
            vertical_space().height(70),
            wordmark,
            vertical_space().height(28),
            search_box,
            vertical_space().height(36),
            tiles_col,
            vertical_space().height(24),
            text(format!(
                "GhitaBrowser v{} — 100% Rust, 0% C++",
                crate::VERSION
            ))
            .size(11)
            .style(iced::theme::Text::from(pal.text_dim)),
        ]
        .align_items(iced::Alignment::Center)
        .width(Length::Fill);

        scrollable(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Chrome incognito New Tab page
    fn build_incognito_page(&self, pal: &'static Pal) -> Element<'_, Message> {
        let page = column![
            vertical_space().height(90),
            text("🕶").size(52),
            vertical_space().height(16),
            text("You've gone incognito")
                .size(24)
                .style(iced::theme::Text::from(pal.text)),
            vertical_space().height(12),
            text("Pages you view in this tab won't appear in the browser history.")
                .size(13)
                .style(iced::theme::Text::from(pal.text_dim)),
            text("Note: cookies and cache are still shared in this build of the Rust engine.")
                .size(12)
                .style(iced::theme::Text::from(pal.text_dim)),
        ]
        .align_items(iced::Alignment::Center)
        .width(Length::Fill);

        scrollable(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// chrome://history equivalent
    fn build_history_page(&self, pal: &'static Pal) -> Element<'_, Message> {
        let header = row![
            text("History")
                .size(24)
                .style(iced::theme::Text::from(pal.text)),
            horizontal_space(),
            button(text("Clear browsing data").size(12))
                .on_press(Message::ClearHistory)
                .padding([6, 14])
                .style(chrome_btn(pal.danger, pal.danger, pal.on_accent, [6.0; 4])),
        ]
        .align_items(iced::Alignment::Center);

        let search = text_input("Search history", &self.history_query)
            .on_input(Message::HistoryQueryChanged)
            .size(13)
            .padding([7, 14])
            .style(omnibox_style(pal, 16.0));

        let q = self.history_query.trim().to_lowercase();
        let mut list = column![].spacing(2);
        let mut shown = 0;
        for h in self.browser.storage.history() {
            if !q.is_empty()
                && !h.url.to_lowercase().contains(&q)
                && !h.title.to_lowercase().contains(&q)
            {
                continue;
            }
            if shown >= 200 {
                break;
            }
            shown += 1;

            list = list.push(
                row![
                    text(fmt_timestamp(h.visited_at))
                        .size(11)
                        .style(iced::theme::Text::from(pal.text_dim))
                        .width(Length::Fixed(120.0)),
                    button(
                        column![
                            text(truncate_label(&h.title, 60))
                                .size(13)
                                .style(iced::theme::Text::from(pal.accent)),
                            text(truncate_label(&h.url, 80))
                                .size(11)
                                .style(iced::theme::Text::from(pal.text_dim)),
                        ]
                        .spacing(1)
                    )
                    .on_press(Message::OpenUrl(h.url.clone()))
                    .padding([4, 8])
                    .width(Length::Fill)
                    .style(chrome_btn(
                        pal.content_bg,
                        pal.menu_hover,
                        pal.text,
                        [6.0; 4]
                    )),
                    button(text("✕").size(11))
                        .on_press(Message::RemoveHistoryItem(h.url.clone()))
                        .padding([4, 8])
                        .style(chrome_btn(
                            pal.content_bg,
                            pal.menu_hover,
                            pal.text_dim,
                            [6.0; 4]
                        )),
                ]
                .spacing(8)
                .align_items(iced::Alignment::Center),
            );
        }
        if shown == 0 {
            list = list.push(
                text("No browsing history found")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text_dim)),
            );
        }

        let page =
            container(column![header, search, scrollable(list).height(Length::Fill)].spacing(16))
                .padding([24, 60])
                .width(Length::Fill)
                .height(Length::Fill);

        page.into()
    }

    /// ghita://search — in-app web search results page
    fn build_search_page(&self, pal: &'static Pal) -> Element<'_, Message> {
        // Search state is tracked per tab, so each tab keeps its own query
        // and results (switching tabs never shows another tab's results).
        let tab_id = self.browser.tabs.active_tab_id().unwrap_or(0);
        let st = self.search_state.get(&tab_id);
        let query = st.map(|s| s.query.clone()).unwrap_or_default();
        let engine_name = self.search_engine_name().to_string();
        let engine_url = self.search_url(&query);
        let current_url = self
            .browser
            .active_tab()
            .map(|t| t.url.clone())
            .unwrap_or_default();

        let header = text(format!("Search results for \"{}\"", query))
            .size(24)
            .style(iced::theme::Text::from(pal.text));

        let mut body: Vec<Element<'_, Message>> = Vec::new();

        let Some(st) = st else {
            // No search was started in this tab yet (e.g. opened the page
            // directly); show a friendly hint instead of an empty page.
            body.push(
                text("Enter a search term in the address bar to get results.")
                    .size(14)
                    .style(iced::theme::Text::from(pal.text_dim))
                    .into(),
            );
            let mut page = column![header, vertical_space().height(8)].spacing(14);
            for el in body {
                page = page.push(el);
            }
            let footer =
                self.build_engine_link(pal, format!("More on {}", engine_name), engine_url);
            return container(column![page, footer].spacing(12))
                .padding([24, 60])
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        };

        if st.loading {
            body.push(
                text(format!("Searching for \"{}\"…", query))
                    .size(14)
                    .style(iced::theme::Text::from(pal.text_dim))
                    .into(),
            );
        } else if let Some(err) = &st.error {
            body.push(
                text(format!("Search failed: {}", err))
                    .size(14)
                    .style(iced::theme::Text::from(pal.danger))
                    .into(),
            );
            body.push(
                row![
                    button(text("Try again").size(12))
                        .on_press(Message::OpenUrl(current_url))
                        .padding([6, 14])
                        .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4])),
                    self.build_engine_link(
                        pal,
                        format!("Open results on {}", engine_name),
                        engine_url.clone()
                    ),
                ]
                .spacing(10)
                .into(),
            );
        } else if st.results.is_empty() {
            body.push(
                text(format!("No results found for \"{}\".", query))
                    .size(14)
                    .style(iced::theme::Text::from(pal.text_dim))
                    .into(),
            );
            body.push(self.build_engine_link(
                pal,
                format!("Search on {}", engine_name),
                engine_url.clone(),
            ));
        } else {
            let mut list = column![].spacing(8);
            for r in &st.results {
                let url = r.url.clone();
                list = list.push(
                    column![
                        button(text(truncate_label(&r.title, 90)).size(14))
                            .on_press(Message::OpenUrl(url))
                            .padding([4, 8])
                            .width(Length::Fill)
                            .style(chrome_btn(
                                pal.content_bg,
                                pal.menu_hover,
                                pal.text,
                                [6.0; 4]
                            )),
                        text(truncate_label(&r.url, 100))
                            .size(11)
                            .style(iced::theme::Text::from(pal.secure)),
                        text(truncate_label(&r.snippet, 240))
                            .size(12)
                            .style(iced::theme::Text::from(pal.text_dim)),
                    ]
                    .spacing(2),
                );
            }
            body.push(scrollable(list).height(Length::Fill).into());
        }

        let mut page = column![header, vertical_space().height(8)].spacing(14);
        for el in body {
            page = page.push(el);
        }

        let footer = self.build_engine_link(pal, format!("More on {}", engine_name), engine_url);

        container(column![page, footer].spacing(12))
            .padding([24, 60])
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Rounded pill button that opens the configured search engine for the query
    fn build_engine_link(
        &self,
        pal: &'static Pal,
        label: String,
        url: String,
    ) -> Element<'_, Message> {
        button(
            row![text("🔍").size(12), text(label).size(12),]
                .spacing(8)
                .align_items(iced::Alignment::Center),
        )
        .on_press(Message::OpenUrl(url))
        .padding([6, 14])
        .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4]))
        .into()
    }

    /// chrome://bookmarks equivalent
    fn build_bookmarks_page(&self, pal: &'static Pal) -> Element<'_, Message> {
        let header = text("Bookmarks")
            .size(24)
            .style(iced::theme::Text::from(pal.text));

        let mut list = column![].spacing(2);
        let bookmarks: Vec<(String, String, i64)> = self
            .browser
            .storage
            .bookmarks()
            .iter()
            .map(|b| (b.title.clone(), b.url.clone(), b.added_at))
            .collect();

        if bookmarks.is_empty() {
            list = list.push(
                text("No bookmarks yet — press ☆ in the address bar or Ctrl+D to add one")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text_dim)),
            );
        }

        for (title, url, added_at) in bookmarks {
            let url_open = url.clone();
            let url_remove = url.clone();
            list = list.push(
                row![
                    text("★")
                        .size(13)
                        .style(iced::theme::Text::from(pal.accent)),
                    button(
                        column![
                            text(truncate_label(&title, 60))
                                .size(13)
                                .style(iced::theme::Text::from(pal.accent)),
                            text(truncate_label(&url, 80))
                                .size(11)
                                .style(iced::theme::Text::from(pal.text_dim)),
                        ]
                        .spacing(1)
                    )
                    .on_press(Message::OpenUrl(url_open))
                    .padding([4, 8])
                    .width(Length::Fill)
                    .style(chrome_btn(
                        pal.content_bg,
                        pal.menu_hover,
                        pal.text,
                        [6.0; 4]
                    )),
                    text(fmt_timestamp(added_at))
                        .size(11)
                        .style(iced::theme::Text::from(pal.text_dim)),
                    button(text("✕").size(11))
                        .on_press(Message::RemoveBookmark(url_remove))
                        .padding([4, 8])
                        .style(chrome_btn(
                            pal.content_bg,
                            pal.menu_hover,
                            pal.text_dim,
                            [6.0; 4]
                        )),
                ]
                .spacing(8)
                .align_items(iced::Alignment::Center),
            );
        }

        container(column![header, scrollable(list).height(Length::Fill)].spacing(16))
            .padding([24, 60])
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// chrome://downloads equivalent
    fn build_downloads_page(&self, pal: &'static Pal) -> Element<'_, Message> {
        let header = row![
            text("Downloads")
                .size(24)
                .style(iced::theme::Text::from(pal.text)),
            horizontal_space(),
            button(text("Clear all").size(12))
                .on_press(Message::ClearDownloads)
                .padding([6, 14])
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [6.0; 4])),
        ]
        .align_items(iced::Alignment::Center);

        let hint = text("Tip: use Menu (⋮) → \"Save page as...\" to download the current page")
            .size(11)
            .style(iced::theme::Text::from(pal.text_dim));

        let mut list = column![].spacing(6);
        let downloads: Vec<crate::storage::DownloadRecord> =
            self.browser.storage.downloads().to_vec();

        if downloads.is_empty() {
            list = list.push(
                text("Files you download appear here")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text_dim)),
            );
        }

        for d in downloads {
            let status: Element<'_, Message> = if d.success {
                text("✓")
                    .size(14)
                    .style(iced::theme::Text::from(pal.secure))
                    .into()
            } else {
                text("✗")
                    .size(14)
                    .style(iced::theme::Text::from(pal.danger))
                    .into()
            };
            list = list.push(
                container(
                    row![
                        text("📄").size(18),
                        column![
                            text(truncate_label(&d.file_name, 50))
                                .size(13)
                                .style(iced::theme::Text::from(pal.text)),
                            text(truncate_label(&d.url, 70))
                                .size(11)
                                .style(iced::theme::Text::from(pal.text_dim)),
                            text(truncate_label(&d.path, 70))
                                .size(10)
                                .style(iced::theme::Text::from(pal.text_dim)),
                        ]
                        .spacing(1)
                        .width(Length::Fill),
                        column![
                            text(fmt_bytes(d.size_bytes))
                                .size(11)
                                .style(iced::theme::Text::from(pal.text_dim)),
                            text(fmt_timestamp(d.completed_at))
                                .size(11)
                                .style(iced::theme::Text::from(pal.text_dim)),
                        ]
                        .spacing(1),
                        status,
                    ]
                    .spacing(12)
                    .align_items(iced::Alignment::Center),
                )
                .style(move |_: &Theme| container::Appearance {
                    background: Some(iced::Background::Color(pal.menu_bg)),
                    border: iced::Border {
                        color: pal.divider,
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                })
                .padding(12)
                .width(Length::Fill),
            );
        }

        container(column![header, hint, scrollable(list).height(Length::Fill)].spacing(14))
            .padding([24, 60])
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// chrome://settings equivalent
    fn build_settings_page(&self, pal: &'static Pal) -> Element<'_, Message> {
        let settings = &self.browser.storage.settings;

        let section = |label: &str| -> Element<'_, Message> {
            text(label.to_string())
                .size(16)
                .style(iced::theme::Text::from(pal.accent))
                .into()
        };

        let choice_btn = |label: &str, selected: bool, msg: Message| -> Element<'_, Message> {
            button(
                row![
                    text(if selected { "●" } else { "○" })
                        .size(11)
                        .style(iced::theme::Text::from(if selected {
                            pal.accent
                        } else {
                            pal.text_dim
                        })),
                    text(label.to_string())
                        .size(13)
                        .style(iced::theme::Text::from(pal.text)),
                ]
                .spacing(8)
                .align_items(iced::Alignment::Center),
            )
            .on_press(msg)
            .padding([6, 14])
            .style(if selected {
                chrome_btn(pal.menu_hover, pal.menu_hover, pal.text, [16.0; 4])
            } else {
                chrome_btn(pal.content_bg, pal.menu_hover, pal.text, [16.0; 4])
            })
            .into()
        };

        let engine = settings.search_engine.clone();
        let pixel_on = settings.pixel_rendering;
        let dark = self.is_dark_theme;
        let bar_on = self.show_bookmarks_bar;

        let page = column![
            text("Settings")
                .size(24)
                .style(iced::theme::Text::from(pal.text)),
            section("Appearance"),
            row![
                text("Theme")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text))
                    .width(Length::Fixed(180.0)),
                choice_btn("Dark", dark, Message::SetThemeDark(true)),
                choice_btn("Light", !dark, Message::SetThemeDark(false)),
            ]
            .spacing(10)
            .align_items(iced::Alignment::Center),
            row![
                text("Show bookmarks bar")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text))
                    .width(Length::Fixed(180.0)),
                choice_btn(
                    if bar_on { "On" } else { "Off" },
                    bar_on,
                    Message::ToggleBookmarksBar
                ),
            ]
            .spacing(10)
            .align_items(iced::Alignment::Center),
            section("Search engine"),
            row![
                text("Used in the omnibox")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text))
                    .width(Length::Fixed(180.0)),
                choice_btn(
                    "Google",
                    engine == "google",
                    Message::SetSearchEngine("google".to_string())
                ),
                choice_btn(
                    "Bing",
                    engine == "bing",
                    Message::SetSearchEngine("bing".to_string())
                ),
                choice_btn(
                    "DuckDuckGo",
                    engine == "duckduckgo",
                    Message::SetSearchEngine("duckduckgo".to_string())
                ),
            ]
            .spacing(10)
            .align_items(iced::Alignment::Center),
            section("On startup"),
            row![
                text("Homepage")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text))
                    .width(Length::Fixed(180.0)),
                text_input("ghita://newtab", &self.homepage_input)
                    .on_input(Message::HomepageChanged)
                    .size(13)
                    .padding([6, 12])
                    .width(Length::Fixed(340.0))
                    .style(omnibox_style(pal, 8.0)),
            ]
            .spacing(10)
            .align_items(iced::Alignment::Center),
            section("Page zoom"),
            row![
                text("Default zoom")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text))
                    .width(Length::Fixed(180.0)),
                button(text("−").size(13))
                    .on_press(Message::ZoomOut)
                    .padding([4, 12])
                    .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [6.0; 4])),
                text(format!("{}%", self.zoom_percent))
                    .size(13)
                    .style(iced::theme::Text::from(pal.text)),
                button(text("+").size(13))
                    .on_press(Message::ZoomIn)
                    .padding([4, 12])
                    .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [6.0; 4])),
                button(text("Reset").size(12))
                    .on_press(Message::ZoomReset)
                    .padding([4, 12])
                    .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [6.0; 4])),
            ]
            .spacing(10)
            .align_items(iced::Alignment::Center),
            section("Renderer"),
            row![
                text("Web page rendering")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text))
                    .width(Length::Fixed(180.0)),
                choice_btn(
                    "Pixels (Chrome-like)",
                    pixel_on,
                    Message::SetPixelRendering(true)
                ),
                choice_btn(
                    "Text mode (legacy)",
                    !pixel_on,
                    Message::SetPixelRendering(false)
                ),
            ]
            .spacing(10)
            .align_items(iced::Alignment::Center),
            text("Pixel mode paints pages on a real graphics canvas with clickable links")
                .size(11)
                .style(iced::theme::Text::from(pal.text_dim)),
            section("Privacy and security"),
            row![
                button(text("Clear browsing data").size(12))
                    .on_press(Message::ClearBrowsingData)
                    .padding([6, 14])
                    .style(chrome_btn(pal.danger, pal.danger, pal.on_accent, [6.0; 4])),
                button(text("Clear downloads list").size(12))
                    .on_press(Message::ClearDownloads)
                    .padding([6, 14])
                    .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [6.0; 4])),
            ]
            .spacing(10),
            text("Clears history, cookies and the resource cache")
                .size(11)
                .style(iced::theme::Text::from(pal.text_dim)),
            section("About"),
            text(format!(
                "GhitaBrowser v{} — a Chrome-style browser written 100% in safe Rust",
                crate::VERSION
            ))
            .size(12)
            .style(iced::theme::Text::from(pal.text_dim)),
        ]
        .spacing(14)
        .max_width(720);

        scrollable(container(page).padding([24, 60]).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// chrome://about equivalent
    fn build_about_page(&self, pal: &'static Pal) -> Element<'_, Message> {
        let page = column![
            vertical_space().height(60),
            text("🦀").size(56),
            vertical_space().height(10),
            row![
                text("G").size(34).style(iced::theme::Text::from(GH_ORANGE)),
                text("h").size(34).style(iced::theme::Text::from(GH_AMBER)),
                text("i").size(34).style(iced::theme::Text::from(GH_RED)),
                text("t")
                    .size(34)
                    .style(iced::theme::Text::from(GH_CRIMSON)),
                text("a").size(34).style(iced::theme::Text::from(GH_EMBER)),
                text("Browser")
                    .size(34)
                    .style(iced::theme::Text::from(pal.text)),
            ]
            .spacing(1),
            vertical_space().height(8),
            text(format!(
                "Version {} (Official Build) — 100% safe Rust",
                crate::VERSION
            ))
            .size(14)
            .style(iced::theme::Text::from(pal.text_dim)),
            vertical_space().height(24),
            column![
                text("• Real pixel graphics renderer with clickable links (canvas)")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
                text("• Custom HTML5 parser, CSS engine & box-model layout")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
                text("• Real HTTP/HTTPS networking with cookies & resource cache")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
                text("• Homemade JavaScript engine (JSv)")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
                text("• Chrome-style UI: tabs, omnibox, bookmarks, history, downloads")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
                text("• GUI: Iced 0.12 (Elm architecture) on tokio")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
            ]
            .spacing(4)
            .align_items(iced::Alignment::Center),
            vertical_space().height(20),
            text("MIT License")
                .size(11)
                .style(iced::theme::Text::from(pal.text_dim)),
        ]
        .align_items(iced::Alignment::Center)
        .width(Length::Fill);

        scrollable(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    // ===== DevTools =====

    fn build_devtools_panel(&self, pal: &'static Pal) -> Element<'_, Message> {
        let pane_btn = |label: &str, pane: DevPane| -> Element<'_, Message> {
            let selected = self.dev_pane == pane;
            button(text(label.to_string()).size(12))
                .on_press(Message::SetDevPane(pane))
                .padding([5, 12])
                .width(Length::Fill)
                .style(if selected {
                    chrome_btn(pal.menu_hover, pal.menu_hover, pal.accent, [6.0; 4])
                } else {
                    chrome_btn(pal.menu_bg, pal.menu_hover, pal.text, [6.0; 4])
                })
                .into()
        };

        let pane_content: Element<'_, Message> = match self.dev_pane {
            DevPane::Console => self.build_console_pane(pal),
            DevPane::Storage => self.build_storage_pane(pal),
            DevPane::Cache => self.build_cache_pane(pal),
        };

        container(
            column![
                row![
                    text("DevTools")
                        .size(14)
                        .style(iced::theme::Text::from(pal.text)),
                    horizontal_space(),
                    button(text("✕").size(12))
                        .on_press(Message::ToggleDevTools)
                        .padding([2, 8])
                        .style(chrome_btn(
                            pal.menu_bg,
                            pal.menu_hover,
                            pal.text_dim,
                            [4.0; 4]
                        )),
                ]
                .align_items(iced::Alignment::Center),
                pane_btn("Console", DevPane::Console),
                pane_btn("Storage", DevPane::Storage),
                pane_btn("Cache", DevPane::Cache),
                container(pane_content).height(Length::Fill),
            ]
            .spacing(8),
        )
        .style(move |_: &Theme| container::Appearance {
            background: Some(iced::Background::Color(pal.menu_bg)),
            border: iced::Border {
                color: pal.divider,
                width: 1.0,
                radius: 0.0.into(),
            },
            text_color: Some(pal.text),
            ..Default::default()
        })
        .padding(12)
        .width(Length::Fixed(320.0))
        .height(Length::Fill)
        .into()
    }

    fn build_console_pane(&self, pal: &'static Pal) -> Element<'_, Message> {
        column![
            scrollable(
                text(&self.js_console_text)
                    .size(11)
                    .style(iced::theme::Text::from(pal.text))
            )
            .width(Length::Fill)
            .height(Length::Fill),
            row![
                text_input("Run JavaScript...", &self.js_input_text)
                    .on_input(Message::JsCodeChanged)
                    .on_submit(Message::ExecuteJs)
                    .size(12)
                    .padding(6)
                    .width(Length::Fill)
                    .style(omnibox_style(pal, 6.0)),
                button(text("▶").size(12))
                    .on_press(Message::ExecuteJs)
                    .padding([5, 10])
                    .style(chrome_btn(pal.accent, pal.accent, pal.on_accent, [6.0; 4])),
            ]
            .spacing(4),
        ]
        .spacing(8)
        .into()
    }

    fn build_storage_pane(&self, pal: &'static Pal) -> Element<'_, Message> {
        let mut details = column![
            text(format!("Cookies: {}", self.browser.storage.cookie_count()))
                .size(12)
                .style(iced::theme::Text::from(pal.text)),
            text(format!(
                "localStorage origins: {}",
                self.browser.storage.local_storage_origins().len()
            ))
            .size(12)
            .style(iced::theme::Text::from(pal.text)),
            text(format!(
                "Bookmarks: {}",
                self.browser.storage.bookmarks().len()
            ))
            .size(12)
            .style(iced::theme::Text::from(pal.text)),
            text(format!(
                "History entries: {}",
                self.browser.storage.history().len()
            ))
            .size(12)
            .style(iced::theme::Text::from(pal.text)),
        ]
        .spacing(6);

        for origin in self.browser.storage.local_storage_origins() {
            if let Some(ls) = self.browser.storage.get_local_storage(&origin) {
                details = details.push(
                    text(format!("  {} ({} items)", origin, ls.len()))
                        .size(11)
                        .style(iced::theme::Text::from(pal.text_dim)),
                );
            }
        }

        scrollable(details)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn build_cache_pane(&self, pal: &'static Pal) -> Element<'_, Message> {
        let stats = self.browser.cache.stats();
        let hit_rate = if stats.hits + stats.misses > 0 {
            format!(
                "{:.1}%",
                (stats.hits as f64 / (stats.hits + stats.misses) as f64) * 100.0
            )
        } else {
            "0.0%".to_string()
        };

        column![
            text(format!("Entries: {}", stats.entries))
                .size(12)
                .style(iced::theme::Text::from(pal.text)),
            text(format!("Hits: {}", stats.hits))
                .size(12)
                .style(iced::theme::Text::from(pal.secure)),
            text(format!("Misses: {}", stats.misses))
                .size(12)
                .style(iced::theme::Text::from(pal.danger)),
            text(format!("Hit rate: {}", hit_rate))
                .size(12)
                .style(iced::theme::Text::from(pal.text)),
            text(format!("Max size: {} MB", stats.max_size / (1024 * 1024)))
                .size(12)
                .style(iced::theme::Text::from(pal.text_dim)),
        ]
        .spacing(6)
        .into()
    }

    /// Slim status bar (Chrome shows hover status bottom-left)
    fn build_status_bar(&self, pal: &'static Pal) -> Element<'_, Message> {
        let load_time_str = self
            .last_load_time
            .map(|t| format!("{}ms", t))
            .unwrap_or_else(|| "—".to_string());

        let status_row = row![
            text(truncate_label(&self.status_msg, 90))
                .size(10)
                .style(iced::theme::Text::from(pal.text_dim)),
            horizontal_space(),
            text(if self.zoom_percent != 100 {
                format!("{}% ", self.zoom_percent)
            } else {
                String::new()
            })
            .size(10)
            .style(iced::theme::Text::from(pal.accent)),
            text(format!(
                "{} | {} tabs | v{} 🦀",
                load_time_str,
                self.browser.tab_count(),
                crate::VERSION
            ))
            .size(10)
            .style(iced::theme::Text::from(pal.text_dim)),
        ]
        .spacing(6)
        .padding([2, 10])
        .align_items(iced::Alignment::Center);

        container(status_row)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.toolbar)),
                border: iced::Border {
                    color: pal.divider,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .into()
    }
}

// ===== Formatting helpers =====

/// Truncate a label on a char boundary, appending an ellipsis
fn truncate_label(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

/// Human-readable byte size
fn fmt_bytes(n: u64) -> String {
    if n >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{} B", n)
    }
}

/// Local time formatting for history/downloads rows
fn fmt_timestamp(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%H:%M · %d/%m/%Y").to_string())
        .unwrap_or_else(|| "—".to_string())
}

pub fn run_gui() -> Result<(), iced::Error> {
    let mut settings = Settings::default();
    settings.window.size = iced::Size::new(1280.0, 900.0);
    settings.window.min_size = Some(iced::Size::new(800.0, 600.0));
    info!("Starting GhitaBrowser v{} GUI", crate::VERSION);
    GhitaBrowserApp::run(settings)
}

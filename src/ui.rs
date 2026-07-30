// src/ui.rs - Modern GUI Browser with real engine integration (v0.0.2)
#![allow(dead_code)]

use iced::widget::{button, column, container, horizontal_space, row, scrollable, text, text_input, vertical_space};
use iced::{Application, Command, Element, Length, Settings, Theme, Color};
use log::info;
use std::time::Instant;

use crate::Browser;
use crate::parser::parse_html;

/// Main application state - connected to real Browser engine
pub struct GhitaBrowserApp {
    /// The core browser engine
    browser: Browser,
    
    // UI state
    url_input: String,
    rendered_content: String,
    page_heading: String,
    status_msg: String,
    render_stats_text: String,
    is_loading: bool,
    last_load_time: u64,
    
    // View state
    show_storage: bool,
    show_cache: bool,
    show_js_console: bool,
    js_console_text: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Navigation
    UrlChanged(String),
    Navigate,
    GoBack,
    GoForward,
    Reload,
    
    // Tabs
    SelectTab(usize),
    NewTab,
    CloseTab(usize),
    
    // View toggles
    ToggleStorage,
    ToggleCache,
    ToggleJsConsole,
    
    // JS Console
    JsCodeChanged(String),
    ExecuteJs,
    
    // Internal
    PageLoaded(String),
    LoadError(String),
}

impl Application for GhitaBrowserApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let mut browser = Browser::new();
        
        // Load a default page
        let welcome_html = String::from(
            "<html>
            <head><title>GhitaBrowser v0.0.2</title></head>
            <body>
                <h1>🚀 GhitaBrowser v0.0.2</h1>
                <p>Welcome to the next-generation Rust browser!</p>
                <p>Built from scratch in safe Rust with:</p>
                <ul>
                    <li>Real HTTP/HTTPS networking via <strong>ureq</strong></li>
                    <li>Custom HTML5 parser with error recovery</li>
                    <li>Advanced CSS selector engine with specificity</li>
                    <li>Layout engine with text wrapping</li>
                    <li>JavaScript evaluator with variables & functions</li>
                    <li>Persistent cookie & localStorage storage</li>
                    <li>Iced GUI with multi-tab support</li>
                </ul>
                <p>Enter a URL above and click Go to start browsing!</p>
            </body>
            </html>"
        );
        
        browser.load_html("https://ghitabrowser.local", &welcome_html);
        
        let rendered = browser.render_current();
        let status = browser.status_string();
        let title = browser.active_tab()
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "GhitaBrowser".to_string());

        (
            Self {
                browser,
                url_input: "https://example.com".to_string(),
                rendered_content: rendered,
                page_heading: format!("Welcome to {}", title),
                status_msg: status,
                render_stats_text: String::new(),
                is_loading: false,
                last_load_time: 0,
                show_storage: false,
                show_cache: false,
                show_js_console: false,
                js_console_text: String::new(),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        self.browser.active_tab()
            .map(|t| format!("GhitaBrowser v0.0.2 - {}", t.title))
            .unwrap_or_else(|| "GhitaBrowser v0.0.2".to_string())
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::UrlChanged(url) => {
                self.url_input = url;
            }
            Message::Navigate => {
                let url = self.url_input.trim().to_string();
                if url.is_empty() {
                    return Command::none();
                }
                
                // Add http:// if missing
                let url = if !url.starts_with("http://") && !url.starts_with("https://") {
                    format!("https://{}", url)
                } else {
                    url.clone()
                };
                
                self.is_loading = true;
                self.status_msg = format!("🔃 Loading {}...", url);
                self.url_input = url.clone();
                
                let start = Instant::now();
                
                match self.browser.load_url(&url) {
                    Ok(rendered) => {
                        let elapsed = start.elapsed().as_millis() as u64;
                        self.last_load_time = elapsed;
                        self.rendered_content = rendered;
                        self.is_loading = false;
                        
                        if let Some(stats) = &self.browser.last_render_stats {
                            self.render_stats_text = format!(
                                "Parse: {}ms | Style: {}ms | Layout: {}ms | Render: {}ms | Total: {}ms",
                                stats.parse_time_ms, stats.style_time_ms,
                                stats.layout_time_ms, stats.render_time_ms,
                                stats.total_time_ms
                            );
                        }
                        
                        let title = self.browser.active_tab()
                            .map(|t| t.title.clone())
                            .unwrap_or_else(|| url.clone());
                        self.page_heading = format!("📄 {}", title);
                        self.status_msg = self.browser.status_string();
                    }
                    Err(e) => {
                        self.is_loading = false;
                        self.status_msg = format!("❌ Error: {}", e);
                        
                        // Generate a nice error page
                        let error_page = format!(
                            "\n\
                             ╔══════════════════════════════════════╗\n\
                             ║        ⚠️  Page Load Error           ║\n\
                             ╚══════════════════════════════════════╝\n\n\
                             ┌─ Error Details ─────────────────────┐\n\
                             │ {}\n\
                             └──────────────────────────────────────┘\n\n\
                             ┌─ Suggestions ───────────────────────┐\n\
                             │ • Check the URL for typos            │\n\
                             │ • Make sure you're connected to the  │\n\
                             │   internet                           │\n\
                             │ • Try again with a different URL     │\n\
                             │ • The site might be down             │\n\
                             └──────────────────────────────────────┘\n\n\
                             Press Reload ⟳ or enter a new URL above.\n",
                            e
                        );
                        self.rendered_content = error_page;
                    }
                }
            }
            Message::GoBack => {
                self.browser.go_back();
                if let Some(tab) = self.browser.active_tab() {
                    self.url_input = tab.url.clone();
                    self.page_heading = format!("📄 {}", tab.title);
                    self.rendered_content = self.browser.render_current();
                    self.status_msg = "⬅ Navigated back".to_string();
                }
            }
            Message::GoForward => {
                self.browser.go_forward();
                if let Some(tab) = self.browser.active_tab() {
                    self.url_input = tab.url.clone();
                    self.page_heading = format!("📄 {}", tab.title);
                    self.rendered_content = self.browser.render_current();
                    self.status_msg = "➡ Navigated forward".to_string();
                }
            }
            Message::Reload => {
                if let Some(tab) = self.browser.active_tab() {
                    let url = tab.url.clone();
                    self.url_input = url.clone();
                    self.is_loading = true;
                    
                    let start = Instant::now();
                    match self.browser.load_url(&url) {
                        Ok(rendered) => {
                            self.last_load_time = start.elapsed().as_millis() as u64;
                            self.rendered_content = rendered;
                            self.is_loading = false;
                            self.status_msg = "🔄 Reloaded".to_string();
                        }
                        Err(e) => {
                            self.is_loading = false;
                            self.status_msg = format!("❌ Reload error: {}", e);
                            self.rendered_content = format!(
                                "\n╔══ Reload Failed ══╗\n║ {}\n╚══════════════════╝\n", e
                            );
                        }
                    }
                }
            }
            Message::SelectTab(index) => {
                // Find the Nth tab and switch to it
                if let Some(tab) = self.browser.tabs.get_tab_by_index(index) {
                    let id = tab.id;
                    self.browser.tabs.set_active_tab(id);
                    if let Some(active) = self.browser.active_tab() {
                        self.url_input = active.url.clone();
                        self.page_heading = format!("📄 {}", active.title);
                        self.rendered_content = self.browser.render_current();
                        self.status_msg = format!("Tab: {}", active.title);
                    }
                }
            }
            Message::NewTab => {
                let html = String::from(
                    "<html><head><title>New Tab</title></head>
                    <body><h1>New Tab</h1><p>Enter a URL and press Go.</p></body></html>"
                );
                let dom = parse_html(&html);
                self.browser.add_tab("about:blank", dom, "New Tab");
                
                if let Some(active) = self.browser.active_tab() {
                    self.url_input = active.url.clone();
                    self.page_heading = "📄 New Tab".to_string();
                    self.rendered_content = self.browser.render_current();
                    self.status_msg = format!("📑 New tab ({} tabs)", self.browser.tab_count());
                }
            }
            Message::CloseTab(index) => {
                // Get the tab ID at this index
                if let Some(tab) = self.browser.tabs.get_tab_by_index(index) {
                    let id = tab.id;
                    self.browser.tabs.remove_tab(id);
                    
                    if let Some(active) = self.browser.active_tab() {
                        self.url_input = active.url.clone();
                        self.page_heading = format!("📄 {}", active.title);
                        self.rendered_content = self.browser.render_current();
                    } else {
                        self.rendered_content = "No open tabs".to_string();
                        self.page_heading = "GhitaBrowser".to_string();
                        self.url_input = String::new();
                    }
                    self.status_msg = format!("Tab closed ({} remaining)", self.browser.tab_count());
                }
            }
            Message::ToggleStorage => {
                self.show_storage = !self.show_storage;
                if self.show_storage {
                    let mut s = String::new();
                    s.push_str("=== Cookies ===\n");
                    let _origins: Vec<String> = vec![]; // Would iterate cookies
                    s.push_str(&format!("Total cookies: {}\n", self.browser.storage.cookie_count()));
                    s.push_str(&format!("localStorage origins: {}\n", self.browser.storage.local_storage_origins().len()));
                    for origin in self.browser.storage.local_storage_origins() {
                        if let Some(ls) = self.browser.storage.get_local_storage(&origin) {
                            s.push_str(&format!("  {} ({} items)\n", origin, ls.len()));
                        }
                    }
                    self.rendered_content = s;
                } else {
                    self.rendered_content = self.browser.render_current();
                }
            }
            Message::ToggleCache => {
                self.show_cache = !self.show_cache;
                if self.show_cache {
                    let stats = self.browser.cache.stats();
                    self.rendered_content = format!(
                        "=== Resource Cache ===\n\
                         Entries: {}\n\
                         Hits: {}\n\
                         Misses: {}\n\
                         Hit rate: {:.1}%\n\
                         Max size: {} MB\n",
                        stats.entries, stats.hits, stats.misses,
                        if stats.hits + stats.misses > 0 {
                            (stats.hits as f64 / (stats.hits + stats.misses) as f64) * 100.0
                        } else { 0.0 },
                        stats.max_size / (1024 * 1024)
                    );
                } else {
                    self.rendered_content = self.browser.render_current();
                }
            }
            Message::ToggleJsConsole => {
                self.show_js_console = !self.show_js_console;
                if self.show_js_console {
                    self.js_console_text = self.browser.js_engine.console_output.join("\n");
                    self.status_msg = "JS Console opened (shown below page)".to_string();
                } else {
                    self.status_msg = "JS Console closed".to_string();
                }
            }
            Message::JsCodeChanged(code) => {
                self.js_console_text = code;
            }
            Message::ExecuteJs => {
                let code = self.js_console_text.clone();
                if !code.is_empty() {
                    match self.browser.js_engine.execute_script(&code) {
                        Ok(val) => {
                            let output = val.to_display_string();
                            self.browser.js_engine.console_output.push(
                                format!("> {} = {}", code, output)
                            );
                            self.js_console_text = self.browser.js_engine.console_output.join("\n");
                            self.status_msg = format!("JS: {} = {}", code, output);
                        }
                        Err(e) => {
                            self.browser.js_engine.console_output.push(
                                format!("> {}  // Error: {}", code, e)
                            );
                            self.js_console_text = self.browser.js_engine.console_output.join("\n");
                            self.status_msg = format!("JS Error: {}", e);
                        }
                    }
                }
            }
            Message::PageLoaded(content) => {
                self.rendered_content = content;
                self.is_loading = false;
            }
            Message::LoadError(err) => {
                self.status_msg = format!("❌ {}", err);
                self.is_loading = false;
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        // ===== Tab Bar =====
        let mut tab_row = row![].spacing(2).padding(2);
        
        // Collect tab IDs and info
        let tab_info: Vec<(usize, String, String)> = self.browser.tabs.iter_tabs()
            .into_iter().map(|t| (t.id, t.title.clone(), t.url.clone()))
            .collect();
        
        let active_id = self.browser.tabs.active_tab_id();
        
        for (i, (id, title, _url)) in tab_info.iter().enumerate() {
            let is_active = Some(*id) == active_id;
            let tab_style = if is_active {
                format!("  ⦿ {}  ", title)
            } else {
                format!("   {}   ", title)
            };
            
            let tab_btn = button(text(tab_style).size(12))
                .on_press(Message::SelectTab(i))
                .style(if is_active {
                    iced::theme::Button::Primary
                } else {
                    iced::theme::Button::Text
                });
            
            tab_row = tab_row.push(tab_btn);
            
            // Close button (except for last tab)
            if tab_info.len() > 1 {
                let close_btn = button(text("✕").size(10))
                    .on_press(Message::CloseTab(i))
                    .style(iced::theme::Button::Destructive);
                tab_row = tab_row.push(close_btn);
            }
        }
        
        let new_tab_btn = button(text(" ＋ ").size(13))
            .on_press(Message::NewTab);
        tab_row = tab_row.push(new_tab_btn);

        // ===== Navigation Toolbar =====
        let loading_indicator = if self.is_loading {
            text(" ⟳ ").size(16)
        } else {
            text("   ").size(16)
        };
        
        let can_go_back = self.browser.active_tab()
            .map(|t| t.can_go_back())
            .unwrap_or(false);
        let can_go_forward = self.browser.active_tab()
            .map(|t| t.can_go_forward())
            .unwrap_or(false);
        
        // HTTPS padlock indicator
        let is_https = self.url_input.starts_with("https://");
        let padlock = if is_https {
            text(" 🔒 ").size(14).style(Color::from_rgb(0.4, 0.8, 0.4))
        } else if self.url_input.starts_with("http://") {
            text(" ⚠ ").size(14).style(Color::from_rgb(0.9, 0.6, 0.1))
        } else {
            text("   ").size(14)
        };
        
        let nav_bar = row![
            button(text(" ◀ ").size(14))
                .on_press_maybe(if can_go_back { Some(Message::GoBack) } else { None }),
            button(text(" ▶ ").size(14))
                .on_press_maybe(if can_go_forward { Some(Message::GoForward) } else { None }),
            button(text(" ⟳ ").size(14)).on_press(Message::Reload),
            loading_indicator,
            padlock,
            text_input("Enter URL and press Go...", &self.url_input)
                .on_input(Message::UrlChanged)
                .on_submit(Message::Navigate)
                .padding(6)
                .width(Length::Fill),
            button(text(" Go ").size(14))
                .on_press(Message::Navigate)
                .style(iced::theme::Button::Primary),
        ]
        .spacing(4)
        .padding(4);

        // ===== Web Page Viewport =====
        let mut web_content = column![
            text(&self.page_heading).size(22),
            vertical_space().height(8),
        ]
        .spacing(4);

        // Loading progress bar
        if self.is_loading {
            web_content = web_content.push(
                text("⏳ Loading... Please wait...").size(14).style(Color::from_rgb(0.4, 0.8, 0.4))
            );
            web_content = web_content.push(
                vertical_space().height(4)
            );
        }

        // Page content (always visible unless dev tools override)
        web_content = web_content.push(
            text(&self.rendered_content).size(13)
        );

        // Render stats
        if !self.render_stats_text.is_empty() {
            web_content = web_content.push(
                vertical_space().height(8)
            );
            web_content = web_content.push(
                text(&self.render_stats_text).size(11).style(Color::from_rgb(0.4, 0.8, 0.4))
            );
        }

        // JS Console panel (shown below content, not replacing it)
        if self.show_js_console {
            web_content = web_content.push(
                vertical_space().height(12)
            );
            web_content = web_content.push(
                text("─ JS Console ─────────────────────").size(12)
                    .style(Color::from_rgb(0.9, 0.7, 0.1))
            );
            web_content = web_content.push(
                text(&self.js_console_text).size(12)
            );
            web_content = web_content.push(
                vertical_space().height(4)
            );
            web_content = web_content.push(
                text_input("Enter JavaScript code...", &self.js_console_text)
                    .on_input(Message::JsCodeChanged)
                    .on_submit(Message::ExecuteJs)
                    .padding(6)
                    .width(Length::Fill)
            );
            web_content = web_content.push(
                button(text(" ▶ Execute JS ").size(13))
                    .on_press(Message::ExecuteJs)
            );
        }
        
        // Dev tools override (storage/cache views - keeps replacing for now but could be improved)
        if self.show_storage || self.show_cache {
            web_content = column![
                text(&self.page_heading).size(22),
                vertical_space().height(8),
                text(&self.rendered_content).size(13),
            ].spacing(4);
        }

        let viewport = container(
            scrollable(web_content)
                .width(Length::Fill)
                .height(Length::Fill)
        )
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill);

        // ===== Status Bar =====
        let loading_dot = if self.is_loading { "● " } else { "" };
        let status_bar = container(
            row![
                text(format!("{}{}", loading_dot, self.status_msg)).size(11),
                horizontal_space().width(Length::Fill),
                // Quick action buttons
                button(text("🍪").size(11)).on_press(Message::ToggleStorage),
                button(text("📦").size(11)).on_press(Message::ToggleCache),
                button(text("▶ JS").size(11)).on_press(Message::ToggleJsConsole),
                text(" ").size(11),
                text(format!("v0.0.2 | {} tabs", self.browser.tab_count())).size(11),
            ]
            .spacing(4)
            .padding(4)
        );

        column![tab_row, nav_bar, viewport, status_bar].into()
    }
}

pub fn run_gui() -> Result<(), iced::Error> {
    let mut settings = Settings::default();
    settings.window.size = iced::Size::new(1200.0, 850.0);
    settings.window.min_size = Some(iced::Size::new(800.0, 600.0));
    info!("Starting GhitaBrowser v0.0.2 GUI");
    GhitaBrowserApp::run(settings)
}

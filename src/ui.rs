// src/ui.rs - Modern GUI Browser with real engine integration (v0.1.5)
#![allow(dead_code)]

use iced::widget::{button, column, container, horizontal_space, row, scrollable, text, text_input, vertical_space};
use iced::{Application, Command, Element, Length, Settings, Theme, Color, keyboard, Shadow};
use log::info;
use std::time::Instant;

use crate::Browser;
use crate::parser::parse_html;

// ===== Brand Colors =====
const BRAND_NAVY: Color = Color::from_rgb(0.10, 0.10, 0.18);
const BRAND_NAVY_LIGHT: Color = Color::from_rgb(0.14, 0.14, 0.26);
const BRAND_ORANGE: Color = Color::from_rgb(0.95, 0.55, 0.20);
const BRAND_ORANGE_DARK: Color = Color::from_rgb(0.80, 0.42, 0.12);
const BRAND_GREEN: Color = Color::from_rgb(0.30, 0.80, 0.40);
const BRAND_RED: Color = Color::from_rgb(0.90, 0.30, 0.25);
const BRAND_YELLOW: Color = Color::from_rgb(0.95, 0.75, 0.10);
const BRAND_TEXT: Color = Color::from_rgb(0.88, 0.88, 0.92);
const BRAND_TEXT_DIM: Color = Color::from_rgb(0.55, 0.55, 0.65);
const BRAND_BORDER: Color = Color::from_rgb(0.25, 0.25, 0.40);

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
    last_load_time: Option<u64>,
    load_start_time: Option<Instant>,
    
    // View state
    show_storage: bool,
    show_cache: bool,
    show_js_console: bool,
    js_console_text: String,
    js_input_text: String,
    show_devtools: bool,
    
    // Theme
    is_dark_theme: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Navigation
    UrlChanged(String),
    Navigate,
    GoBack,
    GoForward,
    Reload,
    Home,
    ClearUrl,
    
    // Tabs
    SelectTab(usize),
    NewTab,
    CloseTab(usize),
    CloseCurrentTab,
    
    // Theme
    ToggleTheme,
    
    // View toggles
    ToggleDevTools,
    CloseDevTools,
    ToggleStorage,
    ToggleCache,
    ToggleJsConsole,
    
    // JS Console
    JsCodeChanged(String),
    ExecuteJs,
    
    // Keyboard
    FocusUrl,
    
    // Internal
    PageLoaded { html: String, url: String, fetch_time: u64 },
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
            "<html>\n            <head><title>GhitaBrowser v0.1.5</title></head>\n            <body>\n                <h1>🚀 GhitaBrowser v0.1.5</h1>\n                <p>Welcome to the next-generation Rust browser!</p>\n                <p>Built from scratch in safe Rust with:</p>\n                <ul>\n                    <li>Real HTTP/HTTPS networking via <strong>ureq</strong></li>\n                    <li>Custom HTML5 parser with error recovery</li>\n                    <li>Advanced CSS selector engine with specificity</li>\n                    <li>Layout engine with text wrapping</li>\n                    <li>JavaScript evaluator with variables & functions</li>\n                    <li>Persistent cookie & localStorage storage</li>\n                    <li>Iced GUI with multi-tab support</li>\n                </ul>\n                <p>Enter a URL above and click Go to start browsing!</p>\n            </body>\n            </html>"
        );
        
        let _ = browser.load_html("https://ghitabrowser.local", &welcome_html);
        
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
                last_load_time: None,
                load_start_time: None,
                show_storage: false,
                show_cache: false,
                show_js_console: false,
                js_console_text: String::new(),
                js_input_text: String::new(),
                show_devtools: false,
                is_dark_theme: true,
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        self.browser.active_tab()
            .map(|t| format!("GhitaBrowser v0.1.5 - {}", t.title))
            .unwrap_or_else(|| "GhitaBrowser v0.1.5".to_string())
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
                self.url_input = url;
            }
            Message::ClearUrl => {
                self.url_input = String::new();
            }
            Message::Navigate => {
                let url = self.url_input.trim().to_string();
                if url.is_empty() {
                    return Command::none();
                }

                // Handle special URLs synchronously
                if url == "about:blank" {
                    let blank_html = String::from(
                        "<html><head><title>New Tab</title></head>\
                         <body style=\"background:#1a1a2e;color:#e0e0e0;font-family:sans-serif;\">\
                         <h1>Blank Page</h1></body></html>"
                    );
                    let _ = self.browser.load_html("about:blank", &blank_html);
                    self.rendered_content = self.browser.render_current();
                    self.page_heading = "Blank Page".to_string();
                    self.status_msg = "about:blank".to_string();
                    self.is_loading = false;
                    return Command::none();
                }

                // Add https:// if missing
                let url = if !url.starts_with("http://") && !url.starts_with("https://") {
                    format!("https://{}", url)
                } else {
                    url.clone()
                };

                self.is_loading = true;
                self.status_msg = format!("Fetching {}...", url);
                self.url_input = url.clone();
                self.load_start_time = Some(Instant::now());
                self.render_stats_text = String::new();

                // Launch async network fetch — UI stays responsive
                let fetch_url = url.clone();
                return Command::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            crate::network::fetch_url(&fetch_url)
                                .map(|result| (result.body, result.url, result.fetch_time_ms))
                                .map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Task error: {}", e)))
                    },
                    |result| match result {
                        Ok((html, url, fetch_time)) => {
                            Message::PageLoaded { html, url, fetch_time }
                        }
                        Err(e) => Message::LoadError(e),
                    },
                )
            }
            Message::GoBack => {
                self.browser.go_back();
                if let Some(tab) = self.browser.active_tab() {
                    self.url_input = tab.url.clone();
                    self.page_heading = format!("{}", tab.title);
                    self.rendered_content = self.browser.render_current();
                    self.status_msg = "Navigated back".to_string();
                }
            }
            Message::GoForward => {
                self.browser.go_forward();
                if let Some(tab) = self.browser.active_tab() {
                    self.url_input = tab.url.clone();
                    self.page_heading = format!("{}", tab.title);
                    self.rendered_content = self.browser.render_current();
                    self.status_msg = "Navigated forward".to_string();
                }
            }
            Message::Reload => {
                if let Some(tab) = self.browser.active_tab() {
                    let url = tab.url.clone();
                    if url == "about:blank" || url == "https://ghitabrowser.local" {
                        self.url_input = url.clone();
                        self.status_msg = "Cannot reload this page".to_string();
                        return Command::none();
                    }
                    self.url_input = url.clone();
                    self.is_loading = true;
                    self.status_msg = format!("Reloading {}...", url);
                    self.load_start_time = Some(Instant::now());
                    self.render_stats_text = String::new();

                    let fetch_url = url.clone();
                    return Command::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                crate::network::fetch_url(&fetch_url)
                                    .map(|result| (result.body, result.url, result.fetch_time_ms))
                                    .map_err(|e| e.to_string())
                            })
                            .await
                            .unwrap_or_else(|e| Err(format!("Task error: {}", e)))
                        },
                        |result| match result {
                            Ok((html, url, fetch_time)) => {
                                Message::PageLoaded { html, url, fetch_time }
                            }
                            Err(e) => Message::LoadError(e),
                        },
                    );
                }
                return Command::none();
            }
            Message::Home => {
                let html = String::from(
                    "<html><head><title>GhitaBrowser v0.1.5</title></head>\
                     <body><h1>🚀 GhitaBrowser v0.1.5</h1>\
                     <p>Welcome! Enter a URL above to start browsing.</p></body></html>"
                );
                let _ = self.browser.load_html("https://ghitabrowser.local", &html);
                self.url_input = "https://ghitabrowser.local".to_string();
                self.page_heading = "GhitaBrowser v0.1.5".to_string();
                self.rendered_content = self.browser.render_current();
                self.status_msg = "Home".to_string();
                self.is_loading = false;
            }
            Message::SelectTab(index) => {
                if let Some(tab) = self.browser.tabs.get_tab_by_index(index) {
                    let id = tab.id;
                    self.browser.tabs.set_active_tab(id);
                    if let Some(active) = self.browser.active_tab() {
                        self.url_input = active.url.clone();
                        self.page_heading = format!("{}", active.title);
                        self.rendered_content = self.browser.render_current();
                        self.status_msg = format!("Tab: {}", active.title);
                    }
                }
            }
            Message::NewTab => {
                let html = String::from(
                    "<html><head><title>New Tab</title></head>\
                     <body><h1>New Tab</h1><p>Enter a URL and press Go.</p></body></html>"
                );
                let dom = parse_html(&html);
                self.browser.add_tab("about:blank", dom, "New Tab");
                
                if let Some(active) = self.browser.active_tab() {
                    self.url_input = active.url.clone();
                    self.page_heading = "New Tab".to_string();
                    self.rendered_content = self.browser.render_current();
                    self.status_msg = format!("New tab ({} tabs)", self.browser.tab_count());
                }
            }
            Message::CloseTab(index) => {
                if let Some(tab) = self.browser.tabs.get_tab_by_index(index) {
                    let id = tab.id;
                    self.browser.tabs.remove_tab(id);

                    // If no tabs left, create a blank one
                    if self.browser.tab_count() == 0 {
                        let blank_html = "<html><head><title>New Tab</title></head>\
                             <body><h1>New Tab</h1><p>Enter a URL and press Go.</p></body></html>";
                        let dom = parse_html(blank_html);
                        self.browser.add_tab("about:blank", dom, "New Tab");
                    }

                    if let Some(active) = self.browser.active_tab() {
                        self.url_input = active.url.clone();
                        self.page_heading = format!("{}", active.title);
                        self.rendered_content = self.browser.render_current();
                    }
                    self.status_msg = format!("Tab closed ({} remaining)", self.browser.tab_count());
                }
            }
            Message::CloseCurrentTab => {
                if let Some(active) = self.browser.active_tab() {
                    let id = active.id;
                    self.browser.tabs.remove_tab(id);

                    // If no tabs left, create a blank one
                    if self.browser.tab_count() == 0 {
                        let blank_html = "<html><head><title>New Tab</title></head>\
                             <body><h1>New Tab</h1><p>Enter a URL and press Go.</p></body></html>";
                        let dom = parse_html(blank_html);
                        self.browser.add_tab("about:blank", dom, "New Tab");
                    }

                    if let Some(active) = self.browser.active_tab() {
                        self.url_input = active.url.clone();
                        self.page_heading = format!("{}", active.title);
                        self.rendered_content = self.browser.render_current();
                    }
                    self.status_msg = format!("Tab closed ({} remaining)", self.browser.tab_count());
                }
            }
            Message::ToggleTheme => {
                self.is_dark_theme = !self.is_dark_theme;
                self.status_msg = if self.is_dark_theme {
                    "Dark theme".to_string()
                } else {
                    "Light theme".to_string()
                };
            }
            Message::ToggleDevTools => {
                self.show_devtools = !self.show_devtools;
                if self.show_devtools {
                    self.show_storage = false;
                    self.show_cache = false;
                    self.show_js_console = true;
                    if let Some(_tab) = self.browser.active_tab() {
                        self.js_console_text = self.browser.js_engine.console_output.join("\n");
                    }
                    self.js_input_text = String::new();
                    self.status_msg = "DevTools opened".to_string();
                } else {
                    self.show_storage = false;
                    self.show_cache = false;
                    self.show_js_console = false;
                    self.status_msg = "DevTools closed".to_string();
                }
            }
            Message::CloseDevTools => {
                if self.show_devtools {
                    self.show_devtools = false;
                    self.show_storage = false;
                    self.show_cache = false;
                    self.show_js_console = false;
                    self.status_msg = "DevTools closed".to_string();
                }
            }
            Message::ToggleStorage => {
                self.show_devtools = true;
                self.show_storage = true;
                self.show_cache = false;
                self.show_js_console = false;
                self.status_msg = "Storage panel".to_string();
            }
            Message::ToggleCache => {
                self.show_devtools = true;
                self.show_storage = false;
                self.show_cache = true;
                self.show_js_console = false;
                self.status_msg = "Cache panel".to_string();
            }
            Message::ToggleJsConsole => {
                self.show_devtools = true;
                self.show_storage = false;
                self.show_cache = false;
                self.show_js_console = true;
                self.js_console_text = self.browser.js_engine.console_output.join("\n");
                self.js_input_text = String::new();
                self.status_msg = "JS Console opened".to_string();
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
                            self.browser.js_engine.console_output.push(
                                format!("> {} = {}", code, output)
                            );
                            self.js_console_text = self.browser.js_engine.console_output.join("\n");
                            self.js_input_text = String::new();
                            self.status_msg = format!("JS: {} = {}", code, output);
                        }
                        Err(e) => {
                            self.browser.js_engine.console_output.push(
                                format!("> {}  // Error: {}", code, e)
                            );
                            self.js_console_text = self.browser.js_engine.console_output.join("\n");
                            self.js_input_text = String::new();
                            self.status_msg = format!("JS Error: {}", e);
                        }
                    }
                }
            }
            Message::FocusUrl => {
                self.status_msg = "URL bar ready — start typing".to_string();
            }
            Message::PageLoaded { html, url, fetch_time } => {
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
                let all_rules: Vec<crate::css_parser::CssRule> = self.browser.css_rules.iter()
                    .cloned()
                    .chain(page_css_rules)
                    .collect();
                let style_time = style_start.elapsed().as_millis() as u64;

                // 4. Create layout
                let layout_start = Instant::now();
                let layout_tree = crate::layout::create_layout_tree(&dom, &all_rules, self.browser.viewport_width());
                let layout_time = layout_start.elapsed().as_millis() as u64;

                // Cache layout tree
                if let Some(ref _root) = layout_tree {
                    if let Some(tab) = self.browser.active_tab_mut() {
                        tab.layout = layout_tree.clone();
                    }
                }

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

                let total_time = start.elapsed().as_millis() as u64;

                self.browser.last_render_stats = Some(crate::RenderStats {
                    parse_time_ms: parse_time,
                    style_time_ms: style_time,
                    layout_time_ms: layout_time,
                    render_time_ms: render_time,
                    total_time_ms: total_time,
                    dom_nodes,
                    layout_nodes: 0,
                });

                // 7. Update tab state
                if let Some(tab) = self.browser.active_tab_mut() {
                    let current_entry = crate::tab::HistoryEntry {
                        url: tab.url.clone(),
                        title: tab.title.clone(),
                        dom: tab.dom.clone(),
                        layout: tab.layout.clone(),
                    };
                    tab.push_history(current_entry);
                    tab.dom = dom;
                    tab.title = title.clone();
                    tab.url = url.clone();
                } else {
                    self.browser.add_tab(&url, dom, &title);
                }

                // 8. Update UI state
                self.rendered_content = rendered;
                self.last_load_time = Some(fetch_time + total_time as u64);
                self.is_loading = false;
                self.load_start_time = None;
                self.page_heading = title;
                self.render_stats_text = format!(
                    "Fetch: {}ms | Parse: {}ms | Style: {}ms | Layout: {}ms | Render: {}ms | Total: {}ms | {} DOM nodes",
                    fetch_time, parse_time, style_time, layout_time, render_time,
                    total_time, dom_nodes
                );
                self.status_msg = format!("Loaded {} | {}ms", url, fetch_time + total_time as u64);
            }
            Message::LoadError(err) => {
                self.status_msg = format!("Error: {}", err);
                self.is_loading = false;
                self.load_start_time = None;
                self.rendered_content = format_error_page(&err, &self.url_input);
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let tab_row = self.build_tab_bar();
        let nav_bar = self.build_toolbar();
        let main_content = if self.show_devtools {
            self.build_devtools_layout()
        } else {
            self.build_viewport()
        };
        let status_bar = self.build_status_bar();
        
        Element::from(
            column![tab_row, nav_bar, main_content, status_bar]
        )
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        keyboard::on_key_press(|key, modifiers| {
            handle_keyboard(key, modifiers)
        })
    }
}

// ===== Keyboard Shortcuts =====

fn handle_keyboard(key: iced::keyboard::Key, modifiers: iced::keyboard::Modifiers) -> Option<Message> {
    use iced::keyboard::Key;
    
    match key {
        Key::Named(iced::keyboard::key::Named::F5) => {
            Some(Message::Reload)
        }
        _ => {
            if modifiers.control() {
                match key {
                    Key::Character(c) if c == "l" || c == "L" => {
                        Some(Message::FocusUrl)
                    }
                    Key::Character(c) if c == "t" || c == "T" => {
                        Some(Message::NewTab)
                    }
                    Key::Character(c) if c == "w" || c == "W" => {
                        Some(Message::CloseCurrentTab)
                    }
                    Key::Character(c) if c == "r" || c == "R" => {
                        Some(Message::Reload)
                    }
                    Key::Character(c) if c == "d" || c == "D" => {
                        Some(Message::ToggleDevTools)
                    }
                    _ => None,
                }
            } else if modifiers.alt() {
                match key {
                    Key::Named(iced::keyboard::key::Named::ArrowLeft) => {
                        Some(Message::GoBack)
                    }
                    Key::Named(iced::keyboard::key::Named::ArrowRight) => {
                        Some(Message::GoForward)
                    }
                    _ => None,
                }
            } else {
                match key {
                    Key::Named(iced::keyboard::key::Named::Escape) => {
                        Some(Message::CloseDevTools)
                    }
                    _ => None,
                }
            }
        }
    }
}

// ===== Builder Methods =====

impl GhitaBrowserApp {
    fn build_tab_bar(&self) -> Element<'_, Message> {
        let mut tab_row = row![].spacing(2).padding(2);
        
        let tab_info: Vec<(usize, String, String)> = self.browser.tabs.iter_tabs()
            .into_iter().map(|t| (t.id, t.title.clone(), t.url.clone()))
            .collect();
        
        let active_id = self.browser.tabs.active_tab_id();
        
        for (i, (id, title, _url)) in tab_info.iter().enumerate() {
            let is_active = Some(*id) == active_id;
            
            // Tab pill styling
            let tab_bg: Element<'_, Message> = if is_active {
                container(text(title.clone()).size(12))
                    .style(|_: &Theme| container::Appearance {
                        background: Some(iced::Background::Color(BRAND_ORANGE)),
                        text_color: Some(Color::WHITE),
                        border: iced::Border { color: BRAND_ORANGE_DARK, width: 0.0, radius: 4.0.into() },
                        shadow: Shadow::default(),
                    })
                    .padding(4)
                    .into()
            } else {
                container(text(title.clone()).size(12))
                    .style(|_: &Theme| container::Appearance {
                        background: Some(iced::Background::Color(BRAND_NAVY_LIGHT)),
                        text_color: Some(BRAND_TEXT_DIM),
                        border: iced::Border { color: BRAND_BORDER, width: 0.0, radius: 4.0.into() },
                        shadow: Shadow::default(),
                    })
                    .padding(4)
                    .into()
            };
            
            let tab_btn = button(tab_bg)
                .on_press(Message::SelectTab(i))
                .style(if is_active {
                    iced::theme::Button::Primary
                } else {
                    iced::theme::Button::Text
                });
            
            tab_row = tab_row.push(tab_btn);
            
            // Close button
            let close_btn = button(text("✕").size(10).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
                .on_press(Message::CloseTab(i))
                .style(iced::theme::Button::Destructive);
            tab_row = tab_row.push(close_btn);
        }
        
        let new_tab_btn = button(text(" + ").size(13).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
            .on_press(Message::NewTab)
            .style(iced::theme::Button::Secondary);
        tab_row = tab_row.push(new_tab_btn);
        
        container(tab_row)
            .style(|_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(BRAND_NAVY)),
                text_color: Some(BRAND_TEXT),
                border: iced::Border { color: BRAND_BORDER, width: 0.0, radius: 0.0.into() },
                shadow: Shadow::default(),
            })
            .width(Length::Fill)
            .into()
    }
    
    fn build_toolbar(&self) -> Element<'_, Message> {
        let can_go_back = self.browser.active_tab()
            .map(|t| t.can_go_back())
            .unwrap_or(false);
        let can_go_forward = self.browser.active_tab()
            .map(|t| t.can_go_forward())
            .unwrap_or(false);
        
        // HTTPS padlock indicator
        let padlock: Element<'_, Message> = if self.url_input.starts_with("https://") {
            text(" 🔒 ").size(14).style(iced::theme::Text::from(BRAND_GREEN)).into()
        } else if self.url_input.starts_with("http://") {
            text(" ⚠ ").size(14).style(iced::theme::Text::from(BRAND_ORANGE)).into()
        } else {
            text(" ").size(14).style(iced::theme::Text::from(BRAND_TEXT_DIM)).into()
        };
        
        // Loading spinner
        let spinner: Element<'_, Message> = if self.is_loading {
            text(" ⟳ ").size(16).style(iced::theme::Text::from(BRAND_ORANGE)).into()
        } else {
            text("   ").size(16).style(iced::theme::Text::from(BRAND_TEXT_DIM)).into()
        };
        
        // Loading status text
        let loading_bar: Element<'_, Message> = if self.is_loading {
            text("Loading...").size(11).style(iced::theme::Text::from(BRAND_ORANGE)).into()
        } else {
            text("Ready").size(11).style(iced::theme::Text::from(BRAND_TEXT_DIM)).into()
        };
        
        let nav_buttons = row![
            button(text("◀").size(14).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
                .on_press_maybe(if can_go_back { Some(Message::GoBack) } else { None })
                .style(if can_go_back { iced::theme::Button::Secondary } else { iced::theme::Button::Text }),
            button(text("▶").size(14).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
                .on_press_maybe(if can_go_forward { Some(Message::GoForward) } else { None })
                .style(if can_go_forward { iced::theme::Button::Secondary } else { iced::theme::Button::Text }),
            button(text("⟳").size(14).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
                .on_press(Message::Reload)
                .style(iced::theme::Button::Secondary),
            button(text("🏠").size(14).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
                .on_press(Message::Home)
                .style(iced::theme::Button::Secondary),
            spinner,
            padlock,
        ]
        .spacing(4)
        .padding(2);
        
        let url_input_widget = text_input("Enter URL and press Go...", &self.url_input)
            .on_input(Message::UrlChanged)
            .on_submit(Message::Navigate)
            .padding(8)
            .width(Length::Fill)
            .style(iced::theme::TextInput::Default);
        
        let clear_btn = button(text("✕").size(12).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
            .on_press(Message::ClearUrl)
            .style(iced::theme::Button::Text);
        
        let go_btn = button(text(" Go ").size(14).style(iced::theme::Text::from(Color::WHITE)))
            .on_press(Message::Navigate)
            .style(iced::theme::Button::Primary);
        
        Element::from(
            row![nav_buttons, url_input_widget, clear_btn, go_btn, loading_bar]
                .spacing(4)
                .padding(4)
                .align_items(iced::Alignment::Center)
        )
    }
    
    fn build_viewport(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = vec![];
        
        // Page heading
        if !self.page_heading.is_empty() {
            items.push(
                text(&self.page_heading).size(20).style(iced::theme::Text::from(BRAND_TEXT)).into()
            );
            items.push(vertical_space().height(6).into());
        }
        
        // Loading indicator
        if self.is_loading {
            items.push(
                container(
                    row![
                        text("⟳").size(16).style(iced::theme::Text::from(BRAND_ORANGE)),
                        text(&self.status_msg).size(13).style(iced::theme::Text::from(BRAND_ORANGE)),
                    ].spacing(8).align_items(iced::Alignment::Center)
                )
                .padding(8)
                .style(|_: &Theme| container::Appearance {
                    background: Some(iced::Background::Color(BRAND_NAVY_LIGHT)),
                    text_color: Some(BRAND_ORANGE),
                    border: iced::Border { color: BRAND_ORANGE, width: 1.0, radius: 6.0.into() },
                    shadow: Shadow::default(),
                })
                .into()
            );
            items.push(vertical_space().height(6).into());
        }
        
        // Page content
        items.push(
            text(&self.rendered_content).size(13)
                .style(iced::theme::Text::from(if self.is_dark_theme { 
                    Color::from_rgb(0.85, 0.85, 0.90) 
                } else { 
                    Color::from_rgb(0.20, 0.20, 0.25) 
                }))
                .into()
        );
        
        // Render stats
        if !self.render_stats_text.is_empty() {
            items.push(vertical_space().height(8).into());
            items.push(
                text(&self.render_stats_text).size(11).style(iced::theme::Text::from(BRAND_TEXT_DIM)).into()
            );
        }
        
        // JS Console panel
        if self.show_js_console {
            items.push(vertical_space().height(12).into());
            
            let console_header = text("─ JS Console ─────────────────────").size(12)
                .style(iced::theme::Text::from(BRAND_YELLOW));
            let console_body = container(
                scrollable(
                    text(&self.js_console_text).size(12).style(iced::theme::Text::from(BRAND_TEXT))
                )
                .width(Length::Fill)
                .height(Length::Fixed(120.0))
            )
            .style(|_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(BRAND_NAVY)),
                text_color: Some(BRAND_TEXT),
                border: iced::Border { color: BRAND_BORDER, width: 1.0, radius: 4.0.into() },
                shadow: Shadow::default(),
            })
            .padding(8)
            .width(Length::Fill);
            
            let console_input_row = row![
                text_input("Enter JavaScript code...", &self.js_input_text)
                    .on_input(Message::JsCodeChanged)
                    .on_submit(Message::ExecuteJs)
                    .padding(6)
                    .width(Length::Fill),
                button(text("▶ Run").size(12).style(iced::theme::Text::from(Color::WHITE)))
                    .on_press(Message::ExecuteJs)
                    .style(iced::theme::Button::Primary),
            ]
            .spacing(4);
            
            let console_panel = container(
                column![console_header, console_body, console_input_row]
                    .spacing(4)
            )
            .style(|_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(BRAND_NAVY_LIGHT)),
                text_color: Some(BRAND_TEXT),
                border: iced::Border { color: BRAND_ORANGE, width: 1.0, radius: 6.0.into() },
                shadow: Shadow::default(),
            })
            .padding(10)
            .width(Length::Fill);
            
            items.push(console_panel.into());
        }
        
        scrollable(column(items).spacing(4))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
    
    fn build_devtools_layout(&self) -> Element<'_, Message> {
        // Select which panel to show in the viewport area
        let viewport_content: Element<'_, Message> = if self.show_storage {
            self.build_storage_viewport()
        } else if self.show_cache {
            self.build_cache_viewport()
        } else {
            self.build_js_console_viewport()
        };
        
        let js_btn_style = if self.show_js_console && !self.show_storage && !self.show_cache {
            iced::theme::Button::Primary
        } else {
            iced::theme::Button::Text
        };
        let storage_btn_style = if self.show_storage {
            iced::theme::Button::Primary
        } else {
            iced::theme::Button::Text
        };
        let cache_btn_style = if self.show_cache {
            iced::theme::Button::Primary
        } else {
            iced::theme::Button::Text
        };
        
        let side_panel = container(
            column![
                text("DevTools").size(14).style(iced::theme::Text::from(BRAND_ORANGE)),
                vertical_space().height(8),
                text("Panels").size(11).style(iced::theme::Text::from(BRAND_TEXT_DIM)),
                button("JS Console").on_press(Message::ToggleJsConsole).style(js_btn_style),
                button("Storage").on_press(Message::ToggleStorage).style(storage_btn_style),
                button("Cache").on_press(Message::ToggleCache).style(cache_btn_style),
                vertical_space().height(16),
                button("Close DevTools").on_press(Message::ToggleDevTools).style(iced::theme::Button::Destructive),
            ]
            .spacing(6)
            .padding(4)
        )
        .style(|_: &Theme| container::Appearance {
            background: Some(iced::Background::Color(BRAND_NAVY_LIGHT)),
            text_color: Some(BRAND_TEXT),
            border: iced::Border { color: BRAND_BORDER, width: 1.0, radius: 0.0.into() },
            shadow: Shadow::default(),
        })
        .padding(12)
        .width(Length::Fixed(180.0))
        .height(Length::Fill);
        
        Element::from(
            row![viewport_content, side_panel]
                .spacing(0)
        )
    }
    
    fn build_js_console_viewport(&self) -> Element<'_, Message> {
        let console_header = text("─ JS Console ─────────────────────").size(12)
            .style(iced::theme::Text::from(BRAND_YELLOW));
        let console_body = container(
            scrollable(
                text(&self.js_console_text).size(12).style(iced::theme::Text::from(BRAND_TEXT))
            )
            .width(Length::Fill)
            .height(Length::Fixed(200.0))
        )
        .style(|_: &Theme| container::Appearance {
            background: Some(iced::Background::Color(BRAND_NAVY)),
            text_color: Some(BRAND_TEXT),
            border: iced::Border { color: BRAND_BORDER, width: 1.0, radius: 4.0.into() },
            shadow: Shadow::default(),
        })
        .padding(8)
        .width(Length::Fill);
        
        let console_input_row = row![
            text_input("Enter JavaScript...", &self.js_input_text)
                .on_input(Message::JsCodeChanged)
                .on_submit(Message::ExecuteJs)
                .padding(6)
                .width(Length::Fill),
            button(text("▶ Run").size(12).style(iced::theme::Text::from(Color::WHITE)))
                .on_press(Message::ExecuteJs)
                .style(iced::theme::Button::Primary),
        ]
        .spacing(4);
        
        container(
            column![console_header, console_body, console_input_row]
                .spacing(8)
        )
        .style(|_: &Theme| container::Appearance {
            background: Some(iced::Background::Color(BRAND_NAVY_LIGHT)),
            text_color: Some(BRAND_TEXT),
            border: iced::Border { color: BRAND_ORANGE, width: 1.0, radius: 6.0.into() },
            shadow: Shadow::default(),
        })
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
    
    fn build_storage_viewport(&self) -> Element<'_, Message> {
        let header = text("Storage Inspector").size(16).style(iced::theme::Text::from(BRAND_ORANGE));
        
        let cookies_text = text(format!("Cookies: {}", self.browser.storage.cookie_count()))
            .size(13).style(iced::theme::Text::from(BRAND_TEXT));
        let origins_text = text(format!("localStorage origins: {}", self.browser.storage.local_storage_origins().len()))
            .size(13).style(iced::theme::Text::from(BRAND_TEXT));
        
        let mut details = column![header, cookies_text, origins_text].spacing(8);
        
        for origin in self.browser.storage.local_storage_origins() {
            if let Some(ls) = self.browser.storage.get_local_storage(&origin) {
                details = details.push(
                    text(format!("  {} ({} items)", origin, ls.len())).size(12).style(iced::theme::Text::from(BRAND_TEXT_DIM))
                );
            }
        }
        
        container(
            scrollable(details.spacing(4))
                .width(Length::Fill)
                .height(Length::Fill)
        )
        .style(|_: &Theme| container::Appearance {
            background: Some(iced::Background::Color(BRAND_NAVY)),
            text_color: Some(BRAND_TEXT),
            border: iced::Border { color: BRAND_BORDER, width: 1.0, radius: 4.0.into() },
            shadow: Shadow::default(),
        })
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
    
    fn build_cache_viewport(&self) -> Element<'_, Message> {
        let header = text("Resource Cache Stats").size(16).style(iced::theme::Text::from(BRAND_ORANGE));
        
        let stats = self.browser.cache.stats();
        let hit_rate = if stats.hits + stats.misses > 0 {
            format!("{:.1}%", (stats.hits as f64 / (stats.hits + stats.misses) as f64) * 100.0)
        } else { "0.0%".to_string() };
        
        let info_lines = column![
            text(format!("Entries: {}", stats.entries)).size(13).style(iced::theme::Text::from(BRAND_TEXT)),
            text(format!("Hits: {}", stats.hits)).size(13).style(iced::theme::Text::from(BRAND_GREEN)),
            text(format!("Misses: {}", stats.misses)).size(13).style(iced::theme::Text::from(BRAND_RED)),
            text(format!("Hit rate: {}", hit_rate)).size(13).style(iced::theme::Text::from(BRAND_TEXT)),
            text(format!("Max size: {} MB", stats.max_size / (1024 * 1024))).size(13).style(iced::theme::Text::from(BRAND_TEXT_DIM)),
        ].spacing(6);
        
        container(
            column![header, info_lines].spacing(12)
        )
        .style(|_: &Theme| container::Appearance {
            background: Some(iced::Background::Color(BRAND_NAVY)),
            text_color: Some(BRAND_TEXT),
            border: iced::Border { color: BRAND_BORDER, width: 1.0, radius: 4.0.into() },
            shadow: Shadow::default(),
        })
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
    
    fn build_status_bar(&self) -> Element<'_, Message> {
        let load_time_str = self.last_load_time
            .map(|t| format!("{}ms", t))
            .unwrap_or_else(|| "—".to_string());
        
        let status_row = row![
            text(format!("{} | {}", self.status_msg, load_time_str)).size(11).style(iced::theme::Text::from(BRAND_TEXT_DIM)),
            horizontal_space(),
            button(text("🎨").size(11).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
                .on_press(Message::ToggleTheme)
                .style(iced::theme::Button::Text),
            button(text("⚙").size(11).style(iced::theme::Text::from(BRAND_TEXT_DIM)))
                .on_press(Message::ToggleDevTools)
                .style(iced::theme::Button::Text),
            text(format!("v0.1.5 | {} tabs", self.browser.tab_count())).size(11).style(iced::theme::Text::from(BRAND_TEXT_DIM)),
        ]
        .spacing(6)
        .padding(4)
        .align_items(iced::Alignment::Center);
        
        container(status_row)
            .style(|_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(BRAND_NAVY)),
                text_color: Some(BRAND_TEXT_DIM),
                border: iced::Border { color: BRAND_BORDER, width: 0.0, radius: 0.0.into() },
                shadow: Shadow::default(),
            })
            .width(Length::Fill)
            .into()
    }
}

// ===== Error Page Formatter =====

fn format_error_page(error: &str, url: &str) -> String {
    format!(
        "\n\
         ╔════════════════════════════════════════════╗\n\
         ║        ⚠️  Page Load Error                  ║\n\
         ╚════════════════════════════════════════════╝\n\n\
         URL: {}\n\
         Error: {}\n\n\
         Suggestions:\n\
         • Check the URL for typos\n\
         • Make sure you're connected to the internet\n\
         • Try again with a different URL\n\
         • The site might be down\n\n\
         Press Reload or enter a new URL above.\n",
        url, error
    )
}

pub fn run_gui() -> Result<(), iced::Error> {
    let mut settings = Settings::default();
    settings.window.size = iced::Size::new(1280.0, 900.0);
    settings.window.min_size = Some(iced::Size::new(800.0, 600.0));
    info!("Starting GhitaBrowser v0.1.5 GUI");
    GhitaBrowserApp::run(settings)
}

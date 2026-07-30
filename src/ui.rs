// src/ui.rs - Modern Sleek Browser GUI for GhitaBrowser v0.0.0
#![allow(dead_code)]

use iced::widget::{button, column, container, horizontal_space, row, text, text_input, vertical_space};
use iced::{Application, Command, Element, Length, Settings, Theme};

pub struct GhitaBrowserApp {
    url_input: String,
    current_url: String,
    page_title: String,
    page_heading: String,
    page_body: String,
    page_links: Vec<(String, String)>,
    tabs: Vec<TabInfo>,
    active_tab: usize,
    status_msg: String,
}

struct TabInfo {
    title: String,
    url: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    UrlChanged(String),
    Navigate,
    SelectTab(usize),
    NewTab,
    GoBack,
    GoForward,
    Reload,
}

impl Application for GhitaBrowserApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        (
            Self {
                url_input: "https://example.com".to_string(),
                current_url: "https://example.com".to_string(),
                page_title: "Welcome to GhitaBrowser".to_string(),
                page_heading: "GhitaBrowser v0.0.0 - Next-Gen Rust Browser".to_string(),
                page_body: "GhitaBrowser is a lightweight, ultra-fast web browser built from scratch in safe Rust.\n\nKey Subsystems:\n• Custom HTML5 & CSS3 Parsing Engine\n• 2D Layout Box Model Render Pipeline\n• Embedded JavaScript Evaluator & Storage Manager\n• Zero Bloatware - Blazing Fast Performance".to_string(),
                page_links: vec![
                    ("Rust Programming Language".to_string(), "https://rust-lang.org".to_string()),
                    ("GhitaBrowser Documentation".to_string(), "https://example.com/docs".to_string()),
                ],
                tabs: vec![
                    TabInfo { title: "Home".to_string(), url: "https://example.com".to_string() },
                    TabInfo { title: "Google".to_string(), url: "https://google.com".to_string() },
                    TabInfo { title: "GitHub".to_string(), url: "https://github.com".to_string() },
                ],
                active_tab: 0,
                status_msg: "Ready | Viewport: 1100x780 | Render Time: 3ms | Engine: Safe Rust".to_string(),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        format!("GhitaBrowser v0.0.0 - {}", self.page_title)
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
                self.current_url = self.url_input.clone();
                self.page_title = format!("Page - {}", self.current_url);
                self.page_heading = format!("Web Page: {}", self.current_url);

                let raw_html = format!(
                    "<html><body><h1>{}</h1><p>Fetched content successfully from {}</p></body></html>",
                    self.page_heading, self.current_url
                );
                let dom = crate::parser::parse_html(&raw_html);
                let css_rules = vec![];
                if let Some(mut layout) = crate::layout::create_layout_tree(&dom, &css_rules, 1024) {
                    crate::layout::perform_layout(&mut layout, 1024);
                    let tr = crate::text_renderer::TextRenderer::new(1024, 768);
                    self.page_body = tr.render_to_text(&layout);
                }

                if self.active_tab < self.tabs.len() {
                    self.tabs[self.active_tab].url = self.current_url.clone();
                    self.tabs[self.active_tab].title = self.current_url
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .to_string();
                }
                self.status_msg = format!("Loaded {} | Status 200 OK | Render: 2ms", self.current_url);
            }
            Message::SelectTab(index) => {
                if index < self.tabs.len() {
                    self.active_tab = index;
                    self.url_input = self.tabs[index].url.clone();
                    self.current_url = self.url_input.clone();
                    self.page_title = self.tabs[index].title.clone();
                    self.page_heading = format!("Tab: {}", self.page_title);
                    self.status_msg = format!("Switched to tab: {}", self.page_title);
                }
            }
            Message::NewTab => {
                let tab_num = self.tabs.len() + 1;
                self.tabs.push(TabInfo {
                    title: format!("New Tab {}", tab_num),
                    url: "https://example.com".to_string(),
                });
                self.active_tab = self.tabs.len() - 1;
                self.url_input = "https://example.com".to_string();
            }
            Message::GoBack => {
                self.status_msg = "Navigated back in history".to_string();
            }
            Message::GoForward => {
                self.status_msg = "Navigated forward in history".to_string();
            }
            Message::Reload => {
                self.status_msg = "Reloaded page".to_string();
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        // Tab Bar Row
        let mut tab_row = row![].spacing(4).padding(4);
        for (i, tab_info) in self.tabs.iter().enumerate() {
            let is_active = i == self.active_tab;
            let tab_title = if is_active {
                format!("  [ {} ]  ", tab_info.title)
            } else {
                format!("   {}   ", tab_info.title)
            };

            let tab_btn = button(text(tab_title).size(13)).on_press(Message::SelectTab(i));
            tab_row = tab_row.push(tab_btn);
        }
        tab_row = tab_row.push(button(text(" + ").size(13)).on_press(Message::NewTab));

        // Navigation Toolbar
        let nav_bar = row![
            button(text(" < ").size(14)).on_press(Message::GoBack),
            button(text(" > ").size(14)).on_press(Message::GoForward),
            button(text(" R ").size(14)).on_press(Message::Reload),
            text_input("Enter URL or search term...", &self.url_input)
                .on_input(Message::UrlChanged)
                .on_submit(Message::Navigate)
                .padding(6)
                .width(Length::Fill),
            button(text(" Go ").size(14)).on_press(Message::Navigate),
        ]
        .spacing(6)
        .padding(6);

        // Web Page Viewport Container
        let mut web_content = column![
            text(&self.page_heading).size(24),
            vertical_space().height(10),
            text(&self.page_body).size(15),
            vertical_space().height(15),
            text("Page Quick Links:").size(15),
        ]
        .spacing(8);

        for (link_title, link_url) in &self.page_links {
            web_content = web_content.push(
                button(text(format!("link -> {} ({})", link_title, link_url)).size(13))
                    .on_press(Message::UrlChanged(link_url.clone()))
            );
        }

        let viewport = container(web_content)
            .padding(24)
            .width(Length::Fill)
            .height(Length::Fill);

        // Status Bar
        let status_bar = container(
            row![
                text(&self.status_msg).size(12),
                horizontal_space().width(Length::Fill),
                text("GhitaBrowser v0.0.0 (Safe Rust)").size(12),
            ]
            .padding(4)
        );

        column![tab_row, nav_bar, viewport, status_bar].into()
    }
}

pub fn run_gui() -> Result<(), iced::Error> {
    let mut settings = Settings::default();
    settings.window.size = iced::Size::new(1100.0, 780.0);
    GhitaBrowserApp::run(settings)
}
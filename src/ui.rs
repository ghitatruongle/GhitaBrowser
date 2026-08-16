// Iced GUI application

use iced::widget::{
    button, canvas, column, container, horizontal_space, row, scrollable, text, text_input,
    vertical_space,
};
use iced::{mouse, Application, Color, Command, Element, Length, Settings, Shadow, Theme};
use log::info;
use std::collections::HashMap;
use std::sync::Arc;

use crate::audio_output::AudioSink;
use crate::paint::{DisplayItem, DisplayList};
use crate::parser::parse_html;
use crate::search::{search_web_async_with_cancellation, SearchResult};
use crate::Browser;

/// Detect if a page appears to be a JavaScript SPA (no rendered content).
/// Returns true if the page has lots of script code but very little text content,
/// indicating the content is loaded via JavaScript.
fn is_spa_or_js_rendered(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();

    // ONLY mark as SPA when there's STRONG evidence of JS rendering
    // We use a scoring system to avoid false positives
    let mut spa_score: i32 = 0;

    // Strong indicators (high confidence)
    if lower.contains("<ytd-app") || lower.contains("<ytd-") {
        spa_score += 10; // YouTube
    }
    if lower.contains("__nuxt") {
        spa_score += 10; // Nuxt.js
    }
    if lower.contains("__next") && lower.contains("<div id=\"__next\"") {
        spa_score += 10; // Next.js
    }
    if lower.contains("ng-version") {
        spa_score += 10; // Angular
    }

    // Medium indicators
    if lower.contains("data-reactroot") {
        spa_score += 5;
    }
    if lower.contains("ng-app") {
        spa_score += 5;
    }

    // Only count <div id="app"> if it's empty (typical for SPAs)
    if lower.contains("<div id=\"app\"></div>") || lower.contains("<div id=\"app\" ></div>") {
        spa_score += 5;
    }
    if lower.contains("<div id=\"root\"></div>") || lower.contains("<div id=\"root\" ></div>") {
        spa_score += 5;
    }

    // Optimize: count script tags and visible text in a single pass
    let mut visible_len = 0;
    let mut in_skip = false;
    let mut i = 0;
    let len = lower.len();

    while i < len {
        if lower.as_bytes()[i] == b'<' {
            // Check if this is a script/style/noscript tag
            if i + 1 < len && lower.as_bytes()[i + 1] == b's' {
                // Check for "script", "style", "noscript"
                if lower[i..].starts_with("<script")
                    || lower[i..].starts_with("<style")
                    || lower[i..].starts_with("<noscript")
                {
                    in_skip = true;
                    i += 7; // Skip past "<script", "<style", or "<noscript"
                    continue;
                }
            }
            if lower[i..].starts_with("</") && in_skip {
                in_skip = false;
                i += 2; // Skip past "</"
                continue;
            }
        }

        if !in_skip {
            visible_len += 1;
        }

        i += 1;
    }

    // Count script tags
    let script_count = lower.match_indices("<script").count();
    let script_len = script_count.min(20) * 500; // Cap at 20 scripts * 500 chars each

    let total_len = html.len();

    // Very strong heuristic: almost no visible text on a large page
    if total_len > 100_000 && visible_len < 1000 {
        spa_score += 10;
    } else if total_len > 50_000 && visible_len < 500 {
        spa_score += 5;
    }

    // If scripts are dominant (>40% of HTML is scripts) - very strong indicator
    if total_len > 0 && script_len * 10 > total_len * 4 {
        spa_score += 5;
    }

    // Only mark as SPA if score is high enough (conservative)
    spa_score >= 10
}

/// Switch to a normal-flow layout only when the collision signal is strong.
/// A few overlaps are legitimate (badges, menus, dialogs), while dozens of
/// mostly-covered text boxes make a document unusable.
fn requires_safe_flow_layout(overlapping_pairs: usize, collision_score: f64) -> bool {
    (overlapping_pairs >= 8 && collision_score >= 0.02) || overlapping_pairs >= 24
}

fn has_navigable_prefix(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("file://")
        || lower.starts_with("ghita://")
        || lower.starts_with("about:")
        || lower.starts_with("www.")
}

/// Recover replacement semantics if a platform text widget appends a newly
/// typed absolute URL before the global Ctrl+L command is delivered. Restrict
/// this to an exact old-value prefix/suffix so embedded redirect URLs in a
/// normal query remain untouched.
fn normalize_omnibox_replacement(previous: &str, edited: String) -> String {
    if previous.is_empty() || edited == previous {
        return edited;
    }
    if let Some(replacement) = edited.strip_prefix(previous) {
        if has_navigable_prefix(replacement) {
            return replacement.to_string();
        }
    }
    if let Some(replacement) = edited.strip_suffix(previous) {
        if has_navigable_prefix(replacement) {
            return replacement.to_string();
        }
    }
    edited
}

/// Extract YouTube video ID from URL (e.g., youtube.com/watch?v=ID, youtu.be/ID)
/// Returns Some(video_id) if URL is a YouTube watch URL.
fn extract_youtube_video_id(url: &str) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(url) {
        let host = parsed.host_str()?.to_lowercase();
        let path = parsed.path();

        // youtube.com/watch?v=ID
        if (host == "youtube.com" || host == "www.youtube.com" || host == "m.youtube.com")
            && path == "/watch"
        {
            return parsed
                .query_pairs()
                .find(|(k, _)| k == "v")
                .map(|(_, v)| v.into_owned());
        }

        // youtu.be/ID or youtube.com/shorts/ID or /embed/ID
        if host == "youtu.be" {
            let id = path.trim_start_matches('/');
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        if (host == "youtube.com" || host == "www.youtube.com" || host == "m.youtube.com")
            && (path.starts_with("/shorts/") || path.starts_with("/embed/"))
        {
            let id = path.split('/').nth(2).unwrap_or("");
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Build a "video info" HTML page for YouTube when JS rendering is not available.
/// Shows the video thumbnail (which our image loader CAN display), the title,
/// and alternative ways to watch.
fn build_video_info_html(video_id: &str, source_url: &str) -> String {
    // Extract a title from the source URL if possible
    let title = "YouTube Video";
    // Thumbnails are plain JPEGs served from i.ytimg.com — the image loader
    // fetches and displays them like any other <img>.
    let thumb_url = format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id);

    // video_id and source_url come from user-supplied URLs; escape them so a
    // `?"/>` payload can't break out of the attribute and inject markup.
    let safe_id = html_escape(video_id);
    let safe_url = html_escape(source_url);
    let safe_thumb = html_escape(&thumb_url);

    format!(
        "<html><head><title>{title} ({id}) - GhitaBrowser</title>\
         <style>\
         body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #0f0f12; color: #e1e1e6; padding: 32px; margin: 0; line-height: 1.6; }}\
         .card {{ background: #1a1a24; border: 1px solid #2e2e3e; border-radius: 12px; padding: 24px; max-width: 640px; margin: 0 auto; box-shadow: 0 8px 24px rgba(0,0,0,0.4); }}\
         h1 {{ font-size: 22px; margin-top: 0; color: #ffffff; }}\
         .thumb {{ width: 100%; height: auto; border-radius: 8px; margin: 16px 0; border: 1px solid #333; }}\
         .meta {{ background: #121218; padding: 12px 16px; border-radius: 8px; font-size: 14px; margin-bottom: 20px; }}\
         .meta p {{ margin: 6px 0; }}\
         .btn {{ display: inline-block; background: #3b82f6; color: #fff; padding: 10px 18px; border-radius: 6px; text-decoration: none; font-weight: 500; margin-right: 8px; margin-bottom: 8px; }}\
         .btn:hover {{ background: #2563eb; }}\
         .btn-secondary {{ background: #2a2a38; color: #cbd5e1; }}\
         .btn-secondary:hover {{ background: #38384c; }}\
         .notice {{ font-size: 13px; color: #94a3b8; margin-top: 20px; border-top: 1px solid #2a2a38; padding-top: 16px; }}\
         </style></head>\
         <body><div class=\"card\">\
         <h1>{title}</h1>\
         <img class=\"thumb\" src=\"{thumb}\" alt=\"Video thumbnail\"/>\
         <div class=\"meta\">\
         <p><b>Video ID:</b> <code>{id}</code></p>\
         <p><b>Source URL:</b> <a style=\"color:#60a5fa;word-break:break-all;\" href=\"{url}\">{url}</a></p>\
         </div>\
         <div style=\"margin: 16px 0;\">\
         <a class=\"btn\" href=\"https://www.youtube.com/embed/{id}\">Watch via Embed</a>\
         <a class=\"btn btn-secondary\" href=\"ghita://search?q={id}\">Search in Ghita</a>\
         </div>\
         <p class=\"notice\">GhitaBrowser 2.0.6 document player mode active.</p>\
         </div></body></html>",
        id = safe_id,
        url = safe_url,
        thumb = safe_thumb,
        title = title
    )
}

/// Render the browser-owned YouTube navigation shell from bounded bootstrap
/// data. This provides real result/watch links without copying website code;
/// it deliberately does not claim media playback when the live player gate has
/// not passed.
fn build_youtube_shell_html(source_url: &str, source_html: &str) -> Option<String> {
    let shell = crate::youtube::YouTubeShell::from_html(source_url, source_html).ok()?;
    let player_status = match crate::youtube::YouTubePlayerResponse::from_html(source_html) {
        Ok(response) => format!(
            "Player metadata validated for {} direct clear-content format(s).",
            response.formats.len()
        ),
        Err(error) => format!("Player unavailable: {}", html_escape(&error)),
    };
    Some(build_youtube_shell_from_model(
        source_url,
        &shell,
        &player_status,
    ))
}

fn build_youtube_shell_from_model(
    source_url: &str,
    shell: &crate::youtube::YouTubeShell,
    player_status: &str,
) -> String {
    use std::fmt::Write as _;

    let route_label = match &shell.route {
        crate::youtube::YouTubeRoute::Home => "Home".to_string(),
        crate::youtube::YouTubeRoute::Search { query } => {
            format!("Search: {}", html_escape(query))
        }
        crate::youtube::YouTubeRoute::Watch { video_id } => {
            format!("Watch: {}", html_escape(video_id))
        }
        crate::youtube::YouTubeRoute::Playlist { playlist_id, .. } => {
            format!("Playlist: {}", html_escape(playlist_id))
        }
        crate::youtube::YouTubeRoute::Channel { channel_id } => {
            format!("Channel: {}", html_escape(channel_id))
        }
        crate::youtube::YouTubeRoute::Shorts { video_id } => {
            format!("Shorts: {}", html_escape(video_id))
        }
        crate::youtube::YouTubeRoute::Embed { video_id } => {
            format!("Embed: {}", html_escape(video_id))
        }
    };
    let mut cards = String::new();
    for result in shell.results.iter().take(24) {
        let id = html_escape(&result.video_id);
        let title = html_escape(&result.title);
        let duration = result
            .duration_text
            .as_deref()
            .map(html_escape)
            .unwrap_or_default();
        let thumbnail = result
            .thumbnail_url
            .as_deref()
            .map(html_escape)
            .map(|url| format!("<img src=\"{url}\" width=\"240\" height=\"135\" alt=\"{title}\"/>"))
            .unwrap_or_default();
        let _ = write!(
            cards,
            "<li style=\"margin:16px 0\"><a href=\"https://www.youtube.com/watch?v={id}\">{thumbnail}<br/><b>{title}</b></a> <span>{duration}</span></li>"
        );
    }
    if cards.is_empty() {
        cards.push_str("<li>No bounded video results were present in the server bootstrap.</li>");
    }
    format!(
        "<html><head><title>YouTube - GhitaBrowser</title></head>\
         <body style=\"font-family:sans-serif;padding:24px;max-width:960px\">\
         <h1>YouTube</h1><p><b>{route}</b></p>\
         <p><a href=\"https://www.youtube.com/\">Home</a> | Use the address bar for YouTube search URLs.</p>\
         <p>{player}</p><h2>Videos</h2><ul style=\"list-style:none;padding:0\">{cards}</ul>\
         <p><b>Address:</b> {url}</p></body></html>",
        route = route_label,
        player = player_status,
        cards = cards,
        url = html_escape(source_url),
    )
}

fn build_spa_fallback_html(title: &str, url: &str) -> String {
    let safe_title = if title.is_empty() {
        "JavaScript Required".to_string()
    } else {
        html_escape(title)
    };
    let safe_url = html_escape(url);
    format!(
        "<html><head><title>{safe_title}</title></head>\
         <body><h1>This page requires unsupported web features</h1>\
         <p>The application at <b>{safe_url}</b> requires JavaScript or Web APIs outside GhitaBrowser's bounded runtime profile.</p>\
         <h2>What you can do:</h2><ul>\
         <li>Reload after checking the address and connection</li>\
         <li>Try a simpler/mobile page when the site provides one</li>\
         <li>Use Reader Mode for document-focused content</li></ul>\
         <p><b>Address:</b> {safe_url}</p></body></html>"
    )
}

/// Human-readable error title and message for network failures.
/// Converts technical error strings into user-friendly messages.
fn humanize_error(err: &str) -> (String, String) {
    let err_lower = err.to_ascii_lowercase();

    if err_lower.contains("timed out") || err_lower.contains("timeout") {
        (
            "Page took too long".to_string(),
            "The server is not responding. Try again later or check your connection.".to_string(),
        )
    } else if err_lower.contains("connection refused") {
        (
            "Cannot connect".to_string(),
            "This site refused the connection. The site may be down or blocking your browser."
                .to_string(),
        )
    } else if err_lower.contains("dns")
        || err_lower.contains("name or service not known")
        || err_lower.contains("could not resolve")
    {
        (
            "Page not found".to_string(),
            "Could not find this page. Check the address for typos.".to_string(),
        )
    } else if err_lower.contains("status 404") || err_lower.contains("not found") {
        (
            "Page not found".to_string(),
            "The page you requested does not exist on this server.".to_string(),
        )
    } else if err_lower.contains("status 500")
        || err_lower.contains("status 502")
        || err_lower.contains("status 503")
        || err_lower.contains("status 504")
        || err_lower.contains("internal server error")
        || err_lower.contains("bad gateway")
        || err_lower.contains("service unavailable")
        || err_lower.contains("gateway timeout")
    {
        (
            "Server error".to_string(),
            "Something went wrong on this website. Try again in a few moments.".to_string(),
        )
    } else if err_lower.contains("ssl")
        || err_lower.contains("tls")
        || err_lower.contains("certificate")
        || err_lower.contains("secure")
    {
        (
            "Secure connection failed".to_string(),
            "Your connection is not private. The site may be trying to steal your information."
                .to_string(),
        )
    } else if err_lower.contains("network")
        || err_lower.contains("no route")
        || err_lower.contains("unreachable")
    {
        (
            "You're offline".to_string(),
            "Check your internet connection and try again.".to_string(),
        )
    } else if err_lower.contains("no such host") || err_lower.contains("name does not resolve") {
        (
            "Page not found".to_string(),
            "This website does not exist. Check the address for typos.".to_string(),
        )
    } else {
        (
            "This site can't be reached".to_string(),
            format!(
                "{} may be down or blocking your connection. Try checking the address or reload (F5).",
                err.split(':').next().unwrap_or(err)
            ),
        )
    }
}

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

#[cfg(target_os = "windows")]
struct LiveYouTubeUiPlayback {
    tab_id: usize,
    controller: crate::youtube::LiveYouTubeController,
    audio_sink: crate::audio_output::WindowsWasapiSink,
    frame_handle: Option<iced::widget::image::Handle>,
    downloaded_bytes: usize,
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
    load_cancellations: HashMap<usize, crate::network_scheduler::CancellationToken>,

    #[cfg(target_os = "windows")]
    youtube_playback: Option<LiveYouTubeUiPlayback>,

    // Pixel renderer state
    display_list: Arc<DisplayList>,
    canvas_cache: canvas::Cache,
    /// Decoded image handles (url -> RGBA pixels) for the web page widget
    page_image_handles: Arc<HashMap<String, iced::widget::image::Handle>>,

    // DevTools
    show_devtools: bool,
    dev_pane: DevPane,
    js_console_text: String,
    js_input_text: String,

    // Theme
    is_dark_theme: bool,

    // Release-supported productivity state
    vertical_tabs: bool,
    task_manager: crate::task_manager::TaskManager,
    tab_search_open: bool,
    tab_search_query: String,
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
    ReplaceUrl,
    OpenFileDialog,
    LocalFilePicked(Option<std::path::PathBuf>),

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

    YouTubeSearchLoaded {
        result: Result<crate::youtube::YouTubeShell, String>,
        query: String,
        tab_id: usize,
        seq: u64,
    },
    #[cfg(target_os = "windows")]
    YouTubePlaybackPrepared {
        result: Result<crate::youtube::LiveYouTubePlayback, String>,
        tab_id: usize,
        seq: u64,
    },
    #[cfg(target_os = "windows")]
    YouTubePlaybackTick,
    #[cfg(target_os = "windows")]
    YouTubeTogglePlayback,
    #[cfg(target_os = "windows")]
    YouTubeSeekBy(f64),
    #[cfg(target_os = "windows")]
    YouTubeSetVolume(f64),
    #[cfg(target_os = "windows")]
    YouTubeToggleMute,
    #[cfg(target_os = "windows")]
    YouTubeRecover,

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
    PinTab(usize),
    ToggleMuteTab(usize),
    ToggleTabGroup(usize),
    MoveTabLeft(usize),
    MoveTabRight(usize),

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
    DownloadFinished {
        result: Result<crate::storage::DownloadRecord, String>,
        record_in_history: bool,
    },
    ClearDownloads,

    // Settings page
    SetThemeDark(bool),
    SetSearchEngine(String),
    HomepageChanged(String),
    ClearBrowsingData,
    SetPixelRendering(bool),
    SetMemorySaver(bool),
    SetMemoryPressure(bool),

    // DevTools
    ToggleDevTools,
    SetDevPane(DevPane),
    JsCodeChanged(String),
    ExecuteJs,

    // Misc
    ToggleTheme,
    EscapePressed,

    // Release-supported productivity controls
    ToggleVerticalTabs,
    ToggleAdBlock,
    ToggleAdBlockForSite,
    ToggleTaskManager,
    ToggleTabSearch,
    TabSearchQueryChanged(String),
    /// Async image loading completed — rebuild display list to show loaded images
    ImagesLoaded {
        images: Vec<crate::image_loader::ImageData>,
        tab_id: usize,
        seq: u64,
    },

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

    // ===== v1.2.0 Messages =====
    /// Timer tick for Memory Saver — checks if any inactive tab should sleep.
    MemorySaverTick,
    /// Advance the persistent JavaScript event loop for the active tab.
    PageRuntimeTick,
    /// Wake a sleeping tab (user clicked on it).
    WakeTab(usize),
    /// Timer tick for Memory Pressure monitor — checks if memory usage is too high.
    MemoryPressureTick,
}

impl Application for GhitaBrowserApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = Option<String>;

    fn new(initial_target: Self::Flags) -> (Self, Command<Message>) {
        let mut browser = Browser::new();
        if let Err(error) = browser.initialize_process_architecture() {
            log::warn!("Native process architecture unavailable: {error}");
        }
        let restored_tabs = browser.restore_previous_session();
        let restored_target = browser.active_tab().map(|tab| tab.url.clone());

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
            load_cancellations: HashMap::new(),
            #[cfg(target_os = "windows")]
            youtube_playback: None,
            display_list: Arc::new(DisplayList::default()),
            page_image_handles: Arc::new(HashMap::new()),
            canvas_cache: canvas::Cache::new(),
            show_devtools: false,
            dev_pane: DevPane::Console,
            js_console_text: String::new(),
            js_input_text: String::new(),
            is_dark_theme,

            // Release-supported productivity fields
            vertical_tabs: settings.vertical_tabs,
            task_manager: crate::task_manager::TaskManager::new(),
            tab_search_open: false,
            tab_search_query: String::new(),
        };

        if restored_tabs == 0 {
            app.open_internal("ghita://newtab", true);
        }

        let startup = initial_target
            .map(|input| {
                let target = app.resolve_omnibox(&input);
                app.navigate(target)
            })
            .or_else(|| restored_target.map(|target| app.navigate(target)))
            .unwrap_or_else(Command::none);

        // Start with keyboard focus in the omnibox so typing works immediately.
        (
            app,
            Command::batch([startup, Command::perform(async {}, |_| Message::FocusUrl)]),
        )
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
                let active_url = self
                    .browser
                    .active_tab()
                    .map(|tab| tab.url.as_str())
                    .unwrap_or_default();
                let url = normalize_omnibox_replacement(active_url, url);
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
                return self.after_tab_change("Navigated back");
            }
            Message::GoForward => {
                self.browser.go_forward();
                self.invalidate_active_tab_loads();
                return self.after_tab_change("Navigated forward");
            }
            Message::Reload => {
                if let Some(tab) = self.browser.active_tab() {
                    let url = tab.url.clone();
                    if url.starts_with("ghita://search") {
                        return self.start_search(&url);
                    }
                    if url.starts_with("http://")
                        || url.starts_with("https://")
                        || url.starts_with("file://")
                    {
                        return self.start_fetch(url);
                    }
                    return self.after_tab_change("Reloaded");
                }
            }
            Message::Home => {
                let home = self.browser.storage.settings.homepage.clone();
                return self.navigate(home);
            }
            Message::FocusUrl => {
                self.show_menu = false;
                return text_input::focus(text_input::Id::new(OMNIBOX_ID));
            }
            Message::ReplaceUrl => {
                // iced 0.12 does not reliably order focus and select-all
                // commands after a custom page widget has handled input. Clear
                // the edit buffer deterministically; the tab URL itself is
                // unchanged and Escape restores it below.
                self.show_menu = false;
                self.show_suggestions = false;
                self.url_input.clear();
                return text_input::focus(text_input::Id::new(OMNIBOX_ID));
            }
            Message::OpenFileDialog => {
                self.show_menu = false;
                return Command::perform(
                    async {
                        tokio::task::spawn_blocking(pick_local_document)
                            .await
                            .ok()
                            .flatten()
                    },
                    Message::LocalFilePicked,
                );
            }
            Message::LocalFilePicked(path) => {
                if let Some(path) = path {
                    match crate::local_file::url_from_path(&path) {
                        Ok(url) => return self.navigate(url),
                        Err(error) => self.status_msg = error,
                    }
                }
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

                // Check if tab still exists (may have been closed during async operation)
                if self.browser.tabs.get_tab(tab_id).is_none() {
                    self.load_cancellations.remove(&tab_id);
                    return Command::none();
                }

                self.load_cancellations.remove(&tab_id);

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

                // Check if tab still exists (may have been closed during async operation)
                if self.browser.tabs.get_tab(tab_id).is_none() {
                    self.load_cancellations.remove(&tab_id);
                    return Command::none();
                }

                self.load_cancellations.remove(&tab_id);

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
            Message::YouTubeSearchLoaded {
                result,
                query,
                tab_id,
                seq,
            } => {
                if self.pending_loads.get(&tab_id) != Some(&seq)
                    || self.browser.tabs.get_tab(tab_id).is_none()
                {
                    return Command::none();
                }
                self.load_cancellations.remove(&tab_id);
                self.is_loading = false;
                match result {
                    Ok(shell) => {
                        let page_url = format!(
                            "https://www.youtube.com/results?search_query={}",
                            url::form_urlencoded::byte_serialize(query.as_bytes())
                                .collect::<String>()
                        );
                        let html = build_youtube_shell_from_model(
                            &page_url,
                            &shell,
                            "Official YouTube search data loaded through the bounded Rust adapter.",
                        );
                        if self.browser.tabs.active_tab_id() == Some(tab_id) {
                            match self.browser.load_html(&page_url, &html) {
                                Ok(rendered) => {
                                    self.rendered_content = rendered;
                                    self.rebuild_display_list();
                                    self.url_input = page_url;
                                    self.status_msg = format!(
                                        "YouTube search loaded: {} result(s)",
                                        shell.results.len()
                                    );
                                }
                                Err(error) => self.status_msg = error,
                            }
                        }
                    }
                    Err(error) => {
                        self.status_msg = format!("YouTube search failed: {error}");
                    }
                }
            }
            #[cfg(target_os = "windows")]
            Message::YouTubePlaybackPrepared {
                result,
                tab_id,
                seq,
            } => {
                if self.pending_loads.get(&tab_id) != Some(&seq)
                    || self.browser.tabs.get_tab(tab_id).is_none()
                {
                    return Command::none();
                }
                self.load_cancellations.remove(&tab_id);
                self.is_loading = false;
                match result {
                    Ok(prepared) => {
                        let downloaded_bytes = prepared.downloaded_bytes;
                        let Some((sample_rate_hz, channels)) = prepared.audio_format() else {
                            self.status_msg =
                                "YouTube playback has no decoded audio format".to_string();
                            return Command::none();
                        };
                        let title = prepared.response.title.clone();
                        let video_id = prepared.response.video_id.clone();
                        let controller = match crate::youtube::LiveYouTubeController::new(prepared)
                        {
                            Ok(controller) => controller,
                            Err(error) => {
                                self.status_msg = format!("YouTube playback failed: {error}");
                                return Command::none();
                            }
                        };
                        let audio_sink = match crate::audio_output::WindowsWasapiSink::open(
                            sample_rate_hz,
                            channels,
                        ) {
                            Ok(sink) => sink,
                            Err(error) => {
                                self.status_msg = format!("YouTube audio failed: {error}");
                                return Command::none();
                            }
                        };
                        let player_html = format!(
                            "<html><head><title>{title}</title></head><body><h1>{title}</h1>\
                             <p>Live YouTube playback is ready in GhitaBrowser.</p>\
                             <p>Video ID: {video_id}</p></body></html>",
                            title = html_escape(&title),
                            video_id = html_escape(&video_id),
                        );
                        let page_url = format!("https://www.youtube.com/watch?v={video_id}");
                        if self.browser.tabs.active_tab_id() == Some(tab_id) {
                            if let Ok(rendered) = self.browser.load_html(&page_url, &player_html) {
                                self.rendered_content = rendered;
                                self.rebuild_display_list();
                                self.url_input = page_url;
                            }
                        }
                        self.youtube_playback = Some(LiveYouTubeUiPlayback {
                            tab_id,
                            controller,
                            audio_sink,
                            frame_handle: None,
                            downloaded_bytes,
                        });
                        self.status_msg = format!(
                            "YouTube ready: {title} ({:.1} MB)",
                            downloaded_bytes as f64 / (1024.0 * 1024.0)
                        );
                    }
                    Err(error) => {
                        self.youtube_playback = None;
                        self.status_msg = format!("YouTube playback failed: {error}");
                    }
                }
            }
            #[cfg(target_os = "windows")]
            Message::YouTubePlaybackTick => {
                let Some(playback) = self.youtube_playback.as_mut() else {
                    return Command::none();
                };
                if self.browser.tabs.active_tab_id() != Some(playback.tab_id) {
                    let _ = playback.audio_sink.pause();
                    return Command::none();
                }
                match playback.controller.tick(33) {
                    Ok(tick) => {
                        if tick.video_frame_presented {
                            if let Some(frame) = playback.controller.current_video_frame() {
                                playback.frame_handle =
                                    Some(iced::widget::image::Handle::from_pixels(
                                        frame.width,
                                        frame.height,
                                        frame.rgba.clone(),
                                    ));
                            }
                        }
                        for frame in playback.controller.drain_audio_frames() {
                            if let Err(error) = playback.audio_sink.enqueue(frame) {
                                self.status_msg = format!("YouTube audio interrupted: {error}");
                                break;
                            }
                        }
                        let _ = playback.audio_sink.pump();
                    }
                    Err(error) => {
                        self.status_msg = format!("YouTube playback interrupted: {error}");
                    }
                }
            }
            #[cfg(target_os = "windows")]
            Message::YouTubeTogglePlayback => {
                if let Some(playback) = self.youtube_playback.as_mut() {
                    match playback.controller.toggle_playback() {
                        Ok(true) => {
                            let _ = playback.audio_sink.resume();
                            self.status_msg = "YouTube playing".to_string();
                        }
                        Ok(false) => {
                            let _ = playback.audio_sink.pause();
                            self.status_msg = "YouTube paused".to_string();
                        }
                        Err(error) => self.status_msg = format!("YouTube control failed: {error}"),
                    }
                }
            }
            #[cfg(target_os = "windows")]
            Message::YouTubeSeekBy(seconds) => {
                if let Some(playback) = self.youtube_playback.as_mut() {
                    let _ = playback.audio_sink.flush();
                    match playback.controller.seek_by(seconds) {
                        Ok(()) => {
                            self.status_msg = format!(
                                "YouTube seek: {:.1}s",
                                playback.controller.controls().current_time_seconds
                            )
                        }
                        Err(error) => self.status_msg = format!("YouTube seek failed: {error}"),
                    }
                }
            }
            #[cfg(target_os = "windows")]
            Message::YouTubeSetVolume(volume) => {
                if let Some(playback) = self.youtube_playback.as_mut() {
                    match playback.controller.set_volume(volume) {
                        Ok(()) => {
                            self.status_msg = format!("YouTube volume: {:.0}%", volume * 100.0)
                        }
                        Err(error) => self.status_msg = format!("YouTube volume failed: {error}"),
                    }
                }
            }
            #[cfg(target_os = "windows")]
            Message::YouTubeToggleMute => {
                if let Some(playback) = self.youtube_playback.as_mut() {
                    playback.controller.toggle_mute();
                    self.status_msg = if playback.controller.controls().muted {
                        "YouTube muted".to_string()
                    } else {
                        "YouTube unmuted".to_string()
                    };
                }
            }
            #[cfg(target_os = "windows")]
            Message::YouTubeRecover => {
                if let Some(playback) = self.youtube_playback.as_mut() {
                    let _ = playback.audio_sink.flush();
                    match playback.controller.recover_after_interruption() {
                        Ok(()) => self.status_msg = "YouTube playback recovered".to_string(),
                        Err(error) => self.status_msg = format!("YouTube recovery failed: {error}"),
                    }
                }
            }
            Message::SelectTab(index) => {
                self.tab_search_open = false;
                // Check if the tab is sleeping or discarded and needs to be restored
                // Use atomic operations to avoid race conditions
                if let Some(tab) = self.browser.tabs.get_tab_by_index(index) {
                    let tab_id = tab.id;

                    // Check if tab is sleeping
                    if tab.is_sleeping {
                        if let Some(t) = self.browser.tabs.get_tab_mut(tab_id) {
                            match t.wake() {
                                crate::tab::WakeResult::NeedsReload(url) => {
                                    self.browser.tabs.set_active_by_index(index);
                                    let _ = self.after_tab_change("Tab waking…");
                                    return self.start_fetch(url);
                                }
                                crate::tab::WakeResult::RestoredFromCache => {
                                    self.browser.tabs.set_active_by_index(index);
                                    self.sync_from_active_tab();
                                    return self.after_tab_change("Tab restored from cache");
                                }
                                crate::tab::WakeResult::NotSleeping => {}
                            }
                        }
                    }
                    // Check if tab is discarded
                    else if tab.is_discarded {
                        if let Some(t) = self.browser.tabs.get_tab_mut(tab_id) {
                            if let Some(url) = t.undiscard() {
                                self.browser.tabs.set_active_by_index(index);
                                let _ = self.after_tab_change("Tab waking…");
                                return self.start_fetch(url);
                            }
                        }
                    }
                }

                // Tab is not sleeping/discarded - activate normally
                self.browser.tabs.set_active_by_index(index);
                return self.after_tab_change("");
            }
            Message::PageRuntimeTick => {
                let active_id = self.browser.tabs.active_tab_id();
                let mut runtime_error = None;
                let mut repaint = false;

                if let Some(tab_id) = active_id {
                    if let Some(tab) = self.browser.tabs.get_tab_mut(tab_id) {
                        if !tab.is_sleeping && !tab.is_discarded && tab.runtime.is_some() {
                            match tab.pump_runtime(33) {
                                Ok(processed) if processed > 0 => {
                                    if let Some(runtime) = tab.runtime.as_mut() {
                                        let render = runtime.refresh_render().clone();
                                        tab.dom = render.dom;
                                        tab.layout = render.layout;
                                        repaint = true;
                                    }
                                }
                                Ok(_) => {}
                                Err(error) => runtime_error = Some(error),
                            }
                        }
                    }
                }

                if repaint {
                    self.rendered_content = self.browser.render_current();
                    self.rebuild_display_list();
                }
                if let Some(error) = runtime_error {
                    self.status_msg = format!("Page runtime error: {error}");
                }
            }
            Message::MemorySaverTick => {
                // Check settings at tick time (user may have changed them)
                let settings = &self.browser.storage.settings;
                if settings.tab_memory_saver && settings.memory_saver_threshold_minutes > 0 {
                    let threshold = settings.memory_saver_threshold_minutes;
                    // Sleep delay: 2 seconds grace period (avoid flash on quick tab switch)
                    if let Some(slept_id) = self.browser.maybe_sleep_inactive_tab(threshold, 2) {
                        #[cfg(target_os = "windows")]
                        self.teardown_youtube_playback_for_tab(slept_id);
                        let tab_title = self
                            .browser
                            .tabs
                            .get_tab(slept_id)
                            .map(|t| t.title.clone())
                            .unwrap_or_default();
                        self.status_msg = format!("Zzz Tab put to sleep: {}", tab_title);
                    }
                }

                // Evict stale images (not accessed in 5 minutes)
                let evicted = self
                    .browser
                    .image_cache
                    .evict_stale(std::time::Duration::from_secs(300));
                if evicted > 0 {
                    log::info!("Evicted {} stale images from cache", evicted);
                }
            }
            Message::WakeTab(index) => {
                if let Some(tab) = self.browser.tabs.get_tab_by_index(index) {
                    let tab_id = tab.id;
                    match self.browser.wake_tab(tab_id) {
                        crate::tab::WakeResult::NeedsReload(url) => {
                            let _ = self.after_tab_change("Tab waking…");
                            return self.start_fetch(url);
                        }
                        crate::tab::WakeResult::RestoredFromCache => {
                            self.sync_from_active_tab();
                            return self.after_tab_change("Tab restored from cache");
                        }
                        crate::tab::WakeResult::NotSleeping => {}
                    }
                }
            }
            Message::MemoryPressureTick => {
                // Check settings at tick time
                let settings = &self.browser.storage.settings;
                if settings.memory_pressure_threshold_mb > 0 {
                    let threshold = settings.memory_pressure_threshold_mb;
                    // Keep at least 2 tabs alive (active + one spare)
                    if let Some(discarded_id) = self.browser.check_memory_pressure(threshold, 2) {
                        let tab_title = self
                            .browser
                            .tabs
                            .get_tab(discarded_id)
                            .map(|t| t.title.clone())
                            .unwrap_or_default();
                        self.status_msg = format!("X Tab discarded (memory): {}", tab_title);
                    }
                }
            }
            Message::ImagesLoaded {
                images,
                tab_id,
                seq,
            } => {
                // A batch from a superseded load (the user navigated while
                // images were in flight) must not touch loading state, the
                // status bar, or rebuild the display list of whatever tab is
                // now active. Decoded pixels are still useful — cache them
                // silently for whichever tab ends up rendering the URL.
                if self.pending_loads.get(&tab_id) != Some(&seq) {
                    // Still cache the decoded pixels (harmless, may repaint
                    // later), but touch no UI state.
                    for data in images {
                        self.browser.image_cache.add(
                            data.url.clone(),
                            crate::image_loader::Image::new(&data.url, data.width, data.height)
                                .with_alt(""),
                        );
                        self.browser
                            .image_cache
                            .insert_decoded(data.url.clone(), std::sync::Arc::new(data));
                    }
                    return Command::none();
                }
                let batch_is_active = self.browser.tabs.active_tab_id() == Some(tab_id);

                // Cache pixels for the originating tab (kept even when the
                // batch's tab is only background — it will repaint on switch).
                for data in images {
                    self.browser.image_cache.add(
                        data.url.clone(),
                        crate::image_loader::Image::new(&data.url, data.width, data.height)
                            .with_alt(""),
                    );
                    let arc_data = std::sync::Arc::new(data);
                    self.browser
                        .image_cache
                        .insert_decoded(arc_data.url.clone(), arc_data);
                }

                // Only refresh paint/state when the batch belongs to the
                // tab the user is looking at.
                if batch_is_active {
                    self.rebuild_display_list();
                    self.is_loading = false;
                    self.status_msg = "Images loaded".to_string();
                }
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
                    if let Some(cancellation) = self.load_cancellations.remove(&id) {
                        cancellation.cancel();
                    }
                    #[cfg(target_os = "windows")]
                    self.teardown_youtube_playback_for_tab(id);
                    self.browser.tabs.remove_tab(id);
                    self.task_manager.tasks.retain(|task| task.tab_id != id);
                    self.search_state.remove(&id);
                    self.pending_loads.remove(&id);
                    self.ensure_tab();
                    return self.after_tab_change("Tab closed");
                }
            }
            Message::CloseCurrentTab => {
                if let Some(tab) = self.browser.active_tab() {
                    let id = tab.id;
                    if let Some(cancellation) = self.load_cancellations.remove(&id) {
                        cancellation.cancel();
                    }
                    #[cfg(target_os = "windows")]
                    self.teardown_youtube_playback_for_tab(id);
                    self.browser.tabs.remove_tab(id);
                    self.task_manager.tasks.retain(|task| task.tab_id != id);
                    self.search_state.remove(&id);
                    self.pending_loads.remove(&id);
                    self.ensure_tab();
                    return self.after_tab_change("Tab closed");
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
                return self.after_tab_change("");
            }
            Message::PrevTab => {
                self.browser.tabs.activate_prev();
                return self.after_tab_change("");
            }
            Message::SelectTabNumber(n) => {
                let count = self.browser.tab_count();
                if count > 0 {
                    // Ctrl+9 selects the last tab, like Chrome
                    let idx = if n >= count { count - 1 } else { n };
                    self.browser.tabs.set_active_by_index(idx);
                    return self.after_tab_change("");
                }
            }
            Message::PinTab(index) => {
                let pinned = self
                    .browser
                    .tabs
                    .get_tab_by_index(index)
                    .is_some_and(|tab| tab.is_pinned);
                if self.browser.tabs.pin_tab_by_index(index, !pinned) {
                    self.browser.persist_session();
                    self.status_msg = if pinned {
                        "Tab unpinned".to_string()
                    } else {
                        "Tab pinned".to_string()
                    };
                }
            }
            Message::ToggleMuteTab(index) => {
                if let Some(muted) = self.browser.tabs.toggle_mute_by_index(index) {
                    self.browser.persist_session();
                    self.status_msg = if muted {
                        "Tab muted".to_string()
                    } else {
                        "Tab unmuted".to_string()
                    };
                }
            }
            Message::ToggleTabGroup(index) => {
                let current = self
                    .browser
                    .tabs
                    .get_tab_by_index(index)
                    .and_then(|tab| tab.group_id);
                let target = if current.is_some() {
                    None
                } else {
                    self.browser
                        .tabs
                        .groups()
                        .keys()
                        .next()
                        .copied()
                        .or_else(|| self.browser.tabs.create_group("Group 1", "#4f8cff").ok())
                };
                if self
                    .browser
                    .tabs
                    .assign_tab_to_group_by_index(index, target)
                {
                    self.browser.persist_session();
                    self.status_msg = if target.is_some() {
                        "Tab added to group".to_string()
                    } else {
                        "Tab removed from group".to_string()
                    };
                }
            }
            Message::MoveTabLeft(index) => {
                if index > 0 && self.browser.tabs.reorder_tab(index, index - 1) {
                    self.browser.persist_session();
                }
            }
            Message::MoveTabRight(index) => {
                if index + 1 < self.browser.tab_count()
                    && self.browser.tabs.reorder_tab(index, index + 1)
                {
                    self.browser.persist_session();
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
                let record_in_history = self
                    .browser
                    .active_tab()
                    .map(|tab| !tab.incognito)
                    .unwrap_or(true);
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    self.status_msg = "Only web pages can be downloaded".to_string();
                    return Command::none();
                }
                self.status_msg = format!("Downloading {}...", url);
                return Command::perform(
                    async move {
                        let (bytes, name, _ct) = crate::network::download_url_async(&url).await?;
                        tokio::task::spawn_blocking(move || {
                            // Sanitize: keep only the final path component so a malicious
                            // Content-Disposition can't traverse dirs or write an absolute path.
                            let name = crate::ui_helpers::sanitize_download_filename(&name);
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
                    move |result| Message::DownloadFinished {
                        result,
                        record_in_history,
                    },
                );
            }
            Message::DownloadFinished {
                result,
                record_in_history,
            } => match result {
                Ok(rec) => {
                    self.status_msg = format!(
                        "Downloaded {} ({})",
                        rec.file_name,
                        fmt_bytes(rec.size_bytes)
                    );
                    if record_in_history {
                        self.browser.storage.add_download(rec);
                    }
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
            Message::SetMemorySaver(on) => {
                self.browser.storage.settings.tab_memory_saver = on;
                if on && self.browser.storage.settings.memory_saver_threshold_minutes == 0 {
                    self.browser.storage.settings.memory_saver_threshold_minutes = 5;
                }
                self.status_msg = if on {
                    "Memory Saver enabled".to_string()
                } else {
                    "Memory Saver disabled".to_string()
                };
            }
            Message::SetMemoryPressure(on) => {
                self.browser.storage.settings.memory_pressure_threshold_mb =
                    if on { 500 } else { 0 };
                self.status_msg = if on {
                    "Memory pressure protection enabled at 500 MB".to_string()
                } else {
                    "Memory pressure protection disabled".to_string()
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
                self.browser.storage.clear_local_storage();
                self.browser.cache.clear();
                self.browser.image_cache.clear();
                self.status_msg =
                    "Browsing data cleared (history, cookies, site data and caches)".to_string();
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
                    let mut repaint_page = false;
                    let result = if let Some(tab) = self.browser.tabs.active_tab_mut() {
                        if tab.runtime.is_some() {
                            let result = tab.evaluate_js(&code);
                            if let Some(runtime) = tab.runtime.as_mut() {
                                let render = runtime.refresh_render().clone();
                                tab.dom = render.dom;
                                tab.layout = render.layout;
                                repaint_page = true;
                            }
                            result
                        } else {
                            self.browser.js_engine.execute_script(&code)
                        }
                    } else {
                        self.browser.js_engine.execute_script(&code)
                    };

                    match result {
                        Ok(val) => {
                            let output = val.to_display_string();
                            let line = format!("> {} = {}", code, output);
                            self.browser.js_engine.console_output.push(line);
                            // Keep the DevTools console bounded (500 lines);
                            // drain overflow like the engine itself does.
                            let co = &mut self.browser.js_engine.console_output;
                            if co.len() > 500 {
                                let overflow = co.len() - 500;
                                co.drain(0..overflow);
                            }
                            self.status_msg = format!("JS: {} = {}", code, output);
                        }
                        Err(e) => {
                            let line = format!("> {}  // Error: {}", code, e);
                            self.browser.js_engine.console_output.push(line);
                            let co = &mut self.browser.js_engine.console_output;
                            if co.len() > 500 {
                                let overflow = co.len() - 500;
                                co.drain(0..overflow);
                            }
                            self.status_msg = format!("JS Error: {}", e);
                        }
                    }
                    if repaint_page {
                        self.rendered_content = self.browser.render_current();
                        self.rebuild_display_list();
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
                } else if self.tab_search_open {
                    self.tab_search_open = false;
                } else if self.task_manager.open {
                    self.task_manager.open = false;
                } else if let Some(url) = self.browser.active_tab().map(|tab| tab.url.clone()) {
                    if self.url_input != url {
                        self.url_input = if is_blank_page(&url) {
                            String::new()
                        } else {
                            url
                        };
                        self.show_suggestions = false;
                    }
                }
            }
            Message::ToggleVerticalTabs => {
                self.vertical_tabs = !self.vertical_tabs;
                self.browser.storage.settings.vertical_tabs = self.vertical_tabs;
                self.status_msg = if self.vertical_tabs {
                    "Vertical tabs enabled (Edge-style)".to_string()
                } else {
                    "Horizontal tab bar enabled".to_string()
                };
            }
            Message::ToggleAdBlock => {
                let on = !self.browser.adblocker.config().enabled;
                let mut config = self.browser.adblocker.config().clone();
                config.enabled = on;
                self.browser.adblocker = crate::adblock::AdBlocker::new(config);
                self.browser.storage.settings.adblock_enabled = on;
                self.status_msg = if on {
                    "AdBlock & Tracker Blocker enabled".to_string()
                } else {
                    "AdBlock disabled".to_string()
                };
            }
            Message::ToggleAdBlockForSite => {
                let domain = self
                    .browser
                    .active_tab()
                    .and_then(|tab| crate::ui_helpers::host(&tab.url));
                if let Some(domain) = domain {
                    let enabled = self.browser.adblocker.toggle_domain(domain.clone());
                    self.browser.storage.settings.adblock_disabled_domains =
                        self.browser.adblocker.config().disabled_domains.clone();
                    self.status_msg = if enabled {
                        format!("Request blocker enabled for {domain}; reload to apply")
                    } else {
                        format!("Request blocker disabled for {domain}; reload to apply")
                    };
                } else {
                    self.status_msg = "Per-site filtering is available on web pages".to_string();
                }
            }
            Message::ToggleTaskManager => {
                self.task_manager.toggle();
                if self.task_manager.open {
                    let mut infos = Vec::new();
                    let estimate = self.browser.estimate_memory();
                    for (idx, tab) in self.browser.tabs.iter().enumerate() {
                        // Use real memory estimate from MemoryTracker
                        let memory_mb = estimate
                            .tabs
                            .get(idx)
                            .map(|e| {
                                crate::memory_tracker::MemoryTracker::bytes_to_mb(e.total_bytes)
                            })
                            .unwrap_or(0.0);
                        // Count real layout nodes if layout exists
                        let layout_nodes = tab
                            .layout
                            .as_ref()
                            .map(crate::layout::count_layout_nodes)
                            .unwrap_or_else(|| crate::count_elements(&tab.dom));
                        infos.push(crate::task_manager::ProcessTaskInfo {
                            tab_id: tab.id,
                            title: tab.title.clone(),
                            url: tab.url.clone(),
                            memory_mb,
                            cpu_percent: 0.0, // CPU tracking not yet implemented
                            layout_nodes,
                            is_incognito: tab.incognito,
                        });
                    }
                    self.task_manager.update_tasks(infos);
                }
            }
            Message::ToggleTabSearch => {
                self.tab_search_open = !self.tab_search_open;
                self.tab_search_query.clear();
            }
            Message::TabSearchQueryChanged(q) => {
                self.tab_search_query = q;
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

                // Check if tab still exists (may have been closed during async operation)
                if self.browser.tabs.get_tab(tab_id).is_none() {
                    self.load_cancellations.remove(&tab_id);
                    return Command::none();
                }

                self.load_cancellations.remove(&tab_id);

                let url = result.url.clone();
                let is_pdf = result.binary_body.is_some();
                let html = if is_pdf {
                    "<!doctype html><title>PDF document</title>".to_string()
                } else {
                    result.body.clone()
                };
                let fetch_time = result.fetch_time_ms;
                let incognito = self
                    .browser
                    .tabs
                    .get_tab(tab_id)
                    .map(|tab| tab.incognito)
                    .unwrap_or(false);

                // Warm the resource cache so repeated visits reuse this response.
                // Responses with a `Vary` header are served differently per
                // request (cookie/user) and must not be cached under a bare
                // URL — otherwise another session/profile could be served this
                // user's stateful content (RFC 7234 §4.1).
                if !incognito && !crate::network::response_varies(&result.headers) {
                    self.browser.cache.insert(
                        &url,
                        result.clone(),
                        crate::network::cache_ttl_secs(&result.headers),
                    );
                }

                // Persist Set-Cookie headers from the response into the jar,
                // so subsequent requests to the same host send the cookies
                if !incognito {
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
                }

                self.is_loading = true;
                self.status_msg = format!("Parsing {}...", url);

                let mut document_rules = self.browser.css_rules.clone();
                let cosmetic_css = self
                    .browser
                    .adblocker
                    .cosmetic_selectors(&url)
                    .into_iter()
                    .map(|selector| format!("{selector} {{ display: none; }}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                document_rules.extend(crate::css_parser::parse_css(&cosmetic_css));
                let content_control_css = self
                    .browser
                    .content_control
                    .generate_cosmetic_css_for_origin(&url);
                document_rules.extend(crate::css_parser::parse_css(&content_control_css));
                let prepared_result = if let Some(pdf_bytes) = result.binary_body.as_deref() {
                    crate::worker::prepare_pdf_isolated(
                        pdf_bytes,
                        &url,
                        &document_rules,
                        self.browser.viewport_width(),
                        self.browser.viewport_height(),
                    )
                } else {
                    crate::worker::prepare_document_isolated(
                        &html,
                        &url,
                        &document_rules,
                        self.browser.viewport_width(),
                        self.browser.viewport_height(),
                    )
                };
                let prepared = match prepared_result {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        return self.update(Message::LoadError {
                            err: format!("Document worker failed: {error}"),
                            url,
                            tab_id,
                            seq,
                        });
                    }
                };
                let stats = prepared.stats.clone();
                let parse_time = stats.parse_time_ms;
                let style_time = stats.style_time_ms;
                let layout_time = stats.layout_time_ms;
                let render_time = stats.render_time_ms;
                let total_time = stats.total_time_ms;
                let dom_nodes = stats.dom_nodes;
                for (phase, duration) in [
                    ("fetch", fetch_time),
                    ("parse", parse_time),
                    ("style", style_time),
                    ("layout", layout_time),
                    ("render", render_time),
                    ("document_total", total_time),
                ] {
                    self.browser.profiler.record(phase, duration);
                }
                let estimated_document_bytes = result
                    .binary_body
                    .as_ref()
                    .map(Vec::len)
                    .unwrap_or(html.len())
                    .saturating_add(dom_nodes.saturating_mul(512));
                let performance_evaluation = crate::performance::PerformanceBudget::default()
                    .evaluate(crate::performance::NavigationMetrics {
                        fetch_ms: fetch_time,
                        parse_ms: parse_time,
                        style_ms: style_time,
                        layout_ms: layout_time,
                        render_ms: render_time,
                        total_ms: total_time,
                        dom_nodes,
                        estimated_memory_bytes: estimated_document_bytes,
                    });
                let mut runtime = prepared.runtime;
                let mut dom = prepared.dom;
                let mut title = prepared.title;
                let mut layout_tree = prepared.layout;
                let mut rendered = prepared.rendered_text;
                let mut page_runtime = None;
                let is_youtube_route = crate::youtube::YouTubeRoute::parse(&url).is_ok();

                // The renderer worker intentionally returns an inert DOM. A
                // persistent runtime belongs to the tab/UI process so event
                // listeners, closures, timers and queued jobs survive after
                // the initial page load instead of being discarded with the
                // short-lived worker process.
                if !is_pdf && !is_youtube_route {
                    let mut runtime_rules = document_rules;
                    for style in dom.find_all_tags("style") {
                        if !style.text.trim().is_empty() {
                            runtime_rules.extend(crate::css_parser::parse_css_with_media(
                                style.text.trim(),
                                self.browser.viewport_width(),
                            ));
                        }
                    }
                    let storage_dir = if incognito {
                        None
                    } else {
                        self.browser.storage.storage_dir().cloned()
                    };
                    match crate::web_runtime::PageRuntime::from_element_with_storage_dir(
                        &dom,
                        runtime_rules,
                        self.browser.viewport_width(),
                        &url,
                        storage_dir.as_ref(),
                    ) {
                        Ok(mut persistent_runtime) => {
                            if let Err(error) = persistent_runtime.run_document() {
                                self.browser
                                    .js_engine
                                    .console_output
                                    .push(format!("Page runtime startup error: {error}"));
                            }
                            let render = persistent_runtime.refresh_render().clone();
                            dom = render.dom;
                            layout_tree = render.layout;
                            rendered = layout_tree
                                .as_ref()
                                .map(|root| {
                                    crate::text_renderer::TextRenderer::new(
                                        self.browser.viewport_width(),
                                        self.browser.viewport_height(),
                                    )
                                    .render_to_text(root)
                                })
                                .unwrap_or_else(|| "[Empty page]".to_string());
                            title = dom
                                .find_tag("title")
                                .or_else(|| dom.find_tag("h1"))
                                .map(|element| element.text.trim().to_string())
                                .filter(|title| !title.is_empty())
                                .unwrap_or_else(|| url.clone());
                            runtime = persistent_runtime.report_snapshot();
                            page_runtime = Some(persistent_runtime);
                        }
                        Err(error) => {
                            self.browser
                                .js_engine
                                .console_output
                                .push(format!("Page runtime unavailable: {error}"));
                        }
                    }
                }
                if !incognito {
                    if let Ok(parsed) = url::Url::parse(&url) {
                        if matches!(parsed.scheme(), "http" | "https") {
                            let origin = parsed.origin().ascii_serialization();
                            let storage = self.browser.storage.local_storage(&origin);
                            for (key, value) in &runtime.storage_writes {
                                let _ = storage.set(key, value);
                            }
                        }
                    }
                }
                for line in &runtime.console {
                    self.browser.js_engine.console_output.push(line.clone());
                }
                for error in runtime.errors.iter().take(32) {
                    self.browser
                        .js_engine
                        .console_output
                        .push(format!("Page script error: {error}"));
                }
                let console = &mut self.browser.js_engine.console_output;
                if console.len() > 500 {
                    let overflow = console.len() - 500;
                    console.drain(0..overflow);
                }
                self.browser.last_render_stats = Some(stats);

                // 7. Update the originating tab (may differ from the active tab now)
                let target_is_active = self.browser.tabs.active_tab_id() == Some(tab_id);
                if let Some(tab) = self.browser.tabs.get_tab_mut(tab_id) {
                    // A sleeping/discarded tab that receives fresh content must
                    // come back to the FULLY awake state: keep the new DOM
                    // (not the stale compressed snapshot) and clear the
                    // hibernation flags, or the tab would render asleep with
                    // a live DOM and later "wake" over it with old content.
                    if tab.is_sleeping || tab.is_discarded {
                        tab.compressed_dom = None;
                        tab.is_sleeping = false;
                        tab.is_discarded = false;
                        tab.slept_at = None;
                    }
                    // Record the freshly loaded page in session history.
                    // push_history dedups consecutive same-URL loads (reloads
                    // and duplicate notifications), and error pages never
                    // enter history (see Tab::go_back's is_error handling).
                    let loaded_entry =
                        crate::tab::HistoryEntry::new(url.clone(), title.clone(), &dom);
                    tab.push_history(loaded_entry);
                    tab.is_error = false;
                    tab.dom = dom;
                    tab.title = title.clone();
                    tab.url = url.clone();
                    // Keep the fresh layout on the tab for pixel painting & tab switching
                    tab.layout = layout_tree.clone();
                    tab.runtime = page_runtime;
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
                    "Fetch: {}ms | Parse: {}ms | Style: {}ms | Layout: {}ms | Render: {}ms | Total: {}ms | {} DOM nodes | {} scripts ({} failed) | {} DOM mutations | {} runtime jobs | {} KiB realm heap | {} budget warnings",
                    fetch_time,
                    parse_time,
                    style_time,
                    layout_time,
                    render_time,
                    total_time,
                    dom_nodes,
                    runtime.scripts_executed,
                    runtime.scripts_failed,
                    runtime.dom_mutations,
                    runtime.scheduled_tasks,
                    runtime.realm_heap_bytes.div_ceil(1024),
                    performance_evaluation.violations.len()
                );
                self.status_msg = format!("Loaded {} | {}ms", url, fetch_time + total_time);

                // 9. Update the visible UI only if the loaded tab is still active
                if target_is_active {
                    // Check if display list is empty (layout failed) - try Reader Mode fallback
                    let display_list = layout_tree
                        .as_ref()
                        .map(|root| {
                            crate::paint::build_display_list_with_cache(
                                root,
                                Some(&self.browser.image_cache),
                            )
                        })
                        .unwrap_or_default();

                    // Detect SPA/JS-rendered pages (e.g., YouTube, Twitter)
                    // and show a user-friendly fallback instead of empty/skeleton content
                    let visible_metrics = crate::paint::calculate_visible_metrics(
                        &display_list,
                        self.browser.viewport_width() as f32,
                        self.browser.viewport_height() as f32,
                    );
                    let stalled_runtime = runtime.dom_mutations == 0
                        && runtime.scheduled_tasks == 0
                        && runtime.animation_frames_fired == 0;
                    let visibly_sparse = visible_metrics.visible_text_characters < 128
                        && visible_metrics.completeness_score < 0.35;
                    let (overlapping_pairs, collision_score) = layout_tree
                        .as_ref()
                        .map(crate::compatibility_diagnostics::evaluate_layout_overlap)
                        .unwrap_or((0, 0.0));
                    let broken_author_layout =
                        requires_safe_flow_layout(overlapping_pairs, collision_score);
                    let is_spa = !is_pdf
                        && is_spa_or_js_rendered(&html)
                        && (display_list.items.len() < 10
                            || visible_metrics.has_major_blank_region
                            || (stalled_runtime && visibly_sparse));
                    let sparse_content =
                        !is_spa && display_list.items.len() < 3 && html.len() > 50000;

                    if is_spa {
                        // Parse YouTube's bounded server bootstrap into the
                        // browser-owned shell before using the static fallback.
                        let page_html = if is_youtube_route {
                            if let Some(shell_html) = build_youtube_shell_html(&url, &html) {
                                self.status_msg =
                                    "YouTube shell loaded (degraded mode)".to_string();
                                shell_html
                            } else if let Some(video_id) = extract_youtube_video_id(&url) {
                                self.status_msg =
                                    format!("YouTube video {} (live player unavailable)", video_id);
                                build_video_info_html(&video_id, &url)
                            } else {
                                build_spa_fallback_html(&title, &url)
                            }
                        } else {
                            // Generic SPA fallback
                            self.status_msg = format!(
                                "JavaScript application at {} could not complete its initial render",
                                url
                            );
                            build_spa_fallback_html(&title, &url)
                        };
                        let spa_dom = parse_html(&page_html);
                        let spa_layout = crate::layout::create_layout_tree(
                            &spa_dom,
                            &self.browser.css_rules,
                            self.browser.viewport_width(),
                        );
                        let spa_list = spa_layout
                            .as_ref()
                            .map(|root| {
                                crate::paint::build_display_list_with_cache(
                                    root,
                                    Some(&self.browser.image_cache),
                                )
                            })
                            .unwrap_or_default();
                        let spa_rendered = if let Some(ref root) = spa_layout {
                            let tr = crate::text_renderer::TextRenderer::new(
                                self.browser.viewport_width(),
                                self.browser.viewport_height(),
                            );
                            tr.render_to_text(root)
                        } else {
                            String::new()
                        };
                        self.display_list = Arc::new(spa_list);
                        self.rendered_content = spa_rendered;
                    } else if broken_author_layout {
                        // Keep the page usable when unsupported author CSS
                        // produces severe text collisions. Re-lay out the
                        // already-scripted DOM with browser defaults only,
                        // preserving text, links and images in normal flow.
                        let recovery_dom =
                            self.browser.tabs.get_tab(tab_id).map(|tab| tab.dom.clone());
                        let recovery_layout = recovery_dom.as_ref().and_then(|dom| {
                            crate::layout::create_layout_tree(
                                dom,
                                &self.browser.css_rules,
                                self.browser.viewport_width(),
                            )
                        });
                        let recovery_list = recovery_layout
                            .as_ref()
                            .map(|root| {
                                crate::paint::build_display_list_with_cache(
                                    root,
                                    Some(&self.browser.image_cache),
                                )
                            })
                            .unwrap_or_default();
                        let recovery_text = recovery_layout
                            .as_ref()
                            .map(|root| {
                                crate::text_renderer::TextRenderer::new(
                                    self.browser.viewport_width(),
                                    self.browser.viewport_height(),
                                )
                                .render_to_text(root)
                            })
                            .unwrap_or_else(|| rendered.clone());
                        if let Some(tab) = self.browser.tabs.get_tab_mut(tab_id) {
                            tab.layout = recovery_layout;
                            // A runtime repaint would immediately restore the
                            // broken author layout. Compatibility mode is a
                            // bounded static representation until navigation.
                            tab.runtime = None;
                        }
                        self.status_msg = format!(
                            "Loaded in compatibility layout: {url} ({overlapping_pairs} text collisions)"
                        );
                        self.display_list = Arc::new(recovery_list);
                        self.rendered_content = recovery_text;
                    } else if display_list.is_empty() && !html.is_empty() {
                        // Display list empty - try Reader Mode fallback
                        let article =
                            crate::reader_mode::ReaderModeExtractor::extract(&html, &url, &title);
                        if !article.text_content.is_empty() {
                            self.status_msg = format!(
                                "Loaded (Reader Mode): {} | est. {} min read",
                                article.title, article.estimated_reading_time_mins
                            );
                            self.rendered_content = article.text_content.clone();
                        } else {
                            self.rendered_content = rendered;
                        }
                        self.display_list = Arc::new(display_list);
                    } else if sparse_content {
                        // Page has lots of HTML but little content (e.g., ads-only, JS skeleton)
                        // Show a notice and fall back to any readable text
                        let article =
                            crate::reader_mode::ReaderModeExtractor::extract(&html, &url, &title);
                        let notice_html = format!(
                            "<html><body>\
                             <h1>This page is mostly empty</h1>\
                             <p>The HTML was loaded ({} KB) but only minimal visible content was found.</p>\
                             <p>This often happens when the page relies heavily on JavaScript to render content.</p>\
                             <p><b>Address:</b> {}</p>\
                             {}\
                             </body></html>",
                            html.len() / 1024,
                            html_escape(&url),
                            if !article.text_content.is_empty() {
                                // Escape page text: it can contain "</pre>" and
                                // would otherwise break out of the element.
                                format!(
                                    "<h2>Extracted text:</h2><pre>{}</pre>",
                                    html_escape(&article.text_content.chars().take(2000).collect::<String>())
                                )
                            } else {
                                String::new()
                            }
                        );
                        let notice_dom = parse_html(&notice_html);
                        let notice_layout = crate::layout::create_layout_tree(
                            &notice_dom,
                            &self.browser.css_rules,
                            self.browser.viewport_width(),
                        );
                        let notice_list = notice_layout
                            .as_ref()
                            .map(|root| {
                                crate::paint::build_display_list_with_cache(
                                    root,
                                    Some(&self.browser.image_cache),
                                )
                            })
                            .unwrap_or_default();
                        let notice_rendered = if let Some(ref root) = notice_layout {
                            let tr = crate::text_renderer::TextRenderer::new(
                                self.browser.viewport_width(),
                                self.browser.viewport_height(),
                            );
                            tr.render_to_text(root)
                        } else {
                            String::new()
                        };
                        self.status_msg = format!(
                            "Limited content: {} (only {} visible items)",
                            url,
                            display_list.items.len()
                        );
                        self.display_list = Arc::new(notice_list);
                        self.rendered_content = notice_rendered;
                    } else {
                        // Normal page - render as usual
                        self.rendered_content = rendered;
                        self.display_list = Arc::new(display_list);
                    }

                    self.canvas_cache.clear();
                    self.show_suggestions = false;
                    self.url_input = url.clone();

                    // Kick off async image loading for any PendingImage items
                    return self.schedule_image_loading();
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
                self.load_cancellations.remove(&tab_id);
                self.is_loading = false;
                let (title, friendly_msg) = humanize_error(&err);
                self.status_msg = format!("Error loading {}: {}", url, err);

                // Turn the failure into an error "page" on the originating tab, so the
                // user actually sees it (even when the load started from an
                // internal page). URL is user/web-supplied: escape it so a
                // crafted URL can't inject markup into the error page.
                let safe_url = html_escape(&url);
                let safe_title = html_escape(&title);
                let error_html = format!(
                    "<html><head><title>{}</title></head>\
                     <body><h1>{}</h1>\
                     <p>{}</p>\
                     <p>Address: {}</p>\
                     <p>Try checking the address, your connection, or reload (F5).</p>\
                     </body></html>",
                    safe_title, safe_title, friendly_msg, safe_url
                );
                let dom = parse_html(&error_html);
                let target_is_active = self.browser.tabs.active_tab_id() == Some(tab_id);
                if let Some(tab) = self.browser.tabs.get_tab_mut(tab_id) {
                    // Error pages are never recorded in history; the tab keeps
                    // its current history position so Back returns to the last
                    // good page (see Tab::go_back's is_error handling).
                    tab.dom = dom;
                    tab.title = title.clone();
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

        let mut layers = column![];
        if !self.vertical_tabs {
            layers = layers.push(self.build_tab_strip(pal));
        }
        layers = layers.push(self.build_toolbar(pal));

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
        if self.tab_search_open {
            layers = layers.push(self.build_tab_search_panel(pal));
        }
        if self.task_manager.open {
            layers = layers.push(self.build_task_manager_panel(pal));
        }

        let content = self.build_content(pal);
        let content_with_tools: Element<'_, Message> = if self.show_devtools {
            row![content, self.build_devtools_panel(pal)].into()
        } else {
            content
        };
        let main: Element<'_, Message> = if self.vertical_tabs {
            row![self.build_vertical_tab_strip(pal), content_with_tools].into()
        } else {
            content_with_tools
        };

        layers = layers.push(main);
        layers = layers.push(self.build_status_bar(pal));

        Element::from(layers)
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        // Browser shortcuts must also observe events captured by the omnibox
        // or another text input. `keyboard::on_key_press` only forwards ignored
        // events, which makes Ctrl+L/F6 stop working as soon as the address bar
        // owns focus.
        let keyboard_sub = iced::event::listen_with(handle_keyboard_event);
        let mut subs: Vec<iced::Subscription<Message>> = vec![keyboard_sub];

        // Memory Saver: tick every 30 seconds to check for inactive tabs.
        let memory_saver = self.browser.storage.settings.tab_memory_saver;
        let sleep_threshold = self.browser.storage.settings.memory_saver_threshold_minutes;
        if memory_saver && sleep_threshold > 0 {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(30))
                    .map(|_| Message::MemorySaverTick),
            );
        }

        // Memory Pressure: tick every 60 seconds to check memory usage.
        let pressure_threshold = self.browser.storage.settings.memory_pressure_threshold_mb;
        if pressure_threshold > 0 {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(60))
                    .map(|_| Message::MemoryPressureTick),
            );
        }

        let active_runtime = self.browser.active_tab().is_some_and(|tab| {
            tab.runtime
                .as_ref()
                .is_some_and(crate::web_runtime::PageRuntime::needs_event_pump)
                && !tab.is_sleeping
                && !tab.is_discarded
        });
        if active_runtime {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(33))
                    .map(|_| Message::PageRuntimeTick),
            );
        }

        #[cfg(target_os = "windows")]
        if self.youtube_playback.is_some() {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(33))
                    .map(|_| Message::YouTubePlaybackTick),
            );
        }

        iced::Subscription::batch(subs)
    }
}

// ===== Keyboard Shortcuts (Chrome bindings) =====

fn handle_keyboard_event(event: iced::Event, _status: iced::event::Status) -> Option<Message> {
    match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            handle_keyboard(key, modifiers)
        }
        _ => None,
    }
}

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
            Named::F6 => return Some(Message::ReplaceUrl),
            Named::F12 => return Some(Message::ToggleDevTools),
            _ => {}
        }
    }

    if modifiers.shift() && !modifiers.control() && !modifiers.alt() {
        if let Key::Named(Named::Escape) = &key {
            return Some(Message::ToggleTaskManager);
        }
    }

    if modifiers.control() && modifiers.shift() {
        return match key {
            Key::Character(c) if c == "t" || c == "T" => Some(Message::ReopenClosedTab),
            Key::Character(c) if c == "n" || c == "N" => Some(Message::NewIncognitoTab),
            Key::Character(c) if c == "b" || c == "B" => Some(Message::ToggleBookmarksBar),
            Key::Character(c) if c == "a" || c == "A" => Some(Message::ToggleTabSearch),
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
            Key::Character(c) if c == "l" || c == "L" => Some(Message::ReplaceUrl),
            Key::Character(c) if c == "t" || c == "T" => Some(Message::NewTab),
            Key::Character(c) if c == "w" || c == "W" => Some(Message::CloseCurrentTab),
            Key::Character(c) if c == "r" || c == "R" => Some(Message::Reload),
            Key::Character(c) if c == "o" || c == "O" => Some(Message::OpenFileDialog),
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
        if let Some(local_url) = crate::local_file::resolve_local_input(input) {
            return local_url;
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
        #[cfg(target_os = "windows")]
        if let Ok(route) = crate::youtube::YouTubeRoute::parse(&target) {
            match route {
                crate::youtube::YouTubeRoute::Search { query } => {
                    return self.start_youtube_search(query);
                }
                crate::youtube::YouTubeRoute::Watch { video_id }
                | crate::youtube::YouTubeRoute::Shorts { video_id }
                | crate::youtube::YouTubeRoute::Embed { video_id } => {
                    return self.start_youtube_playback(video_id);
                }
                crate::youtube::YouTubeRoute::Playlist {
                    video_id: Some(video_id),
                    ..
                } => {
                    return self.start_youtube_playback(video_id);
                }
                _ => {}
            }
        }
        if target.starts_with("ghita://search") {
            self.open_internal(&target, false);
            return self.start_search(&target);
        }
        if target.starts_with("ghita://") || target.starts_with("about:") {
            self.open_internal(&target, false);
            Command::none()
        } else {
            let target = self.browser.secure_navigation_url(&target);
            self.start_fetch(target)
        }
    }

    /// Invalidate any fetch/search still in flight for the active tab so its
    /// late response can't overwrite a page the user explicitly navigated to
    /// (back/forward, internal pages, ...).
    fn invalidate_active_tab_loads(&mut self) {
        if let Some(tab_id) = self.browser.tabs.active_tab_id() {
            if let Some(cancellation) = self.load_cancellations.remove(&tab_id) {
                cancellation.cancel();
            }
            self.load_seq = self.load_seq.wrapping_add(1);
            let seq = self.load_seq;
            self.pending_loads.insert(tab_id, seq);
        }
    }

    /// Kick off an async network fetch — UI stays responsive
    fn start_fetch(&mut self, url: String) -> Command<Message> {
        #[cfg(target_os = "windows")]
        self.teardown_youtube_playback();
        self.is_loading = true;
        self.status_msg = format!("Loading {}...", url);
        self.url_input = url.clone();
        self.render_stats_text = String::new();
        self.show_suggestions = false;

        // Bind the load to the tab that started it, so switching tabs mid-load
        // never applies content or history to the wrong tab. The sequence
        // number lets stale responses (from a superseded navigation) be dropped.
        let tab_id = self.browser.tabs.active_tab_id().unwrap_or(0);
        if let Err(error) = self.browser.attach_navigation_process(tab_id, &url) {
            self.status_msg = format!("Renderer isolation warning: {error}");
        }
        self.load_seq = self.load_seq.wrapping_add(1);
        let seq = self.load_seq;
        self.pending_loads.insert(tab_id, seq);
        if let Some(previous) = self.load_cancellations.remove(&tab_id) {
            previous.cancel();
        }
        let cancellation = crate::network_scheduler::CancellationToken::default();
        self.load_cancellations.insert(tab_id, cancellation.clone());
        let fetch_cancellation = cancellation.clone();
        let fetch_url = url.clone();
        let err_url = url;
        let incognito = self
            .browser
            .tabs
            .get_tab(tab_id)
            .map(|tab| tab.incognito)
            .unwrap_or(false);

        // Build the Cookie header up front instead of deep-cloning the whole
        // jar (which stalls the main thread on large stores and can't see
        // cookies a concurrent in-flight response just set). Each cookie is
        // validated against the request URL (Secure over https, path match,
        // SameSite=Strict) like the blocking fetch path does.
        let cookie_header = if incognito {
            String::new()
        } else {
            // A document navigation is first-party to its own target. The
            // scheduler only forwards this header to same-origin script/style
            // resources, so it cannot leak into third-party requests.
            self.browser
                .cookie_header_for_navigation(&fetch_url, &fetch_url)
        };

        Command::perform(
            async move {
                if fetch_url.starts_with("file://") {
                    tokio::task::spawn_blocking(move || {
                        crate::local_file::fetch_local_document(&fetch_url)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("Task error: {e}")))
                } else {
                    crate::network_scheduler::fetch_document_bundle(
                        fetch_url,
                        cookie_header,
                        2,
                        fetch_cancellation,
                    )
                    .await
                }
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

    fn begin_youtube_load(
        &mut self,
        status: String,
    ) -> (usize, u64, crate::network_scheduler::CancellationToken) {
        self.is_loading = true;
        self.status_msg = status;
        let tab_id = self.browser.tabs.active_tab_id().unwrap_or(0);
        self.load_seq = self.load_seq.wrapping_add(1);
        let seq = self.load_seq;
        self.pending_loads.insert(tab_id, seq);
        if let Some(previous) = self.load_cancellations.remove(&tab_id) {
            previous.cancel();
        }
        let cancellation = crate::network_scheduler::CancellationToken::default();
        self.load_cancellations.insert(tab_id, cancellation.clone());
        (tab_id, seq, cancellation)
    }

    fn start_youtube_search(&mut self, query: String) -> Command<Message> {
        #[cfg(target_os = "windows")]
        self.teardown_youtube_playback();
        let query = query.trim().to_string();
        let (tab_id, seq, cancellation) =
            self.begin_youtube_load(format!("Searching YouTube for {query}..."));
        let message_query = query.clone();
        Command::perform(
            async move { crate::youtube::fetch_live_youtube_search(&query, cancellation).await },
            move |result| Message::YouTubeSearchLoaded {
                result,
                query: message_query,
                tab_id,
                seq,
            },
        )
    }

    #[cfg(target_os = "windows")]
    fn start_youtube_playback(&mut self, video_id: String) -> Command<Message> {
        self.teardown_youtube_playback();
        let target = format!("https://www.youtube.com/watch?v={video_id}");
        self.url_input = target;
        let (tab_id, seq, cancellation) =
            self.begin_youtube_load("Preparing live YouTube playback...".to_string());
        Command::perform(
            async move { crate::youtube::prepare_live_youtube_playback(&video_id, cancellation).await },
            move |result| Message::YouTubePlaybackPrepared {
                result,
                tab_id,
                seq,
            },
        )
    }

    #[cfg(target_os = "windows")]
    fn teardown_youtube_playback(&mut self) {
        if let Some(playback) = self.youtube_playback.take() {
            let mut sink = playback.audio_sink;
            let _ = sink.flush();
        }
    }

    #[cfg(target_os = "windows")]
    fn teardown_youtube_playback_for_tab(&mut self, tab_id: usize) {
        if self
            .youtube_playback
            .as_ref()
            .is_some_and(|playback| playback.tab_id == tab_id)
        {
            self.teardown_youtube_playback();
        }
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
        if let Some(previous) = self.load_cancellations.remove(&tab_id) {
            previous.cancel();
        }
        let cancellation = crate::network_scheduler::CancellationToken::default();
        self.load_cancellations.insert(tab_id, cancellation.clone());

        let st = self.search_state.entry(tab_id).or_default();
        st.query = query.clone();
        st.results.clear();
        st.loading = true;
        st.error = None;

        self.is_loading = true;
        self.status_msg = format!("Searching for \"{}\"...", query);

        Command::perform(
            async move { search_web_async_with_cancellation(&query, cancellation).await },
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
            let entry = crate::tab::HistoryEntry::new(url.to_string(), title.clone(), &dom);
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

    /// Rebuild the pixel display list for the active tab, load any pending
    /// images (so they become `DisplayItem::Image`), and refresh the decoded
    /// image handles used by the page widget.
    fn rebuild_display_list(&mut self) {
        fn build_list(
            browser: &crate::Browser,
            image_cache: &crate::image_loader::ImageCache,
        ) -> DisplayList {
            if let Some(tab) = browser.active_tab() {
                if is_internal_page(&tab.url) {
                    DisplayList::default()
                } else if let Some(ref root) = tab.layout {
                    crate::paint::build_display_list_with_cache(root, Some(image_cache))
                } else {
                    // No cached layout (e.g. restored history entry): re-layout from the DOM
                    match crate::layout::create_layout_tree(
                        &tab.dom,
                        &browser.css_rules,
                        browser.viewport_width(),
                    ) {
                        Some(root) => {
                            crate::paint::build_display_list_with_cache(&root, Some(image_cache))
                        }
                        None => DisplayList::default(),
                    }
                }
            } else {
                DisplayList::default()
            }
        }

        // Build display list with whatever images are already decoded.
        let list = build_list(&self.browser, &self.browser.image_cache);

        self.display_list = Arc::new(list);
        self.refresh_image_handles();
        self.canvas_cache.clear();
    }

    /// Spawn async image loading for any PendingImage items in the display list.
    /// Returns a Command that sends `Message::ImagesLoaded` when done.
    fn schedule_image_loading(&mut self) -> Command<Message> {
        let base_url = self
            .browser
            .active_tab()
            .map(|tab| tab.url.clone())
            .unwrap_or_default();
        let candidates: Vec<&str> = self
            .display_list
            .items
            .iter()
            .filter_map(|item| match item {
                crate::paint::DisplayItem::PendingImage { url, .. } => Some(url.as_str()),
                _ => None,
            })
            .collect();
        let page_domain = crate::ui_helpers::host(&base_url);
        let mut pending_urls = crate::ui_helpers::bounded_resource_urls(&base_url, candidates, 8);
        pending_urls.retain(|url| {
            !self.browser.image_cache.is_decoded(url)
                && !self.browser.adblocker.should_block_resource(
                    url,
                    page_domain.as_deref(),
                    crate::adblock::ResourceType::Image,
                )
        });

        if pending_urls.is_empty() {
            return Command::none();
        }

        // Bind the batch to the tab+seq it was scheduled for, like page
        // loads: a batch finishing after the user navigated must not touch
        // the (now different) active tab's UI state.
        let tab_id = self.browser.tabs.active_tab_id().unwrap_or(0);
        self.load_seq = self.load_seq.wrapping_add(1);
        let seq = self.load_seq;
        self.pending_loads.insert(tab_id, seq);

        Command::perform(
            async move {
                let mut loaded = Vec::new();
                for url in &pending_urls {
                    let result = crate::image_loader::fetch_and_decode_image_async(url).await;
                    if let Ok(image_data) = result {
                        loaded.push(image_data);
                    }
                }
                loaded
            },
            move |loaded| Message::ImagesLoaded {
                images: loaded,
                tab_id,
                seq,
            },
        )
    }

    /// Rebuild the url -> RGBA Handle map from the decoded image cache.
    fn refresh_image_handles(&mut self) {
        let mut handles: HashMap<String, iced::widget::image::Handle> = HashMap::new();
        for item in &self.display_list.items {
            if let crate::paint::DisplayItem::Image { url, .. } = item {
                if handles.contains_key(url) {
                    continue;
                }
                if let Some(data) = self.browser.image_cache.get_decoded(url) {
                    handles.insert(
                        url.clone(),
                        iced::widget::image::Handle::from_pixels(
                            data.width,
                            data.height,
                            data.rgba_pixels.clone(),
                        ),
                    );
                }
            }
        }
        self.page_image_handles = Arc::new(handles);
    }

    fn after_tab_change(&mut self, status: &str) -> Command<Message> {
        // A tab switched back to a ghita://search page (Back/Forward, tab
        // switch) may still carry a stale `loading` flag with an invalidated
        // seq, which would render "Searching…" forever. Restart the search so
        // the page shows results (or an error) again.
        if let Some(url) = self.browser.tabs.active_tab().map(|t| t.url.clone()) {
            if url.starts_with("ghita://search") && self.is_search_active(&url) {
                return self.start_search(&url);
            }
        }
        self.sync_from_active_tab();
        self.is_loading = false;
        if !status.is_empty() {
            self.status_msg = status.to_string();
        }
        self.rebuild_display_list();
        self.browser.persist_session();
        self.schedule_image_loading()
    }

    /// True when the given ghita://search URL either has a search in flight or
    /// is stuck with a stale loading flag that will never resolve.
    fn is_search_active(&self, url: &str) -> bool {
        let query = search_query_from_url(url).unwrap_or_default();
        if let Some(tab_id) = self.browser.tabs.active_tab_id() {
            if let Some(st) = self.search_state.get(&tab_id) {
                // Re-run when the stored search never completed (bug window:
                // loading set but the seq was invalidated by back/forward).
                if st.loading && st.query != query {
                    return true;
                }
                // Re-run only when the page really has no results and no error;
                // a resolved search must not restart on every tab change.
                if st.loading && st.results.is_empty() && st.error.is_none() {
                    return true;
                }
            }
        }
        false
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

type TabStripInfo = (
    usize,
    String,
    String,
    bool,
    bool,
    bool,
    bool,
    bool,
    Option<u64>,
);

impl GhitaBrowserApp {
    /// Chrome tab strip: rounded tabs, favicon, close button, "+" button
    fn build_tab_strip(&self, pal: &'static Pal) -> Element<'_, Message> {
        let mut strip = row![].spacing(1).padding([6, 8, 0, 8]);

        let tab_info: Vec<TabStripInfo> = self
            .browser
            .tabs
            .iter_tabs()
            .into_iter()
            .map(|t| {
                (
                    t.id,
                    t.title.clone(),
                    t.url.clone(),
                    t.incognito,
                    t.is_sleeping,
                    t.is_discarded,
                    t.is_pinned,
                    t.is_muted,
                    t.group_id,
                )
            })
            .collect();
        let active_id = self.browser.tabs.active_tab_id();

        for (i, (id, title, url, incognito, is_sleeping, is_discarded, pinned, muted, group_id)) in
            tab_info.iter().enumerate()
        {
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

            // Favicon-ish glyph: discarded / sleeping / incognito / internal / regular page.
            // Uses ONLY ASCII characters so they render in EVERY font, no squares.
            let icon_char: char = if *is_discarded {
                'R' // Reload needed
            } else if *is_sleeping {
                'z' // zzz = sleeping
            } else if *incognito {
                'I' // Incognito
            } else if is_internal_page(url) {
                'G' // GhitaBrowser internal
            } else {
                // Use first letter of title for visual variety
                title.chars().next().unwrap_or('?').to_ascii_uppercase()
            };
            let icon_str = icon_char.to_string();

            // Show prefix for sleeping/discarded tabs
            let display_title = if *is_discarded {
                format!("[R] {}", title)
            } else if *is_sleeping {
                format!("[z] {}", title)
            } else if *pinned {
                format!("[P] {}", title)
            } else if *muted {
                format!("[M] {}", title)
            } else if group_id.is_some() {
                format!("[G] {}", title)
            } else {
                title.clone()
            };

            let label = row![
                text(icon_str).size(11),
                text(truncate_label(&display_title, 18)).size(12),
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

            let mut controls = row![tab_btn].spacing(0);
            if is_active {
                controls = controls
                    .push(
                        button(text(if *pinned { "U" } else { "P" }).size(9))
                            .on_press(Message::PinTab(i))
                            .padding([8, 4])
                            .style(chrome_btn(bg, hover, pal.text_dim, [0.0; 4])),
                    )
                    .push(
                        button(text(if *muted { "A" } else { "M" }).size(9))
                            .on_press(Message::ToggleMuteTab(i))
                            .padding([8, 4])
                            .style(chrome_btn(bg, hover, pal.text_dim, [0.0; 4])),
                    )
                    .push(
                        button(text("G").size(9))
                            .on_press(Message::ToggleTabGroup(i))
                            .padding([8, 4])
                            .style(chrome_btn(bg, hover, pal.text_dim, [0.0; 4])),
                    );
            }
            controls = controls.push(close_btn);
            strip = strip.push(controls);
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

    /// Functional vertical tab rail. It exposes the same selection, close and
    /// sleeping/discarded state as the horizontal strip.
    fn build_vertical_tab_strip(&self, pal: &'static Pal) -> Element<'_, Message> {
        let active_id = self.browser.tabs.active_tab_id();
        let mut tabs = column![row![
            text("Tabs")
                .size(14)
                .style(iced::theme::Text::from(pal.text)),
            horizontal_space(),
            button(text("+").size(16))
                .on_press(Message::NewTab)
                .padding([3, 10])
                .style(chrome_btn(pal.tab_strip, pal.tab_hover, pal.text, [6.0; 4])),
        ]
        .align_items(iced::Alignment::Center)]
        .spacing(4)
        .padding(8);

        for (index, tab) in self.browser.tabs.iter_tabs().into_iter().enumerate() {
            let active = Some(tab.id) == active_id;
            let bg = if active { pal.toolbar } else { pal.tab_strip };
            let marker = if tab.is_discarded {
                "R"
            } else if tab.is_sleeping {
                "z"
            } else if tab.incognito {
                "I"
            } else {
                "G"
            };
            let label = row![
                text(marker).size(11),
                text(truncate_label(&tab.title, 21))
                    .size(12)
                    .width(Length::Fill),
            ]
            .spacing(6)
            .align_items(iced::Alignment::Center);
            tabs = tabs.push(
                row![
                    button(label)
                        .on_press(Message::SelectTab(index))
                        .padding([7, 8])
                        .width(Length::Fill)
                        .style(chrome_btn(bg, pal.tab_hover, pal.text, [6.0; 4])),
                    button(text("✕").size(10))
                        .on_press(Message::CloseTab(index))
                        .padding([7, 7])
                        .style(chrome_btn(bg, pal.tab_hover, pal.text_dim, [6.0; 4])),
                ]
                .spacing(2),
            );
        }

        container(scrollable(tabs).height(Length::Fill))
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.tab_strip)),
                border: iced::Border {
                    color: pal.divider,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fixed(230.0))
            .height(Length::Fill)
            .into()
    }

    fn build_tab_search_panel(&self, pal: &'static Pal) -> Element<'_, Message> {
        let query = self.tab_search_query.to_ascii_lowercase();
        let mut results = column![
            row![
                text("Search tabs")
                    .size(14)
                    .style(iced::theme::Text::from(pal.text)),
                horizontal_space(),
                button(text("✕").size(11))
                    .on_press(Message::ToggleTabSearch)
                    .padding([3, 9])
                    .style(chrome_btn(
                        pal.menu_bg,
                        pal.menu_hover,
                        pal.text_dim,
                        [6.0; 4]
                    )),
            ]
            .align_items(iced::Alignment::Center),
            text_input("Title or address", &self.tab_search_query)
                .on_input(Message::TabSearchQueryChanged)
                .size(12)
                .padding(7)
                .style(omnibox_style(pal, 7.0)),
        ]
        .spacing(6);

        for (index, tab) in self.browser.tabs.iter_tabs().into_iter().enumerate() {
            if !query.is_empty()
                && !tab.title.to_ascii_lowercase().contains(&query)
                && !tab.url.to_ascii_lowercase().contains(&query)
            {
                continue;
            }
            results = results.push(
                button(
                    row![
                        text(truncate_label(&tab.title, 30))
                            .size(12)
                            .width(Length::Fill),
                        text(truncate_label(&tab.url, 48))
                            .size(10)
                            .style(iced::theme::Text::from(pal.text_dim)),
                    ]
                    .spacing(12),
                )
                .on_press(Message::SelectTab(index))
                .padding([6, 10])
                .width(Length::Fill)
                .style(chrome_btn(pal.menu_bg, pal.menu_hover, pal.text, [5.0; 4])),
            );
        }

        container(results)
            .padding([10, 18])
            .width(Length::Fill)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.menu_bg)),
                border: iced::Border {
                    color: pal.divider,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn build_task_manager_panel(&self, pal: &'static Pal) -> Element<'_, Message> {
        let mut tasks = column![row![
            text(format!(
                "Task Manager — {:.1} MB estimated",
                self.task_manager.total_memory_mb()
            ))
            .size(14)
            .style(iced::theme::Text::from(pal.text)),
            horizontal_space(),
            button(text("Close").size(11))
                .on_press(Message::ToggleTaskManager)
                .padding([4, 10])
                .style(chrome_btn(pal.menu_bg, pal.menu_hover, pal.text, [6.0; 4])),
        ]
        .align_items(iced::Alignment::Center)]
        .spacing(5);

        for task in &self.task_manager.tasks {
            let index = self
                .browser
                .tabs
                .iter_tabs()
                .into_iter()
                .position(|tab| tab.id == task.tab_id);
            let mut task_row = row![
                text(truncate_label(&task.title, 30))
                    .size(12)
                    .width(Length::Fill),
                text(format!("{:.1} MB", task.memory_mb))
                    .size(11)
                    .width(Length::Fixed(80.0)),
                text(format!("{} nodes", task.layout_nodes))
                    .size(11)
                    .width(Length::Fixed(90.0)),
            ]
            .spacing(8)
            .align_items(iced::Alignment::Center);
            if let Some(index) = index {
                task_row = task_row.push(
                    button(text("End task").size(10))
                        .on_press(Message::CloseTab(index))
                        .padding([4, 8])
                        .style(chrome_btn(pal.danger, pal.danger, pal.on_accent, [5.0; 4])),
                );
            }
            tasks = tasks.push(task_row);
        }

        container(tasks)
            .padding([10, 18])
            .width(Length::Fill)
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(pal.menu_bg)),
                border: iced::Border {
                    color: pal.divider,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
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

        let fwd_btn = button(text(">").size(16))
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

        let reload_btn = button(text("R").size(16))
            .on_press(Message::Reload)
            .padding([4, 9])
            .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4]));

        let home_btn = button(text("H").size(16))
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
            text("L")
                .size(12)
                .style(iced::theme::Text::from(pal.secure))
                .into()
        } else if active_url.starts_with("http://") {
            text("!")
                .size(12)
                .style(iced::theme::Text::from(pal.danger))
                .into()
        } else if is_internal_page(&active_url) {
            text("i").size(12).into()
        } else {
            text("?")
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

        let downloads_btn = button(text("↓").size(15))
            .on_press(Message::OpenInternalPage("ghita://downloads".to_string()))
            .padding([4, 9])
            .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4]));

        let profile_btn = button(text("S").size(14))
            .on_press(Message::OpenInternalPage("ghita://settings".to_string()))
            .padding([4, 9])
            .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [16.0; 4]));

        // Theme toggle button (sun/moon icon)
        let theme_icon = if self.is_dark_theme { "D" } else { "L" };
        let theme_btn = button(text(theme_icon).size(14))
            .on_press(Message::ToggleTheme)
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
            theme_btn,
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
                    text("?").size(12),
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
                matches.push((h.title.clone(), h.url.clone(), "T"));
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

        let site_filter_label = self
            .browser
            .active_tab()
            .and_then(|tab| crate::ui_helpers::host(&tab.url))
            .map(|domain| {
                if self.browser.adblocker.is_domain_enabled(&domain) {
                    format!("Disable blocker for {domain}")
                } else {
                    format!("Enable blocker for {domain}")
                }
            })
            .unwrap_or_else(|| "Site-specific blocker unavailable".to_string());

        let menu = column![
            item("New tab", "Ctrl+T", Message::NewTab),
            item(
                "New Incognito tab",
                "Ctrl+Shift+N",
                Message::NewIncognitoTab
            ),
            item("Open file...", "Ctrl+O", Message::OpenFileDialog),
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
            item("Search tabs", "Ctrl+Shift+A", Message::ToggleTabSearch),
            item("Task manager", "Shift+Esc", Message::ToggleTaskManager),
            item(
                if self.vertical_tabs {
                    "Use horizontal tabs"
                } else {
                    "Use vertical tabs"
                },
                "",
                Message::ToggleVerticalTabs
            ),
            item(
                if self.browser.adblocker.config().enabled {
                    "Disable request blocker"
                } else {
                    "Enable request blocker"
                },
                "",
                Message::ToggleAdBlock
            ),
            item(&site_filter_label, "", Message::ToggleAdBlockForSite),
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

        #[cfg(target_os = "windows")]
        if self
            .youtube_playback
            .as_ref()
            .is_some_and(|playback| self.browser.tabs.active_tab_id() == Some(playback.tab_id))
        {
            return container(self.build_youtube_player(pal))
                .style(move |_: &Theme| container::Appearance {
                    background: Some(iced::Background::Color(pal.content_bg)),
                    text_color: Some(pal.text),
                    ..Default::default()
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

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

            // Real pixel painting: backgrounds, borders, styled glyphs,
            // clickable links, and decoded images on top
            let page = iced::Element::new(WebPageWidget::new(
                self.display_list.clone(),
                &self.canvas_cache,
                self.page_image_handles.clone(),
                zoom,
                base_url,
            ));

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

    #[cfg(target_os = "windows")]
    fn build_youtube_player(&self, pal: &'static Pal) -> Element<'_, Message> {
        let playback = self.youtube_playback.as_ref().expect("player exists");
        let controls = playback.controller.controls();
        let duration = controls.duration_seconds.unwrap_or_default();
        let title = &playback.controller.response.title;
        let frame: Element<'_, Message> = if let Some(handle) = &playback.frame_handle {
            container(
                iced::widget::image(handle.clone())
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fixed(360.0))
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(Color::BLACK)),
                ..Default::default()
            })
            .into()
        } else {
            container(
                text("Press Play to present the first decoded video frame")
                    .size(18)
                    .style(iced::theme::Text::from(pal.text_dim)),
            )
            .width(Length::Fill)
            .height(Length::Fixed(360.0))
            .center_x()
            .center_y()
            .style(move |_: &Theme| container::Appearance {
                background: Some(iced::Background::Color(Color::BLACK)),
                ..Default::default()
            })
            .into()
        };
        let control_row = row![
            button(text(if controls.paused { "Play" } else { "Pause" }))
                .on_press(Message::YouTubeTogglePlayback)
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [5.0; 4])),
            button(text("-10s"))
                .on_press(Message::YouTubeSeekBy(-10.0))
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [5.0; 4])),
            button(text("+10s"))
                .on_press(Message::YouTubeSeekBy(10.0))
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [5.0; 4])),
            button(text(if controls.muted { "Unmute" } else { "Mute" }))
                .on_press(Message::YouTubeToggleMute)
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [5.0; 4])),
            button(text("25%"))
                .on_press(Message::YouTubeSetVolume(0.25))
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [5.0; 4])),
            button(text("50%"))
                .on_press(Message::YouTubeSetVolume(0.5))
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [5.0; 4])),
            button(text("100%"))
                .on_press(Message::YouTubeSetVolume(1.0))
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [5.0; 4])),
            button(text("Recover"))
                .on_press(Message::YouTubeRecover)
                .style(chrome_btn(pal.toolbar, pal.menu_hover, pal.text, [5.0; 4])),
        ]
        .spacing(8)
        .align_items(iced::Alignment::Center);
        scrollable(
            column![
                frame,
                text(title)
                    .size(22)
                    .style(iced::theme::Text::from(pal.text)),
                control_row,
                text(format!(
                    "{:.1}s / {:.1}s | volume {:.0}% | {} | {:.1} MB downloaded | format {}",
                    controls.current_time_seconds,
                    duration,
                    controls.volume * 100.0,
                    if controls.muted { "muted" } else { "audio on" },
                    playback.downloaded_bytes as f64 / (1024.0 * 1024.0),
                    playback.controller.plan.video.itag,
                ))
                .size(12)
                .style(iced::theme::Text::from(pal.text_dim)),
            ]
            .spacing(12)
            .padding(16),
        )
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

/// Custom widget that paints the display list with real pixels AND draws
/// actual decoded images on top of the page geometry.
///
/// iced 0.12's canvas API cannot draw raster images (only shapes and text),
/// so a custom `Widget` is used: the page geometry (backgrounds, borders,
/// text, link hit-testing) is built via the canvas `Cache`, then each cached
/// image is drawn on top with `image::Renderer::draw`.
struct WebPageWidget<'a> {
    list: Arc<DisplayList>,
    cache: &'a canvas::Cache,
    /// url -> decoded image handle (RGBA pixels)
    images: Arc<HashMap<String, iced::widget::image::Handle>>,
    zoom: f32,
    base_url: String,
}

impl<'a> WebPageWidget<'a> {
    fn new(
        list: Arc<DisplayList>,
        cache: &'a canvas::Cache,
        images: Arc<HashMap<String, iced::widget::image::Handle>>,
        zoom: f32,
        base_url: String,
    ) -> Self {
        Self {
            list,
            cache,
            images,
            zoom,
            base_url,
        }
    }

    /// Zoom actually used for the widget size and raster, bounded so the
    /// framebuffer stays sane: the canvas rasterizes `width×height` pixels
    /// (4 bytes each) at the CSS pixel scale × zoom², so an unbounded long
    /// document at high zoom could allocate gigabytes (OOM abort). Both a
    /// total pixel budget and a per-axis dimension cap are enforced; the
    /// doc is simply drawn smaller when the budget would be exceeded.
    fn effective_zoom(&self) -> f32 {
        const MAX_CANVAS_PIXELS: f32 = 32_000_000.0; // ~128 MB RGBA
        const MAX_CANVAS_DIM: f32 = 16_384.0;

        let w = self.list.width.max(1.0);
        let h = self.list.height.max(1.0);
        let mut z = self.zoom;
        if w * h * z * z > MAX_CANVAS_PIXELS {
            z = (MAX_CANVAS_PIXELS / (w * h)).sqrt();
        }
        if w * z > MAX_CANVAS_DIM {
            z = MAX_CANVAS_DIM / w;
        }
        if h * z > MAX_CANVAS_DIM {
            z = MAX_CANVAS_DIM / h;
        }
        z.max(0.05)
    }
}

impl<'a> iced_core::Widget<Message, Theme, iced::Renderer> for WebPageWidget<'a> {
    fn size(&self) -> iced_core::Size<iced_core::Length> {
        let z = self.effective_zoom();
        iced_core::Size::new(
            iced_core::Length::Fixed((self.list.width * z).max(1.0)),
            iced_core::Length::Fixed((self.list.height * z).max(1.0)),
        )
    }

    fn layout(
        &self,
        _tree: &mut iced_core::widget::Tree,
        _renderer: &iced::Renderer,
        limits: &iced_core::layout::Limits,
    ) -> iced_core::layout::Node {
        let z = self.effective_zoom();
        let intrinsic = iced_core::Size::new(
            (self.list.width * z).max(1.0),
            (self.list.height * z).max(1.0),
        );
        let size = limits
            .width(iced_core::Length::Fixed(intrinsic.width))
            .height(iced_core::Length::Fixed(intrinsic.height))
            .resolve(
                iced_core::Length::Fixed(intrinsic.width),
                iced_core::Length::Fixed(intrinsic.height),
                intrinsic,
            );
        iced_core::layout::Node::new(size)
    }

    fn draw(
        &self,
        _tree: &iced_core::widget::Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &iced_core::renderer::Style,
        layout: iced_core::Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &iced_core::Rectangle,
    ) {
        let bounds = layout.bounds();

        // 1. Page geometry: backgrounds, borders, text (cached)
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            frame.scale(self.effective_zoom());

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
                    DisplayItem::VectorShape(shape) => {
                        if let Some(fill) = shape.fill {
                            if fill.a > 0.0 {
                                frame.fill_rectangle(
                                    iced::Point::new(shape.x, shape.y),
                                    iced::Size::new(shape.w, shape.h),
                                    to_color(fill),
                                );
                            }
                        }
                        if let Some(stroke) = shape.stroke {
                            let c = to_color(stroke);
                            let bw = shape.stroke_width.max(0.5);
                            if shape.kind == crate::paint::VectorShapeKind::Line {
                                // Axis-aligned line approximation: a thin
                                // rectangle between the endpoints.
                                let (x1, y1, x2, y2) =
                                    (shape.x, shape.y, shape.x + shape.w, shape.y + shape.h);
                                if (x2 - x1).abs() >= (y2 - y1).abs() {
                                    frame.fill_rectangle(
                                        iced::Point::new(x1.min(x2), y1),
                                        iced::Size::new((x2 - x1).abs().max(bw), bw),
                                        c,
                                    );
                                } else {
                                    frame.fill_rectangle(
                                        iced::Point::new(x1, y1.min(y2)),
                                        iced::Size::new(bw, (y2 - y1).abs().max(bw)),
                                        c,
                                    );
                                }
                            } else {
                                frame.fill_rectangle(
                                    iced::Point::new(shape.x, shape.y),
                                    iced::Size::new(shape.w, bw),
                                    c,
                                );
                                frame.fill_rectangle(
                                    iced::Point::new(shape.x, shape.y + shape.h - bw),
                                    iced::Size::new(shape.w, bw),
                                    c,
                                );
                                frame.fill_rectangle(
                                    iced::Point::new(shape.x, shape.y),
                                    iced::Size::new(bw, shape.h),
                                    c,
                                );
                                frame.fill_rectangle(
                                    iced::Point::new(shape.x + shape.w - bw, shape.y),
                                    iced::Size::new(bw, shape.h),
                                    c,
                                );
                            }
                        }
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
                            let text_width =
                                crate::layout::estimate_text_width(content, *size as f64) as f32;
                            frame.fill_rectangle(
                                iced::Point::new(*x, *y + size * 1.18),
                                iced::Size::new(text_width, 1.0),
                                to_color(*color),
                            );
                        }
                    }
                    // Images are drawn on top (after the geometry), see below.
                    // The geometry only paints a neutral placeholder box.
                    DisplayItem::Image {
                        x, y, w, h, alt, ..
                    } => {
                        frame.fill_rectangle(
                            iced::Point::new(*x, *y),
                            iced::Size::new(*w, *h),
                            iced::Color::from_rgb(0.93, 0.93, 0.94),
                        );
                        frame.fill_text(canvas::Text {
                            content: alt.clone(),
                            position: iced::Point::new(*x + 2.0, *y + 2.0),
                            color: iced::Color::from_rgb(0.45, 0.45, 0.5),
                            size: iced::Pixels(11.0),
                            font: iced::Font::default(),
                            shaping: iced::widget::text::Shaping::Advanced,
                            ..canvas::Text::default()
                        });
                    }
                    DisplayItem::PendingImage {
                        x, y, w, h, alt, ..
                    } => {
                        frame.fill_rectangle(
                            iced::Point::new(*x, *y),
                            iced::Size::new(*w, *h),
                            iced::Color::from_rgb(0.92, 0.92, 0.94),
                        );
                        frame.fill_text(canvas::Text {
                            content: alt.clone(),
                            position: iced::Point::new(*x + 2.0, *y + 2.0),
                            color: iced::Color::from_rgb(0.5, 0.5, 0.6),
                            size: iced::Pixels(11.0),
                            font: iced::Font::default(),
                            shaping: iced::widget::text::Shaping::Advanced,
                            ..canvas::Text::default()
                        });
                    }
                }
            }
        });

        // 2. Custom widgets receive document-local geometry, but the renderer
        // operates in window coordinates. Translate both geometry and decoded
        // images to the assigned widget bounds and clip them so page pixels can
        // never cover the browser chrome above the content viewport.
        iced_core::Renderer::with_layer(renderer, bounds, |renderer| {
            iced_core::Renderer::with_translation(
                renderer,
                iced_core::Vector::new(bounds.x, bounds.y),
                |renderer| {
                    iced::widget::canvas::Renderer::draw(renderer, vec![geometry]);

                    // 3. Draw real decoded images on top of the geometry. The
                    // geometry layer is rasterized at effective_zoom() (capped
                    // for OOM protection), so images use the same scale.
                    let img_zoom = self.effective_zoom();
                    for item in &self.list.items {
                        if let DisplayItem::Image {
                            x, y, w, h, url, ..
                        } = item
                        {
                            if let Some(handle) = self.images.get(url) {
                                iced_core::image::Renderer::draw(
                                    renderer,
                                    handle.clone(),
                                    iced_core::image::FilterMethod::Linear,
                                    iced_core::Rectangle::new(
                                        iced_core::Point::new(x * img_zoom, y * img_zoom),
                                        iced_core::Size::new(w * img_zoom, h * img_zoom),
                                    ),
                                );
                            }
                        }
                    }
                },
            );
        });
    }

    fn on_event(
        &mut self,
        _state: &mut iced_core::widget::Tree,
        event: iced_core::Event,
        layout: iced_core::Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn iced_core::Clipboard,
        shell: &mut iced_core::Shell<'_, Message>,
        _viewport: &iced_core::Rectangle,
    ) -> iced_core::event::Status {
        if let iced_core::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if let Some(pos) = cursor.position_in(layout.bounds()) {
                // Hit-test in DOCUMENT coordinates, which requires the same
                // scale the geometry was rasterized at (effective zoom, not
                // the raw user zoom — they differ when the framebuffer is
                // capped for long docs at high zoom).
                let effective = self.effective_zoom();
                let x = pos.x / effective;
                let y = pos.y / effective;
                if let Some(href) = self.list.link_at(x, y) {
                    let resolved = resolve_href(&self.base_url, href);
                    shell.publish(Message::OpenUrl(resolved));
                    return iced_core::event::Status::Captured;
                }
            }
        }
        iced_core::event::Status::Ignored
    }

    fn mouse_interaction(
        &self,
        _state: &iced_core::widget::Tree,
        layout: iced_core::Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &iced_core::Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if let Some(pos) = cursor.position_in(layout.bounds()) {
            let effective = self.effective_zoom();
            if self
                .list
                .link_at(pos.x / effective, pos.y / effective)
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
                "GhitaBrowser v{} — lightweight document browser",
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
            text("N").size(52),
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
                        text(truncate_label(&r.snippet, 500))
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
            row![text("?").size(12), text(label).size(12),]
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
                        text("P").size(18),
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
        let memory_saver_on = settings.tab_memory_saver;
        let memory_pressure_on = settings.memory_pressure_threshold_mb > 0;

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
            section("Performance"),
            row![
                text("Memory Saver")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text))
                    .width(Length::Fixed(180.0)),
                choice_btn("On", memory_saver_on, Message::SetMemorySaver(true)),
                choice_btn("Off", !memory_saver_on, Message::SetMemorySaver(false)),
            ]
            .spacing(10)
            .align_items(iced::Alignment::Center),
            row![
                text("Memory pressure protection")
                    .size(13)
                    .style(iced::theme::Text::from(pal.text))
                    .width(Length::Fixed(180.0)),
                choice_btn("On", memory_pressure_on, Message::SetMemoryPressure(true)),
                choice_btn(
                    "Off",
                    !memory_pressure_on,
                    Message::SetMemoryPressure(false)
                ),
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
                "GhitaBrowser v{} — a document-focused browser written in safe Rust",
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
            text("G").size(56),
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
                "Version {} — document-focused Windows build",
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
                text("• Bounded JavaScript language subset (no complete DOM/Web APIs)")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
                text("• Native UI: tabs, omnibox, bookmarks, history and downloads")
                    .size(12)
                    .style(iced::theme::Text::from(pal.text_dim)),
                text("• Resource-bounded parser, images, cache and tab lifecycle")
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
                "{} | {} tabs | v{}",
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
    run_gui_with_target(None)
}

pub fn run_gui_with_target(initial_target: Option<String>) -> Result<(), iced::Error> {
    // Use the lightweight CPU tiny-skia renderer instead of the wgpu GPU
    // backend. wgpu alone costs 100+ MB of RAM (GPU buffers, shaders,
    // swapchain) — tiny-skia renders to CPU memory and dramatically reduces
    // the browser's memory footprint. (ICED_BACKEND must be set before the
    // compositor is created inside Application::run.)
    std::env::set_var("ICED_BACKEND", "tiny-skia");

    let mut settings = Settings {
        flags: initial_target,
        ..Settings::default()
    };
    settings.window.size = iced::Size::new(1280.0, 900.0);
    settings.window.min_size = Some(iced::Size::new(800.0, 600.0));
    info!("Starting GhitaBrowser v{} GUI", crate::VERSION);
    GhitaBrowserApp::run(settings)
}

#[cfg(windows)]
fn pick_local_document() -> Option<std::path::PathBuf> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SCRIPT: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.OpenFileDialog
$dialog.Title = 'Open a document in GhitaBrowser'
$dialog.Filter = 'Web and PDF documents (*.html;*.htm;*.xhtml;*.pdf;*.txt)|*.html;*.htm;*.xhtml;*.pdf;*.txt|All files (*.*)|*.*'
$dialog.Multiselect = $false
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    [Console]::Out.Write($dialog.FileName)
}
"#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-STA", "-Command", SCRIPT])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let trimmed = path.trim();
    (!trimmed.is_empty()).then(|| std::path::PathBuf::from(trimmed))
}

#[cfg(not(windows))]
fn pick_local_document() -> Option<std::path::PathBuf> {
    None
}

#[cfg(test)]
mod widget_tests {
    use super::*;

    fn list_of(w: f32, h: f32) -> Arc<crate::paint::DisplayList> {
        let dl = crate::paint::DisplayList {
            width: w,
            height: h,
            ..Default::default()
        };
        Arc::new(dl)
    }

    #[test]
    fn test_effective_zoom_bounds_framebuffer() {
        let cache = canvas::Cache::new();
        let empty = Arc::new(HashMap::new());
        // A long document at 4x zoom would rasterize ~4.2 Gpx (OOM).
        let widget = WebPageWidget::new(
            list_of(15_000.0, 1_100.0),
            &cache,
            empty.clone(),
            4.0,
            String::new(),
        );
        let z = widget.effective_zoom();
        let pixels = 15_000.0 * 1_100.0 * z * z;
        assert!(
            pixels <= 32_000_000.0 + 1.0,
            "framebuffer pixel budget exceeded: {}",
            pixels
        );
        assert!(15_000.0 * z <= 16_384.0, "max dimension exceeded");
    }

    #[test]
    fn test_effective_zoom_unchanged_for_small_docs() {
        let cache = canvas::Cache::new();
        let widget = WebPageWidget::new(
            list_of(100.0, 60.0),
            &cache,
            Arc::new(HashMap::new()),
            2.5,
            String::new(),
        );
        assert!((widget.effective_zoom() - 2.5).abs() < 1e-3);
    }

    #[test]
    fn safe_flow_layout_requires_a_strong_collision_signal() {
        assert!(!requires_safe_flow_layout(7, 0.9));
        assert!(!requires_safe_flow_layout(12, 0.01));
        assert!(requires_safe_flow_layout(8, 0.02));
        assert!(requires_safe_flow_layout(24, 0.0));
    }

    #[test]
    fn address_bar_shortcuts_use_deterministic_replacement_mode() {
        assert!(matches!(
            handle_keyboard(
                iced::keyboard::Key::Character("l".into()),
                iced::keyboard::Modifiers::CTRL
            ),
            Some(Message::ReplaceUrl)
        ));
        assert!(matches!(
            handle_keyboard(
                iced::keyboard::Key::Named(iced::keyboard::key::Named::F6),
                iced::keyboard::Modifiers::empty()
            ),
            Some(Message::ReplaceUrl)
        ));
    }

    #[test]
    fn omnibox_recovers_absolute_urls_appended_by_a_captured_shortcut() {
        assert_eq!(
            normalize_omnibox_replacement(
                "https://old.test/path",
                "https://old.test/pathhttps://new.test/".to_string()
            ),
            "https://new.test/"
        );
        assert_eq!(
            normalize_omnibox_replacement(
                "file:///C:/old.html",
                "file:///C:/old.htmlfile:///C:/new.html".to_string()
            ),
            "file:///C:/new.html"
        );
        assert_eq!(
            normalize_omnibox_replacement(
                "https://old.test/",
                "https://old.test/?next=https://new.test/".to_string()
            ),
            "https://old.test/?next=https://new.test/"
        );

        let active_url = "https://old.test/";
        let mut edited = active_url.to_string();
        for character in "https://new.test/".chars() {
            edited.push(character);
            edited = normalize_omnibox_replacement(active_url, edited);
        }
        assert_eq!(edited, "https://new.test/");
    }

    #[test]
    fn youtube_server_bootstrap_renders_browser_owned_navigation_shell() {
        let initial = serde_json::json!({
            "contents": {"videoRenderer": {
                "videoId": "ghitaVideo1",
                "title": {"runs": [{"text": "Ghita media gate"}]},
                "thumbnail": {"thumbnails": [{"url": "https://img.test/gate.jpg"}]},
                "lengthText": {"simpleText": "0:08"}
            }}
        });
        let player = serde_json::json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {
                "videoId": "ghitaVideo1", "title": "Ghita media gate", "lengthSeconds": "8"
            },
            "streamingData": {"formats": [{
                "itag": 18,
                "mimeType": "video/mp4; codecs=\"avc1.42001e, mp4a.40.2\"",
                "url": "https://media.test/muxed.mp4",
                "bitrate": 500000,
                "width": 640,
                "height": 360,
                "audioQuality": "AUDIO_QUALITY_MEDIUM"
            }]}
        });
        let html = format!(
            "<script>var ytInitialData={initial};var ytInitialPlayerResponse={player};</script>"
        );
        let shell =
            build_youtube_shell_html("https://www.youtube.com/watch?v=ghitaVideo1", &html).unwrap();
        assert!(shell.contains("Ghita media gate"));
        assert!(shell.contains("watch?v=ghitaVideo1"));
        assert!(shell.contains("Player metadata validated for 1 direct clear-content format(s)."));
    }
}

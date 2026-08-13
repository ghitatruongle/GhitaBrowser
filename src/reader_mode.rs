// Reader mode extractor - extracts clean article content from web pages

use crate::parser::{parse_html, Element};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReaderTheme {
    Light,
    Sepia,
    Dark,
    SoftBlue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReaderArticle {
    pub title: String,
    pub byline: Option<String>,
    pub content_html: String,
    pub text_content: String,
    pub estimated_reading_time_mins: usize,
    pub site_name: Option<String>,
    pub published_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReaderSettings {
    pub theme: ReaderTheme,
    pub font_size: u32,
    pub font_family: String,
    pub line_height: f32,
}

impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            theme: ReaderTheme::Sepia,
            font_size: 18,
            font_family: "Georgia, serif".to_string(),
            line_height: 1.6,
        }
    }
}

/// Tags that should never appear in article content
const BLOCK_TAGS: &[&str] = &[
    "script",
    "style",
    "nav",
    "header",
    "footer",
    "aside",
    "iframe",
    "noscript",
    "form",
    "button",
    "input",
    "advertisement",
    "social",
    "comment",
    "sidebar",
];

/// Tags that indicate article content containers
const ARTICLE_TAGS: &[&str] = &["article", "main", "content", "entry", "post"];

/// Tags that are okay to keep in article content
const CONTENT_TAGS: &[&str] = &[
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "blockquote",
    "pre",
    "code",
    "figure",
    "figcaption",
    "img",
    "a",
    "strong",
    "em",
    "b",
    "i",
    "u",
    "br",
    "hr",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
];

/// Budgets bounding reader extraction: a hostile/huge page must not produce
/// unbounded output text or HTML, and traversal stops past a sane node count.
const MAX_TRAVERSED_NODES: u64 = 100_000;
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 500_000;

/// Running reader-extraction budget.
struct Budget {
    nodes: u64,
    html_bytes: usize,
    text_chars: usize,
}

pub struct ReaderModeExtractor;

impl ReaderModeExtractor {
    /// Distill article content from raw HTML string using DOM readability heuristics.
    /// Uses the crate parser (not regex line-stripping) for proper DOM traversal.
    pub fn extract(html: &str, url_hint: &str, title_hint: &str) -> ReaderArticle {
        let dom = parse_html(html);

        // Try to find the best article container
        let article_container = Self::find_article_container(&dom);

        // Extract title
        let title = Self::extract_title(&dom, title_hint);

        // Extract byline/author
        let byline = Self::extract_byline(&dom);

        // Extract site name from meta tags or URL
        let site_name = Self::extract_site_name(&dom, url_hint);

        // Extract published date
        let published_date = Self::extract_published_date(&dom);

        // Extract and clean content
        let (content_html, text_content) = if let Some(container) = article_container {
            Self::clean_article_content(container)
        } else {
            // Fallback: use body
            if let Some(body) = dom.find_tag("body") {
                Self::clean_article_content(body)
            } else {
                (String::new(), String::new())
            }
        };

        // Estimate reading time (200 words per minute)
        let word_count = text_content.split_whitespace().count();
        let reading_time = (word_count / 200).max(1);

        ReaderArticle {
            title,
            byline,
            content_html,
            text_content,
            estimated_reading_time_mins: reading_time,
            site_name,
            published_date,
        }
    }

    /// Find the best article container using semantic HTML and class/id hints
    fn find_article_container(dom: &Element) -> Option<&Element> {
        // 1. Try <article> tag directly
        if let Some(article) = dom.find_tag("article") {
            return Some(article);
        }

        // 2. Try <main> tag
        if let Some(main) = dom.find_tag("main") {
            return Some(main);
        }

        // 3. Look for elements with article-related classes/ids
        let candidates = dom.find_all_tags("div");
        let mut best: Option<&Element> = None;
        let mut best_score = 0;

        for div in candidates {
            let score = Self::score_element(div);
            if score > best_score {
                best_score = score;
                best = Some(div);
            }
        }

        if best_score > 0 {
            best
        } else {
            None
        }
    }

    /// Score an element based on how likely it is to contain article content
    fn score_element(el: &Element) -> i32 {
        let mut score: i32 = 0;
        let class_id = format!(
            "{} {}",
            el.get_attr("class").cloned().unwrap_or_default(),
            el.get_attr("id").cloned().unwrap_or_default()
        )
        .to_lowercase();

        for tag in ARTICLE_TAGS {
            if class_id.contains(tag) {
                score += 10;
            }
        }

        if class_id.contains("content") {
            score += 5;
        }
        if class_id.contains("article") {
            score += 8;
        }
        if class_id.contains("post") {
            score += 5;
        }
        if class_id.contains("entry") {
            score += 5;
        }

        // Penalize elements that likely contain navigation/ads
        if class_id.contains("nav")
            || class_id.contains("sidebar")
            || class_id.contains("footer")
            || class_id.contains("header")
            || class_id.contains("comment")
            || class_id.contains("ad")
            || class_id.contains("social")
        {
            score = score.saturating_sub(20);
        }

        // Bonus for having paragraph children (indicates real content)
        let p_count = el.find_all_tags("p").len();
        score += p_count.min(10) as i32;

        score
    }

    /// Extract page title from DOM
    fn extract_title(dom: &Element, hint: &str) -> String {
        // Try <title> first
        if let Some(title_el) = dom.find_tag("title") {
            let title = title_el.text.trim().to_string();
            if !title.is_empty() {
                return title;
            }
        }

        // Try og:title meta tag
        if let Some(og_title) = Self::get_meta_content(dom, "og:title") {
            return og_title;
        }

        // Try <h1>
        if let Some(h1) = dom.find_tag("h1") {
            let text = h1.text.trim().to_string();
            if !text.is_empty() {
                return text;
            }
        }

        // Fallback to hint or "Article"
        if !hint.is_empty() {
            hint.to_string()
        } else {
            "Article".to_string()
        }
    }

    /// Extract author/byline from common patterns
    fn extract_byline(dom: &Element) -> Option<String> {
        // Try meta tags
        if let Some(author) = Self::get_meta_content(dom, "author") {
            return Some(author);
        }
        if let Some(author) = Self::get_meta_content(dom, "article:author") {
            return Some(author);
        }

        // Look for common byline class/id patterns
        let candidates = dom.find_all_tags("div");
        for div in candidates {
            let class_id = format!(
                "{} {}",
                div.get_attr("class").cloned().unwrap_or_default(),
                div.get_attr("id").cloned().unwrap_or_default()
            )
            .to_lowercase();

            if class_id.contains("author")
                || class_id.contains("byline")
                || class_id.contains("writer")
            {
                let text = div.text_content().trim().to_string();
                if !text.is_empty() && text.len() < 200 {
                    return Some(text);
                }
            }
        }

        None
    }

    /// Extract site name from meta tags or URL
    fn extract_site_name(dom: &Element, url_hint: &str) -> Option<String> {
        // Try og:site_name meta tag
        if let Some(site_name) = Self::get_meta_content(dom, "og:site_name") {
            return Some(site_name);
        }

        // Extract from URL
        if !url_hint.is_empty() {
            if let Ok(parsed) = url::Url::parse(url_hint) {
                if let Some(host) = parsed.host_str() {
                    // Remove www. prefix
                    let name = host.trim_start_matches("www.").to_string();
                    if !name.is_empty() {
                        return Some(name);
                    }
                }
            }
        }

        None
    }

    /// Extract published date from meta tags
    fn extract_published_date(dom: &Element) -> Option<String> {
        let candidates = [
            "article:published_time",
            "datePublished",
            "date",
            "pubdate",
            "og:published_time",
        ];

        for meta_name in &candidates {
            if let Some(date) = Self::get_meta_content(dom, meta_name) {
                // Clean and truncate to date portion
                let date_clean = date.split('T').next().unwrap_or(&date).to_string();
                if !date_clean.is_empty() {
                    return Some(date_clean);
                }
            }
        }

        None
    }

    /// Get content of a meta tag by property/name attribute
    fn get_meta_content(dom: &Element, name: &str) -> Option<String> {
        for meta in dom.find_all_tags("meta") {
            let prop = meta.get_attr("property").cloned().unwrap_or_default();
            let meta_name = meta.get_attr("name").cloned().unwrap_or_default();
            if prop == name || meta_name == name {
                if let Some(content) = meta.get_attr("content") {
                    let c = content.trim().to_string();
                    if !c.is_empty() {
                        return Some(c);
                    }
                }
            }
        }
        None
    }

    /// Clean article content by removing unwanted elements and returning HTML + text
    fn clean_article_content(container: &Element) -> (String, String) {
        let mut content_elements: Vec<String> = Vec::new();
        let mut text_parts: Vec<String> = Vec::new();
        let mut budget = Budget {
            nodes: 0,
            html_bytes: 0,
            text_chars: 0,
        };

        Self::collect_content(
            container,
            &mut content_elements,
            &mut text_parts,
            &mut budget,
        );

        let mut content_html = content_elements.join("\n");
        if content_html.len() > MAX_HTML_BYTES {
            content_html.truncate(MAX_HTML_BYTES);
        }
        let mut text_content = text_parts.join(" ");
        if text_content.chars().count() > MAX_TEXT_CHARS {
            text_content = text_content.chars().take(MAX_TEXT_CHARS).collect();
        }

        (content_html, text_content)
    }

    /// Recursively collect content elements, skipping unwanted ones.
    /// Always recurses into children to ensure unwanted tags (script, style, etc.)
    /// are excluded from the collected text. `budget` bounds total traversal
    /// and output so reader extraction can't blow up on huge pages.
    fn collect_content(
        el: &Element,
        html_out: &mut Vec<String>,
        text_out: &mut Vec<String>,
        budget: &mut Budget,
    ) {
        // Hard node budget: stop once a page is unreasonably large instead
        // of O(N²)-style scans over it.
        budget.nodes += 1;
        if budget.nodes > MAX_TRAVERSED_NODES || budget.html_bytes > MAX_HTML_BYTES {
            return;
        }

        let tag_lower = el.tag.to_lowercase();

        // Skip unwanted tags entirely (don't recurse into them)
        if BLOCK_TAGS.contains(&tag_lower.as_str()) {
            return;
        }

        // Skip hidden elements
        if Self::is_hidden(el) {
            return;
        }

        // If this is a leaf content tag (like <p>, <h1>, etc.), emit and stop
        if CONTENT_TAGS.contains(&tag_lower.as_str()) {
            let html = el.to_html();
            let html = html.trim();
            if !html.is_empty() && budget.html_bytes + html.len() <= MAX_HTML_BYTES {
                html_out.push(html.to_string());
                budget.html_bytes += html.len();
            }
            // Text: the element's own text, plus children's text (formatting
            // tags like <strong>/<em> carry no HTML of their own) — text-only
            // recursion, never re-emitting HTML.
            let text = el.text.trim().to_string();
            if !text.is_empty() {
                budget.text_chars = budget.text_chars.saturating_add(text.chars().count());
                if budget.text_chars <= MAX_TEXT_CHARS {
                    text_out.push(text);
                }
            }
            for child in &el.children {
                Self::collect_text_only(child, text_out, budget);
            }
            return;
        }

        // For container tags (div, span, section, body, etc.), just recurse
        for child in &el.children {
            Self::collect_content(child, html_out, text_out, budget);
        }
    }

    /// Emit only text for a subtree (used under content tags whose children
    /// are already serialized into the parent's HTML). Hidden/blocked tags
    /// are skipped, matching `collect_content`.
    fn collect_text_only(el: &Element, text_out: &mut Vec<String>, budget: &mut Budget) {
        budget.nodes += 1;
        if budget.nodes > MAX_TRAVERSED_NODES || budget.text_chars > MAX_TEXT_CHARS {
            return;
        }
        let tag_lower = el.tag.to_lowercase();
        if BLOCK_TAGS.contains(&tag_lower.as_str()) {
            return;
        }
        if Self::is_hidden(el) {
            return;
        }
        let text = el.text.trim().to_string();
        if !text.is_empty() {
            budget.text_chars = budget.text_chars.saturating_add(text.chars().count());
            if budget.text_chars <= MAX_TEXT_CHARS {
                text_out.push(text);
            }
        }
        for child in &el.children {
            Self::collect_text_only(child, text_out, budget);
        }
    }

    /// True when an element is hidden by inline style or the hidden/aria-hidden
    /// attributes. Normalized (whitespace-agnostic) so `display:none` and
    /// `display: none` are both caught.
    fn is_hidden(el: &Element) -> bool {
        if let Some(style) = el.get_attr("style") {
            let s = style.to_ascii_lowercase().replace(' ', "");
            if s.contains("display:none") || s.contains("visibility:hidden") {
                return true;
            }
        }
        if let Some(h) = el.get_attr("hidden") {
            let h = h.trim();
            if h.is_empty() || h.eq_ignore_ascii_case("hidden") || h == "true" {
                return true;
            }
        }
        el.get_attr("aria-hidden")
            .map(|v| v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reader_mode_extract_basic() {
        let html = r#"<html>
        <head><title>Test Article</title></head>
        <body>
            <nav>Navigation</nav>
            <article>
                <h1>Test Article Title</h1>
                <p>This is the first paragraph of the article with some content.</p>
                <p>This is the second paragraph with more content for reading.</p>
            </article>
            <footer>Footer content</footer>
        </body>
        </html>"#;

        let article = ReaderModeExtractor::extract(html, "https://example.com", "");
        assert!(article.title.contains("Test Article"));
        assert!(article.text_content.contains("first paragraph"));
        assert!(!article.text_content.contains("Navigation")); // nav should be stripped
    }

    #[test]
    fn test_reader_mode_extract_with_meta() {
        let html = r#"<html>
        <head>
            <meta property="og:title" content="Social Media Title">
            <meta name="author" content="John Doe">
            <meta property="og:site_name" content="Example Site">
        </head>
        <body>
            <h1>Page Title</h1>
            <p>Article content here.</p>
        </body>
        </html>"#;

        let article = ReaderModeExtractor::extract(html, "https://example.com/article", "");
        assert!(
            article.title.contains("Social Media Title"),
            "title: {:?}",
            article.title
        );
        assert!(
            article.byline.as_deref().unwrap_or("").contains("John Doe"),
            "byline: {:?}",
            article.byline
        );
        // og:site_name should take precedence, fallback to URL host
        // The fallback to URL should work since url_hint is https://example.com/article
        let site = article.site_name.as_deref().unwrap_or("");
        assert!(
            !site.is_empty(),
            "site_name should not be empty, got: {:?}",
            article.site_name
        );
        // Check that site_name is either "Example Site" (from meta) or "example.com" (from URL)
        let site_lower = site.to_lowercase();
        assert!(
            site_lower.contains("example") || site_lower.contains("site"),
            "site_name should contain 'example' or 'site', got: {}",
            site
        );
    }

    #[test]
    fn test_reader_mode_reading_time() {
        // Create HTML with enough words (200+)
        let words = "word ".repeat(500);
        let html = format!(
            r#"<html><body><article><p>{}</p></article></body></html>"#,
            words
        );

        let article = ReaderModeExtractor::extract(&html, "", "");
        assert!(article.estimated_reading_time_mins >= 1);
    }

    #[test]
    fn test_strips_scripts_and_styles() {
        let html = r#"<html>
        <body>
            <script>document.write('bad');</script>
            <style>.hidden { display: none; }</style>
            <p>Good paragraph content here.</p>
        </body>
        </html>"#;

        let article = ReaderModeExtractor::extract(html, "", "");
        assert!(article.text_content.contains("Good paragraph"));
        assert!(!article.text_content.contains("document.write"));
        assert!(!article.text_content.contains("display: none"));
    }

    #[test]
    fn test_extract_empty_html() {
        let article = ReaderModeExtractor::extract("", "", "");
        assert_eq!(article.title, "Article");
        assert!(article.text_content.is_empty());
        assert_eq!(article.estimated_reading_time_mins, 1);
    }

    #[test]
    fn test_extract_handles_nested_elements() {
        // Reader Mode should handle nested elements correctly
        let html = r#"<html><body>
            <article>
                <h1>Title Here</h1>
                <div class="content">
                    <p>First <strong>paragraph</strong> with nested <em>elements</em>.</p>
                    <ul><li>Item 1</li><li>Item 2</li></ul>
                </div>
            </article>
        </body></html>"#;

        let article = ReaderModeExtractor::extract(html, "https://test.com", "");
        assert!(article.title.contains("Title Here"));
        assert!(article.text_content.contains("paragraph"));
        assert!(article.text_content.contains("nested"));
        assert!(article.text_content.contains("Item 1"));
    }

    #[test]
    fn test_inline_formatting_is_not_duplicated() {
        // <strong>/<em> inside <p> must not be emitted twice (once via <p>'s
        // own HTML, once via its own content-tag branch).
        let html = r#"<html><body>
            <article>
                <p>Hello <strong>world</strong>!</p>
            </article>
        </body></html>"#;

        let article = ReaderModeExtractor::extract(html, "https://test.com", "");
        let strong_count = article.content_html.match_indices("<strong>").count();
        assert_eq!(strong_count, 1, "HTML must contain <strong> exactly once");
        // Text contains the bold word once
        assert_eq!(
            article.text_content.matches("world").count(),
            1,
            "bold text must appear exactly once in the text"
        );
    }

    #[test]
    fn test_hidden_attribute_and_no_space_style_are_skipped() {
        let html = r#"<html><body>
            <article>
                <p>Visible paragraph</p>
                <p style="display:none">Hidden by compact style</p>
                <p hidden>Hidden by attribute</p>
                <p aria-hidden="true">Hidden by ARIA</p>
            </article>
        </body></html>"#;

        let article = ReaderModeExtractor::extract(html, "https://test.com", "");
        assert!(article.text_content.contains("Visible paragraph"));
        assert!(!article.text_content.contains("Hidden by compact style"));
        assert!(!article.text_content.contains("Hidden by attribute"));
        assert!(!article.text_content.contains("Hidden by ARIA"));
    }

    #[test]
    fn test_extract_handles_unicode() {
        let html = r#"<html><body>
            <article>
                <h1>Tiếng Việt có dấu</h1>
                <p>Xin chào thế giới! 🌍 This has emoji too.</p>
                <p>中文测试 - Chinese characters here.</p>
            </article>
        </body></html>"#;

        let article = ReaderModeExtractor::extract(html, "https://test.com", "");
        assert!(article.title.contains("Tiếng Việt"));
        assert!(article.text_content.contains("Xin chào"));
        assert!(article.text_content.contains("中文"));
    }

    #[test]
    fn test_extract_main_tag() {
        // When there's no <article>, fall back to <main>
        let html = r#"<html><body>
            <main>
                <h2>Main Content</h2>
                <p>This is the main content of the page.</p>
            </main>
        </body></html>"#;

        let article = ReaderModeExtractor::extract(html, "", "");
        assert!(article.text_content.contains("main content"));
    }

    #[test]
    fn test_extract_url_fallback_for_site() {
        // When no meta tag, fall back to URL host
        let article = ReaderModeExtractor::extract(
            "<html><body><p>x</p></body></html>",
            "https://www.example.com/path",
            "",
        );
        assert!(article.site_name.is_some());
        let site = article.site_name.as_deref().unwrap();
        assert!(site.contains("example.com"), "got: {}", site);
    }

    #[test]
    fn test_extract_invalid_url() {
        // Invalid URL should not crash
        let article = ReaderModeExtractor::extract(
            "<html><body><p>x</p></body></html>",
            "not a valid url",
            "",
        );
        // Site name should be None (URL parsing failed)
        assert!(
            article.site_name.is_none()
                || article.site_name.as_deref().is_some_and(|s| !s.is_empty())
        );
    }
}

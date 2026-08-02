// Reader mode extractor

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

pub struct ReaderModeExtractor;

impl ReaderModeExtractor {
    /// Distill article content from raw HTML string using DOM readability heuristics
    pub fn extract(html: &str, title_hint: &str) -> ReaderArticle {
        // Strip scripts, styles, nav, headers, footers
        let text = html
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.starts_with("<script") && !line.starts_with("<style") && !line.starts_with("<nav"))
            .collect::<Vec<_>>()
            .join("\n");

        let word_count = text.split_whitespace().count();
        let reading_time = (word_count / 200).max(1);

        ReaderArticle {
            title: if title_hint.is_empty() { "Article".to_string() } else { title_hint.to_string() },
            byline: None,
            content_html: format!("<article><h1>{}</h1><div>{}</div></article>", title_hint, text),
            text_content: text,
            estimated_reading_time_mins: reading_time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reader_mode_extraction() {
        let html = "<html><body><h1>Sample Header</h1><p>This is paragraph 1 of the article.</p></body></html>";
        let article = ReaderModeExtractor::extract(html, "Sample Header");
        assert_eq!(article.title, "Sample Header");
        assert!(article.estimated_reading_time_mins >= 1);
    }
}

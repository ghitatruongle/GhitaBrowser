//! Omnibox & Unified Address Bar Engine for GhitaBrowser (Phase 24).
//! Implements URL security indicators, query autocomplete, search engine formatting.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityIndicator {
    Secure,   // https://
    Insecure, // http://
    Local,    // file://
    Internal, // about:, chrome:
}

impl SecurityIndicator {
    pub fn from_url(url: &str) -> Self {
        if url.starts_with("https://") {
            SecurityIndicator::Secure
        } else if url.starts_with("http://") {
            SecurityIndicator::Insecure
        } else if url.starts_with("file://") {
            SecurityIndicator::Local
        } else {
            SecurityIndicator::Internal
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmniboxMatchKind {
    History,
    Bookmark,
    SearchSuggestion,
}

#[derive(Debug, Clone)]
pub struct OmniboxMatch {
    pub title: String,
    pub url: String,
    pub kind: OmniboxMatchKind,
    pub score: usize,
}

pub struct OmniboxEngine {
    pub default_search_url_template: String,
}

impl OmniboxEngine {
    pub fn new() -> Self {
        Self {
            default_search_url_template: "https://google.com/search?q={query}".to_string(),
        }
    }

    pub fn is_direct_url(input: &str) -> bool {
        let input = input.trim();
        input.starts_with("http://")
            || input.starts_with("https://")
            || input.starts_with("file://")
            || input.starts_with("about:")
            || (input.contains('.') && !input.contains(' '))
    }

    pub fn format_nav_or_search_url(&self, input: &str) -> String {
        let input = input.trim();
        if Self::is_direct_url(input) {
            if !input.contains("://") && !input.starts_with("about:") {
                format!("https://{input}")
            } else {
                input.to_string()
            }
        } else {
            let encoded = encode_query_component(input);
            self.default_search_url_template
                .replace("{query}", &encoded)
        }
    }

    pub fn autocomplete(
        &self,
        query: &str,
        history: &[(String, String)],   // (url, title)
        bookmarks: &[(String, String)], // (url, title)
    ) -> Vec<OmniboxMatch> {
        let query_lower = query.trim().to_lowercase();
        if query_lower.is_empty() {
            return Vec::new();
        }

        let mut matches = Vec::new();

        // Match bookmarks
        for (url, title) in bookmarks {
            if title.to_lowercase().contains(&query_lower)
                || url.to_lowercase().contains(&query_lower)
            {
                matches.push(OmniboxMatch {
                    title: title.clone(),
                    url: url.clone(),
                    kind: OmniboxMatchKind::Bookmark,
                    score: 100,
                });
            }
        }

        // Match history
        for (url, title) in history {
            if (title.to_lowercase().contains(&query_lower)
                || url.to_lowercase().contains(&query_lower))
                && !matches.iter().any(|m| m.url == *url)
            {
                matches.push(OmniboxMatch {
                    title: title.clone(),
                    url: url.clone(),
                    kind: OmniboxMatchKind::History,
                    score: 80,
                });
            }
        }

        // Search engine fallback suggestion
        let search_url = self.format_nav_or_search_url(query);
        matches.push(OmniboxMatch {
            title: format!("Search Google for '{query}'"),
            url: search_url,
            kind: OmniboxMatchKind::SearchSuggestion,
            score: 50,
        });

        matches.sort_by_key(|b| std::cmp::Reverse(b.score));
        matches
    }
}

impl Default for OmniboxEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn encode_query_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omnibox_security_indicator_and_formatting() {
        assert_eq!(
            SecurityIndicator::from_url("https://secure.com"),
            SecurityIndicator::Secure
        );
        assert_eq!(
            SecurityIndicator::from_url("http://insecure.com"),
            SecurityIndicator::Insecure
        );

        let omni = OmniboxEngine::new();
        assert_eq!(
            omni.format_nav_or_search_url("example.com"),
            "https://example.com"
        );
        assert!(omni
            .format_nav_or_search_url("rust browser")
            .contains("google.com/search?q=rust"));
    }

    #[test]
    fn omnibox_autocomplete_matches_bookmarks_and_history() {
        let omni = OmniboxEngine::new();
        let history = vec![("https://news.com".to_string(), "Daily News".to_string())];
        let bookmarks = vec![("https://github.com".to_string(), "GitHub Repo".to_string())];

        let results = omni.autocomplete("git", &history, &bookmarks);
        assert!(!results.is_empty());
        assert_eq!(results[0].kind, OmniboxMatchKind::Bookmark);
        assert_eq!(results[0].url, "https://github.com");
    }
}

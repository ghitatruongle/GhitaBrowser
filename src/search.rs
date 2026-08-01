// src/search.rs - Web search results for GhitaBrowser (v0.6.0)
// Fetches DuckDuckGo's lightweight HTML endpoint (no JavaScript needed)
// and parses the result list for display in the in-app results page.

use std::time::Duration;

use crate::parser::parse_html;

/// A single web search result (title, real URL, snippet)
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// DuckDuckGo's no-JS HTML endpoint, works with any plain HTTP client
const DDG_HTML_URL: &str = "https://html.duckduckgo.com/html/?q=";

/// A standard browser user-agent so search engines serve normal pages
const SEARCH_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// Perform a web search and return the parsed result list.
/// Errors are user-facing strings ("offline", "status 429", ...).
pub fn search_web(query: &str) -> Result<Vec<SearchResult>, String> {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let url = format!("{}{}", DDG_HTML_URL, encoded);

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .redirects(5)
        .user_agent(SEARCH_UA)
        .build();

    let response = agent.get(&url).call().map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.into_string().map_err(|e| e.to_string())?;

    if !(200..300).contains(&status) {
        return Err(format!("Search engine returned status {}", status));
    }

    Ok(parse_ddg_html(&body))
}

/// Parse DuckDuckGo HTML results into structured search results.
///
/// Result markup looks like:
///   <div class="result">
///     <h2 class="result__title">
///       <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=<encoded>&rut=...">Title</a>
///     </h2>
///     <a class="result__snippet" href="...">Snippet text</a>
///   </div>
///
/// Iterating per `result` div keeps each title paired with its own snippet
/// even when a title gets filtered out, and matching class *tokens* exactly
/// avoids substring false positives (e.g. a `result__body` div matching
/// `result`, or a `not_a_result__a` link matching `result__a`).
pub fn parse_ddg_html(html: &str) -> Vec<SearchResult> {
    let dom = parse_html(html);
    let mut results = Vec::new();

    for div in dom.find_all_tags("div") {
        if !has_class(div, "result") {
            continue;
        }
        let snippet = find_descendant(div, "a", "result__snippet")
            .map(|a| clean_whitespace(&a.text_content()))
            .unwrap_or_default();

        if let Some(title_a) = find_descendant(div, "a", "result__a") {
            let title = clean_whitespace(&title_a.text_content());
            let url = title_a
                .get_attr("href")
                .and_then(|href| decode_ddg_url(href))
                .unwrap_or_default();
            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
    }

    results
}

/// Whether an element's `class` attribute contains `needle` as a whole
/// whitespace-separated token.
fn has_class(el: &crate::parser::Element, needle: &str) -> bool {
    el.get_attr("class")
        .map(|c| c.split_whitespace().any(|tok| tok == needle))
        .unwrap_or(false)
}

/// Depth-first search for the first descendant element with the given tag
/// and an exact class token.
fn find_descendant<'a>(
    el: &'a crate::parser::Element,
    tag: &str,
    class: &str,
) -> Option<&'a crate::parser::Element> {
    if el.tag == tag && has_class(el, class) {
        return Some(el);
    }
    el.children
        .iter()
        .find_map(|child| find_descendant(child, tag, class))
}

/// DuckDuckGo wraps every result in a redirect URL with a percent-encoded
/// `uddg` parameter. Extract and decode it to get the real destination.
fn decode_ddg_url(href: &str) -> Option<String> {
    let raw = href.split("uddg=").nth(1)?.split('&').next()?;
    if raw.is_empty() {
        return None;
    }
    let decoded = url::form_urlencoded::parse(format!("q={}", raw).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())?;
    if decoded.starts_with("http://") || decoded.starts_with("https://") {
        Some(decoded)
    } else {
        None
    }
}

/// Collapse runs of whitespace into single spaces and trim
fn clean_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ddg_html_basic() {
        let html = r#"
            <html><body>
            <div class="result">
                <h2 class="result__title">
                    <a rel="nofollow" class="result__a"
                       href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">The
                       Rust Programming Language</a>
                </h2>
                <a class="result__snippet" href="//duckduckgo.com/l/?uddg=...">A language
                   empowering everyone to build reliable software.</a>
            </div>
            <div class="result">
                <h2 class="result__title">
                    <a rel="nofollow" class="result__a"
                       href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F&amp;rut=def">The
                       Rust Book</a>
                </h2>
                <a class="result__snippet" href="//duckduckgo.com/l/?uddg=...">Learn Rust with
                   the official book.</a>
            </div>
            </body></html>
        "#;

        let results = parse_ddg_html(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "The Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(
            results[0].snippet,
            "A language empowering everyone to build reliable software."
        );
        assert_eq!(results[1].url, "https://doc.rust-lang.org/book/");
    }

    #[test]
    fn test_parse_ddg_html_no_results() {
        let html = "<html><body><div class=\"no-results\">No results found</div></body></html>";
        assert!(parse_ddg_html(html).is_empty());
    }

    #[test]
    fn test_snippet_stays_aligned_when_title_filtered() {
        // The first result's title anchor has no decodable URL, so it is
        // filtered out — its snippet must NOT be attached to the next result.
        let html = r#"
            <html><body>
            <div class="result">
                <h2 class="result__title">
                    <a class="result__a" href="https://ads.example.com/landing">Ad without redirect</a>
                </h2>
                <a class="result__snippet">This snippet belongs to the filtered ad.</a>
            </div>
            <div class="result">
                <h2 class="result__title">
                    <a class="result__a"
                       href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">The
                       Rust Programming Language</a>
                </h2>
                <a class="result__snippet">A language empowering everyone.</a>
            </div>
            </body></html>
        "#;
        let results = parse_ddg_html(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone.");
    }

    #[test]
    fn test_class_matches_exact_tokens_not_substrings() {
        // "result__a" inside a non-result container, and classes that merely
        // *contain* "result" as a substring, must be ignored.
        let html = r#"
            <html><body>
            <div class="result__footer">
                <a class="not_a_result__a"
                   href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&amp;rut=zzz">Decoy</a>
                <a class="result__snippet">Decoy snippet</a>
            </div>
            <div class="result">
                <a class="result__a"
                   href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">The
                   Rust Programming Language</a>
                <a class="result__snippet">A language empowering everyone.</a>
            </div>
            </body></html>
        "#;
        let results = parse_ddg_html(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "The Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone.");
    }

    #[test]
    fn test_decode_ddg_url() {
        assert_eq!(
            decode_ddg_url(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1%26c%3D2&rut=xyz"
            ),
            Some("https://example.com/a?b=1&c=2".to_string())
        );
        assert_eq!(decode_ddg_url("//example.com/direct"), None);
        assert_eq!(decode_ddg_url("//duckduckgo.com/l/?uddg="), None);
    }

    #[test]
    fn test_clean_whitespace() {
        assert_eq!(clean_whitespace("  a\n  b\t c  "), "a b c");
    }
}

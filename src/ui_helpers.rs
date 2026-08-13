//! Pure helpers used by UI resource scheduling.

use std::collections::HashSet;

/// Resolve a subresource against its page URL and accept only HTTP(S).
pub(crate) fn resolve_http_resource(base_url: &str, candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() || candidate.starts_with("data:") {
        return None;
    }
    let resolved = url::Url::parse(candidate)
        .or_else(|_| url::Url::parse(base_url)?.join(candidate))
        .ok()?;
    matches!(resolved.scheme(), "http" | "https").then(|| resolved.into())
}

/// Resolve a page subresource. Remote pages may use only HTTP(S); local pages
/// may use files inside the selected document's directory.
pub(crate) fn resolve_page_resource(base_url: &str, candidate: &str) -> Option<String> {
    if base_url.starts_with("file://") {
        crate::local_file::resolve_local_subresource(base_url, candidate)
    } else {
        resolve_http_resource(base_url, candidate)
    }
}

/// Deduplicate, resolve and bound a page-provided subresource list.
pub(crate) fn bounded_resource_urls<'a>(
    base_url: &str,
    candidates: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<String> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|candidate| resolve_page_resource(base_url, candidate))
        .filter(|url| seen.insert(url.clone()))
        .take(limit)
        .collect()
}

pub(crate) fn host(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

/// Produce a Windows-safe final path component for downloaded content.
pub(crate) fn sanitize_download_filename(candidate: &str) -> String {
    let basename = std::path::Path::new(candidate)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    let mut clean: String = basename
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .take(120)
        .collect();
    clean = clean.trim_matches([' ', '.']).to_string();
    if clean.is_empty() {
        return "download".to_string();
    }

    let stem = std::path::Path::new(&clean)
        .file_stem()
        .and_then(|part| part.to_str())
        .unwrap_or("")
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            });
    if reserved {
        clean.insert(0, '_');
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_and_rejects_unsupported_resources() {
        assert_eq!(
            resolve_http_resource("https://example.com/a/", "../x.png").as_deref(),
            Some("https://example.com/x.png")
        );
        assert!(resolve_http_resource("https://example.com", "data:image/png;base64,x").is_none());
        assert!(resolve_http_resource("https://example.com", "file:///secret").is_none());
    }

    #[test]
    fn resource_collection_is_unique_and_bounded() {
        let urls = bounded_resource_urls(
            "https://example.com/",
            ["a.png", "a.png", "b.png", "c.png"],
            2,
        );
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com/a.png");
    }

    #[test]
    fn local_resource_resolution_stays_beneath_document_directory() {
        let root = std::env::temp_dir().join(format!("ghita_ui_resource_{}", std::process::id()));
        let page_dir = root.join("page");
        std::fs::create_dir_all(&page_dir).unwrap();
        let page = page_dir.join("index.html");
        let image = page_dir.join("image.png");
        let outside = root.join("outside.png");
        std::fs::write(&page, b"page").unwrap();
        std::fs::write(&image, b"image").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let page_url = crate::local_file::url_from_path(&page).unwrap();
        assert!(resolve_page_resource(&page_url, "image.png").is_some());
        assert!(resolve_page_resource(&page_url, "../outside.png").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn download_filename_is_a_safe_component() {
        assert_eq!(sanitize_download_filename("../../CON.txt"), "_CON.txt");
        assert_eq!(
            sanitize_download_filename("bad<name>?.html"),
            "bad_name__.html"
        );
        assert_eq!(sanitize_download_filename("..."), "download");
        assert!(sanitize_download_filename(&"x".repeat(500)).len() <= 120);
    }
}

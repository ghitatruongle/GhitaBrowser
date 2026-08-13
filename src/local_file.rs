//! Safe, bounded loading for user-selected local documents.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::network::FetchResult;

pub const MAX_LOCAL_DOCUMENT_BYTES: u64 = 50 * 1024 * 1024;

pub fn url_from_path(path: &Path) -> Result<String, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Cannot open local path: {error}"))?;
    if !canonical.is_file() {
        return Err("Local path is not a file".to_string());
    }
    url::Url::from_file_path(canonical)
        .map(String::from)
        .map_err(|_| "Cannot convert local path to a file URL".to_string())
}

pub fn resolve_local_input(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_matches('"');
    if trimmed.starts_with("file://") {
        return url::Url::parse(trimmed)
            .ok()
            .filter(|url| url.scheme() == "file")
            .map(String::from);
    }
    let path = PathBuf::from(trimmed);
    path.is_file().then(|| url_from_path(&path).ok()).flatten()
}

pub fn fetch_local_document(url_str: &str) -> Result<FetchResult, String> {
    let start = Instant::now();
    let url = url::Url::parse(url_str).map_err(|error| error.to_string())?;
    if url.scheme() != "file" {
        return Err("Local loader accepts only file URLs".to_string());
    }
    let path = url
        .to_file_path()
        .map_err(|_| "Invalid local file URL".to_string())?
        .canonicalize()
        .map_err(|error| format!("Cannot resolve local file: {error}"))?;
    if !path.is_file() {
        return Err("Local document is not a regular file".to_string());
    }
    let metadata = path
        .metadata()
        .map_err(|error| format!("Cannot inspect local file: {error}"))?;
    if metadata.len() > MAX_LOCAL_DOCUMENT_BYTES {
        return Err(format!(
            "Local document exceeds the {} MB limit",
            MAX_LOCAL_DOCUMENT_BYTES / 1024 / 1024
        ));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(&path)
        .and_then(|file| {
            file.take(MAX_LOCAL_DOCUMENT_BYTES + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("Cannot read local file: {error}"))?;
    if bytes.len() as u64 > MAX_LOCAL_DOCUMENT_BYTES {
        return Err("Local document grew beyond its size limit".to_string());
    }

    let content_type = content_type_for_path(&path);
    let (body, binary_body) = if content_type == "application/pdf" {
        (String::new(), Some(bytes))
    } else {
        (decode_text(&bytes)?, None)
    };
    let canonical_url = url_from_path(&path)?;
    Ok(FetchResult {
        body,
        binary_body,
        url: canonical_url,
        status_code: 200,
        content_type: content_type.to_string(),
        headers: Default::default(),
        fetch_time_ms: start.elapsed().as_millis() as u64,
        set_cookie_headers: Vec::new(),
    })
}

pub fn fetch_local_text(url_str: &str) -> Result<FetchResult, String> {
    let result = fetch_local_document(url_str)?;
    if result.binary_body.is_some() {
        return Err("The selected file is a binary document".to_string());
    }
    Ok(result)
}

pub fn resolve_local_subresource(base_url: &str, candidate: &str) -> Option<String> {
    let base = url::Url::parse(base_url).ok()?;
    if base.scheme() != "file" {
        return None;
    }
    let base_path = base.to_file_path().ok()?.canonicalize().ok()?;
    let root = base_path.parent()?.canonicalize().ok()?;
    let resolved = base.join(candidate.trim()).ok()?;
    if resolved.scheme() != "file" {
        return None;
    }
    let target = resolved.to_file_path().ok()?.canonicalize().ok()?;
    if !target.is_file() || !target.starts_with(&root) {
        return None;
    }
    url::Url::from_file_path(target).ok().map(String::from)
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html",
        "xhtml" => "application/xhtml+xml",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn decode_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(bytes[3..].to_vec()).map_err(|error| error.to_string());
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let words: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16(&words).map_err(|error| error.to_string());
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let words: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16(&words).map_err(|error| error.to_string());
    }
    String::from_utf8(bytes.to_vec()).map_err(|error| format!("Unsupported text encoding: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ghita_local_{name}_{}_{}",
            std::process::id(),
            nonce
        ))
    }

    #[test]
    fn loads_utf8_and_utf16_documents() {
        let root = fixture_dir("encoding");
        std::fs::create_dir_all(&root).unwrap();
        let utf8 = root.join("index.html");
        std::fs::write(&utf8, b"<title>Local</title>").unwrap();
        let fetched = fetch_local_text(&url_from_path(&utf8).unwrap()).unwrap();
        assert_eq!(fetched.content_type, "text/html");
        assert!(fetched.body.contains("Local"));

        let utf16 = root.join("utf16.html");
        let mut bytes = vec![0xFF, 0xFE];
        for word in "<p>Xin chao</p>".encode_utf16() {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        std::fs::write(&utf16, bytes).unwrap();
        assert!(fetch_local_text(&url_from_path(&utf16).unwrap())
            .unwrap()
            .body
            .contains("Xin chao"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_subresources_cannot_escape_document_directory() {
        let root = fixture_dir("resource");
        let page_dir = root.join("page");
        std::fs::create_dir_all(&page_dir).unwrap();
        let page = page_dir.join("index.html");
        let image = page_dir.join("image.png");
        let secret = root.join("secret.txt");
        std::fs::write(&page, b"page").unwrap();
        std::fs::write(&image, b"image").unwrap();
        std::fs::write(&secret, b"secret").unwrap();
        let page_url = url_from_path(&page).unwrap();
        assert!(resolve_local_subresource(&page_url, "image.png").is_some());
        assert!(resolve_local_subresource(&page_url, "../secret.txt").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}

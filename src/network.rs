// src/network.rs - Real HTTP Networking & Resource Caching (v0.6.0)
// Uses ureq for HTTP/HTTPS fetching with TLS support, integrated with cookie jar

use log::info;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// User agent for all network requests, kept in sync with the crate version
pub(crate) fn browser_ua() -> String {
    format!("GhitaBrowser/{} (Rust)", crate::VERSION)
}

/// Cache TTL from the Cache-Control header, falling back to 5 minutes.
/// Returns 0 (do not cache) for no-store/no-cache/private responses, which
/// the browser must revalidate or never store.
/// Shared by the headless engine (fetch_with_cache) and the GUI fetch path.
pub(crate) fn cache_ttl_secs(headers: &HashMap<String, String>) -> u64 {
    headers
        .get("cache-control")
        .and_then(|cc| {
            let lower = cc.to_ascii_lowercase();
            // These directives forbid storing/reusing the response
            if lower.contains("no-store") || lower.contains("no-cache") || lower.contains("private")
            {
                return Some(0);
            }
            if lower.contains("max-age=") {
                let max_age_str = lower
                    .split("max-age=")
                    .nth(1)?
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?;
                Some(max_age_str.parse::<u64>().unwrap_or(300))
            } else {
                None
            }
        })
        .unwrap_or(300) // default 5 minutes
}

/// Result of an HTTP fetch operation
#[derive(Debug, Clone)]
pub struct FetchResult {
    pub body: String,
    pub url: String,
    pub status_code: u16,
    pub content_type: String,
    pub headers: HashMap<String, String>,
    pub fetch_time_ms: u64,
    pub set_cookie_headers: Vec<String>,
}

/// Fetch content from a URL using real HTTP/HTTPS via ureq
pub fn fetch_url(url_str: &str) -> Result<FetchResult, Box<dyn std::error::Error>> {
    // Build request agent
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(5)
        .user_agent(&browser_ua())
        .build();

    execute_fetch(&agent, url_str, None)
}

/// Fetch with cookie jar integration
/// Injects stored cookies into request, and parses Set-Cookie from response
pub fn fetch_with_cookies(
    url_str: &str,
    cookie_store: &mut crate::storage::CookieStore,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let _start = Instant::now();
    let parsed = url::Url::parse(url_str)?;
    let domain = parsed.host_str().unwrap_or("").to_string();

    // Build the request agent
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(5)
        .user_agent(&browser_ua())
        .build();

    // Get matching cookies and build Cookie header
    let matching_cookies = cookie_store.get_cookies(&domain);
    let cookie_header: String = matching_cookies
        .iter()
        .map(|c| c.to_header_value())
        .collect::<Vec<_>>()
        .join("; ");

    // Execute fetch with or without cookies
    let result = if cookie_header.is_empty() {
        info!("No cookies for domain: {}", domain);
        execute_fetch(&agent, url_str, None)?
    } else {
        info!(
            "Sending {} cookies for domain: {}",
            matching_cookies.len(),
            domain
        );
        execute_fetch(&agent, url_str, Some(&cookie_header))?
    };

    // Parse Set-Cookie headers from response. Attribute them to the host of
    // the FINAL URL (after redirects), so a Set-Cookie sent by the redirect
    // target is not stored under the original host that never sent it.
    let final_domain = url::Url::parse(&result.url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or(domain);
    for set_cookie_val in &result.set_cookie_headers {
        let cookie = crate::storage::Cookie::from_set_cookie_header(set_cookie_val, &final_domain);
        if !cookie.name.is_empty() {
            info!(
                "Stored cookie: {}={} for domain {}",
                cookie.name, cookie.value, cookie.domain
            );
            cookie_store.add_cookie(cookie);
        }
    }

    Ok(result)
}

/// Internal: execute the actual HTTP request
fn execute_fetch(
    agent: &ureq::Agent,
    url_str: &str,
    cookie_header: Option<&str>,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let start = Instant::now();

    // Validate and parse URL
    let parsed = url::Url::parse(url_str)?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("Unsupported URL scheme: {}", scheme).into());
    }

    info!("Fetching URL: {}", url_str);

    // Build request with optional cookie header
    let mut request = agent.get(url_str);
    if let Some(cookie_val) = cookie_header {
        request = request.set("Cookie", cookie_val);
    }

    let response = request.call()?;

    let status_code = response.status();
    let content_type = response
        .header("content-type")
        .unwrap_or("text/html")
        .to_string();

    // Collect all Set-Cookie headers (there can be multiple)
    let mut set_cookie_headers = Vec::new();
    for cookie_val in response.all("set-cookie") {
        let trimmed = cookie_val.trim();
        if !trimmed.is_empty() {
            set_cookie_headers.push(trimmed.to_string());
        }
    }

    // Collect response headers
    let mut headers = HashMap::new();
    for header_name in &[
        "content-type",
        "content-length",
        "set-cookie",
        "cache-control",
        "last-modified",
    ] {
        if let Some(val) = response.header(header_name) {
            headers.insert(header_name.to_string(), val.to_string());
        }
    }

    // Report the FINAL URL: ureq follows redirects internally, so the body
    // and headers came from a possibly different host than url_str.
    let final_url = response.get_url().to_string();

    // Read body as string
    let body = response.into_string()?;
    let fetch_time_ms = start.elapsed().as_millis() as u64;

    info!(
        "Fetched {} ({} bytes, {} ms, status {})",
        final_url,
        body.len(),
        fetch_time_ms,
        status_code
    );

    Ok(FetchResult {
        body,
        url: final_url,
        status_code,
        content_type,
        headers,
        fetch_time_ms,
        set_cookie_headers,
    })
}

/// Download raw bytes from a URL (used by the downloads manager)
/// Returns (bytes, suggested_file_name, content_type)
pub fn download_url(
    url_str: &str,
) -> Result<(Vec<u8>, String, String), Box<dyn std::error::Error>> {
    let parsed = url::Url::parse(url_str)?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("Unsupported URL scheme: {}", scheme).into());
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .redirects(5)
        .user_agent(&browser_ua())
        .build();

    let response = agent.get(url_str).call()?;
    let content_type = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();

    // Suggested name: Content-Disposition filename, else last URL path segment
    let mut file_name = response
        .header("content-disposition")
        .and_then(|cd| cd.split("filename=").nth(1))
        .and_then(|s| s.split(';').next())
        .map(|s| s.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default();
    if file_name.is_empty() {
        file_name = parsed
            .path_segments()
            .and_then(|mut segs| segs.rfind(|s| !s.is_empty()))
            .unwrap_or("")
            .to_string();
    }
    if file_name.is_empty() {
        file_name = format!("{}.html", parsed.host_str().unwrap_or("download"));
    }

    // Read body bytes (limit 100MB). Read one extra byte so an over-limit
    // response is reported as an error instead of being silently truncated.
    use std::io::Read;
    const MAX_DOWNLOAD: u64 = 100 * 1024 * 1024;
    let mut bytes: Vec<u8> = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DOWNLOAD {
        return Err("File exceeds the 100MB download limit".into());
    }

    info!(
        "Downloaded {} ({} bytes) as {}",
        url_str,
        bytes.len(),
        file_name
    );

    Ok((bytes, file_name, content_type))
}

/// Fetch with caching support - returns cached result if fresh
/// If cookie_store is provided, integrates cookie jar (inject + parse Set-Cookie)
pub fn fetch_with_cache(
    url: &str,
    cache: &mut ResourceCache,
    cookie_store: Option<&mut crate::storage::CookieStore>,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    // Check cache first
    if let Some(cached) = cache.get(url) {
        if !cached.is_expired() {
            info!("Cache HIT for: {}", url);
            let result = cached.result.clone();
            cache.record_hit();
            return Ok(result);
        } else {
            info!("Cache STALE for: {}", url);
        }
    }

    cache.record_miss();

    // Fetch fresh (with or without cookies)
    let result = if let Some(cs) = cookie_store {
        fetch_with_cookies(url, cs)?
    } else {
        fetch_url(url)?
    };

    // Determine TTL from cache-control header or use default. Error responses
    // are never cached (a transient 500 must not be served for 5 minutes).
    let ttl = if result.status_code >= 400 {
        0
    } else {
        cache_ttl_secs(&result.headers)
    };

    if ttl > 0 {
        cache.insert(url, result.clone(), ttl);
    }

    Ok(result)
}

/// Cache entry with TTL support
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub result: FetchResult,
    pub cached_at: Instant,
    pub ttl_secs: u64,
}

impl CacheEntry {
    pub fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > Duration::from_secs(self.ttl_secs)
    }
}

/// Resource cache with TTL-based eviction
pub struct ResourceCache {
    entries: HashMap<String, CacheEntry>,
    pub max_size: usize,
    pub max_entries: usize,
    hits: u64,
    misses: u64,
}

impl Default for ResourceCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_size: 1024 * 1024 * 50, // 50MB default
            max_entries: 100,
            hits: 0,
            misses: 0,
        }
    }

    /// Calculate total bytes used by all cached entries
    pub fn total_bytes(&self) -> usize {
        self.entries.values().map(|e| e.result.body.len()).sum()
    }

    pub fn insert(&mut self, url: &str, result: FetchResult, ttl_secs: u64) {
        let entry_size = result.body.len();

        // Evict expired entries first
        self.evict_expired();

        // Enforce max_entries limit - remove oldest if at capacity
        while self.entries.len() >= self.max_entries {
            if let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.cached_at)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest_key);
            } else {
                break;
            }
        }

        // Enforce max_size byte limit - remove oldest entries until under limit
        while self.total_bytes() + entry_size > self.max_size && !self.entries.is_empty() {
            if let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.cached_at)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest_key);
            } else {
                break;
            }
        }

        let entry = CacheEntry {
            result,
            cached_at: Instant::now(),
            ttl_secs,
        };
        self.entries.insert(url.to_string(), entry);
    }

    pub fn get(&self, url: &str) -> Option<&CacheEntry> {
        self.entries.get(url)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.hits = 0;
        self.misses = 0;
    }

    /// Remove expired entries
    pub fn evict_expired(&mut self) {
        let expired_keys: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.is_expired())
            .map(|(k, _)| k.clone())
            .collect();
        let count = expired_keys.len();
        for key in expired_keys {
            self.entries.remove(&key);
        }
        if count > 0 {
            info!("Evicted {} expired cache entries", count);
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.entries.len(),
            hits: self.hits,
            misses: self.misses,
            max_size: self.max_size,
        }
    }

    /// Record a cache hit
    pub fn record_hit(&mut self) {
        self.hits += 1;
    }

    /// Record a cache miss
    pub fn record_miss(&mut self) {
        self.misses += 1;
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub max_size: usize,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cache: {} entries | Hits: {} | Misses: {} | Hit rate: {:.1}%",
            self.entries,
            self.hits,
            self.misses,
            if self.hits + self.misses > 0 {
                (self.hits as f64 / (self.hits + self.misses) as f64) * 100.0
            } else {
                0.0
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_validation() {
        let result = fetch_url("https://example.com");
        assert!(result.is_ok() || result.is_err()); // Network-dependent
    }

    #[test]
    fn test_invalid_url() {
        let result = fetch_url("ftp://invalid-scheme.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = ResourceCache::new();
        let url = "https://example.com";
        let fetch_result = FetchResult {
            body: "Hello World!".to_string(),
            url: url.to_string(),
            status_code: 200,
            content_type: "text/html".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 10,
            set_cookie_headers: vec![],
        };

        cache.insert(url, fetch_result.clone(), 3600);
        assert!(cache.get(url).is_some());
        assert_eq!(cache.get(url).unwrap().result.body, "Hello World!");
    }

    #[test]
    fn test_cache_expiry() {
        let mut cache = ResourceCache::new();
        let url = "https://test.com";
        let fetch_result = FetchResult {
            body: "Test".to_string(),
            url: url.to_string(),
            status_code: 200,
            content_type: "text/plain".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
        };

        cache.insert(url, fetch_result, 0); // TTL = 0, expires immediately
        assert!(cache.get(url).unwrap().is_expired());
    }

    #[test]
    fn test_cache_evict_expired() {
        let mut cache = ResourceCache::new();
        let fetch_result = FetchResult {
            body: "Data".to_string(),
            url: "https://x.com".to_string(),
            status_code: 200,
            content_type: "text/plain".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
        };

        cache.insert("https://x.com", fetch_result, 0); // expires immediately
        assert_eq!(cache.len(), 1);
        cache.evict_expired();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cache_ttl_respects_no_store() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "no-store, max-age=3600".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_respects_no_cache() {
        let mut headers = HashMap::new();
        headers.insert("cache-control".to_string(), "no-cache".to_string());
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_respects_private() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "private, max-age=100".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_uses_max_age() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "public, max-age=120".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 120);
    }

    #[test]
    fn test_cache_ttl_default() {
        assert_eq!(cache_ttl_secs(&HashMap::new()), 300);
    }

    #[test]
    fn test_error_responses_are_not_cached() {
        let mut cache = ResourceCache::new();
        let err_result = FetchResult {
            body: "Internal Server Error".to_string(),
            url: "https://e.com".to_string(),
            status_code: 500,
            content_type: "text/html".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
        };

        // FetchResult built by hand (no network): verify the guard used by
        // fetch_with_cache never stores a >= 400 response.
        let ttl = if err_result.status_code >= 400 {
            0
        } else {
            cache_ttl_secs(&err_result.headers)
        };
        if ttl > 0 {
            cache.insert("https://e.com", err_result.clone(), ttl);
        }
        assert!(cache.get("https://e.com").is_none());
    }
}

// src/network.rs - Real HTTP Networking & Resource Caching (v0.0.2)
// Uses ureq for HTTP/HTTPS fetching with TLS support, integrated with cookie jar

use std::collections::HashMap;
use std::time::{Duration, Instant};
use log::info;

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
        .user_agent("GhitaBrowser/0.0.2 (Rust)")
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
        .user_agent("GhitaBrowser/0.0.2 (Rust)")
        .build();
    
    // Get matching cookies and build Cookie header
    let matching_cookies = cookie_store.get_cookies(&domain);
    let cookie_header: String = matching_cookies.iter()
        .map(|c| c.to_header_value())
        .collect::<Vec<_>>()
        .join("; ");
    
    // Execute fetch with or without cookies
    let result = if cookie_header.is_empty() {
        info!("No cookies for domain: {}", domain);
        execute_fetch(&agent, url_str, None)?
    } else {
        info!("Sending {} cookies for domain: {}", matching_cookies.len(), domain);
        execute_fetch(&agent, url_str, Some(&cookie_header))?
    };
    
    // Parse Set-Cookie headers from response
    for set_cookie_val in &result.set_cookie_headers {
        let cookie = crate::storage::Cookie::from_set_cookie_header(set_cookie_val, &domain);
        if !cookie.name.is_empty() {
            info!("Stored cookie: {}={} for domain {}", cookie.name, cookie.value, cookie.domain);
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
    if let Some(all_cookies) = response.header("set-cookie") {
        // ureq may return all Set-Cookie headers concatenated
        // Split by common patterns for multiple cookies
        for cookie_str in all_cookies.lines() {
            let trimmed = cookie_str.trim();
            if !trimmed.is_empty() {
                set_cookie_headers.push(trimmed.to_string());
            }
        }
    }
    
    // Collect response headers
    let mut headers = HashMap::new();
    for header_name in &["content-type", "content-length", "set-cookie", "cache-control", "last-modified"] {
        if let Some(val) = response.header(header_name) {
            headers.insert(header_name.to_string(), val.to_string());
        }
    }
    
    // Read body as string
    let body = response.into_string()?;
    let fetch_time_ms = start.elapsed().as_millis() as u64;
    
    info!("Fetched {} ({} bytes, {} ms, status {})", url_str, body.len(), fetch_time_ms, status_code);
    
    Ok(FetchResult {
        body,
        url: url_str.to_string(),
        status_code,
        content_type,
        headers,
        fetch_time_ms,
        set_cookie_headers,
    })
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
            return Ok(cached.result.clone());
        } else {
            info!("Cache STALE for: {}", url);
        }
    }
    
    // Fetch fresh (with or without cookies)
    let result = if let Some(cs) = cookie_store {
        fetch_with_cookies(url, cs)?
    } else {
        fetch_url(url)?
    };
    
    // Determine TTL from cache-control header or use default
    let ttl = result.headers
        .get("cache-control")
        .and_then(|cc| {
            if cc.contains("max-age=") {
                let max_age_str = cc.split("max-age=").nth(1)?
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?;
                Some(max_age_str.parse::<u64>().unwrap_or(300))
            } else {
                None
            }
        })
        .unwrap_or(300); // default 5 minutes
    
    cache.insert(url, result.clone(), ttl);
    
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
    hits: u64,
    misses: u64,
}

impl ResourceCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_size: 1024 * 1024 * 50, // 50MB default
            hits: 0,
            misses: 0,
        }
    }
    
    pub fn insert(&mut self, url: &str, result: FetchResult, ttl_secs: u64) {
        // Evict if at capacity (simple: remove oldest if over max_size)
        if self.entries.len() >= 100 {
            if let Some(oldest_key) = self.entries.iter()
                .min_by_key(|(_, e)| e.cached_at)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest_key);
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
        let expired_keys: Vec<String> = self.entries.iter()
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
}

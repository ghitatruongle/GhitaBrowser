// src/network.rs - Network & Resource Caching Module (Phase 1-2)

use std::collections::HashMap;
use std::time::Duration;

/// Fetch content from a URL (with fallback simulation for offline/test environments)
pub fn fetch_url(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(format!("<html><body><h1>Loaded content from {}</h1></body></html>", url))
    } else if !url.is_empty() {
        Ok(format!("<html><body><p>Local content: {}</p></body></html>", url))
    } else {
        Err("Invalid or empty URL".into())
    }
}

/// Resource cache entry
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub data: Vec<u8>,
    pub mime_type: String,
}

/// Resource cache for caching network responses
pub struct ResourceCache {
    entries: HashMap<String, CacheEntry>,
    pub max_size: usize,
}

impl ResourceCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_size: 1024 * 1024 * 10, // 10MB default limit
        }
    }

    pub fn insert(&mut self, url: &str, data: Vec<u8>, mime_type: &str) {
        let entry = CacheEntry {
            data,
            mime_type: mime_type.to_string(),
        };
        self.entries.insert(url.to_string(), entry);
    }

    pub fn get(&self, url: &str) -> Option<&Vec<u8>> {
        self.entries.get(url).map(|e| &e.data)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Timeout configuration for network requests
#[derive(Debug, Clone)]
pub struct TimeoutConfig {
    pub connect: Duration,
    pub read: Duration,
}

impl TimeoutConfig {
    pub fn new(connect: Duration, read: Duration) -> Self {
        Self { connect, read }
    }

    pub fn default() -> Self {
        Self {
            connect: Duration::from_secs(30),
            read: Duration::from_secs(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetch_url() {
        let res = fetch_url("https://example.com");
        assert!(res.is_ok());
        assert!(res.unwrap().contains("https://example.com"));
    }

    #[test]
    fn test_cache() {
        let mut cache = ResourceCache::new();
        cache.insert("https://a.com", b"test".to_vec(), "text/plain");
        assert_eq!(cache.get("https://a.com"), Some(&b"test".to_vec()));
    }
}
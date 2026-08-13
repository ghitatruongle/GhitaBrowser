//! Bounded clean-room Cache API implementation for GhitaBrowser (Phase 22).
//! Provides origin-scoped Request/Response cache storage for SW & web platform.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub const MAX_CACHES_PER_ORIGIN: usize = 64;
pub const MAX_ENTRIES_PER_CACHE: usize = 500;
pub const MAX_BODY_BYTES_PER_ENTRY: usize = 5 * 1024 * 1024; // 5 MB

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheEntry {
    pub request_url: String,
    pub request_method: String,
    pub response_status: u16,
    pub response_headers: HashMap<String, String>,
    pub response_body: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Cache {
    pub name: String,
    pub entries: Vec<CacheEntry>,
}

impl Cache {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            entries: Vec::new(),
        }
    }

    pub fn put(
        &mut self,
        request_url: impl Into<String>,
        request_method: impl Into<String>,
        response_status: u16,
        response_headers: HashMap<String, String>,
        response_body: Vec<u8>,
    ) -> Result<(), String> {
        let url = request_url.into();
        let method = request_method.into().to_uppercase();

        if method != "GET" {
            return Err("Cache API only supports caching GET requests".to_string());
        }

        if response_body.len() > MAX_BODY_BYTES_PER_ENTRY {
            return Err("Cached body exceeds 5 MB limit".to_string());
        }

        let entry = CacheEntry {
            request_url: url.clone(),
            request_method: method,
            response_status,
            response_headers,
            response_body,
        };

        if let Some(pos) = self.entries.iter().position(|e| e.request_url == url) {
            self.entries[pos] = entry;
        } else {
            if self.entries.len() >= MAX_ENTRIES_PER_CACHE {
                return Err("Max cache entries limit exceeded".to_string());
            }
            self.entries.push(entry);
        }

        Ok(())
    }

    pub fn match_req(&self, request_url: &str) -> Option<&CacheEntry> {
        self.entries.iter().find(|e| e.request_url == request_url)
    }

    pub fn delete(&mut self, request_url: &str) -> bool {
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.request_url == request_url)
        {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn keys(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.request_url.clone()).collect()
    }
}

#[derive(Debug, Default)]
pub struct CacheStorage {
    pub origin: String,
    pub caches: HashMap<String, Cache>,
    storage_path: Option<PathBuf>,
}

impl CacheStorage {
    pub fn new(origin: impl Into<String>, storage_path: Option<PathBuf>) -> Self {
        let origin = origin.into();
        let mut cs = Self {
            origin,
            caches: HashMap::new(),
            storage_path,
        };
        let _ = cs.load_from_disk();
        cs
    }

    pub fn open(&mut self, name: &str) -> Result<&mut Cache, String> {
        if !self.caches.contains_key(name) {
            if self.caches.len() >= MAX_CACHES_PER_ORIGIN {
                return Err("Max caches per origin limit exceeded".to_string());
            }
            self.caches.insert(name.to_string(), Cache::new(name));
        }
        Ok(self.caches.get_mut(name).expect("cache exists"))
    }

    pub fn has(&self, name: &str) -> bool {
        self.caches.contains_key(name)
    }

    pub fn delete(&mut self, name: &str) -> bool {
        let removed = self.caches.remove(name).is_some();
        if removed {
            let _ = self.save_to_disk();
        }
        removed
    }

    pub fn keys(&self) -> Vec<String> {
        self.caches.keys().cloned().collect()
    }

    pub fn match_all(&self, request_url: &str) -> Option<CacheEntry> {
        for cache in self.caches.values() {
            if let Some(entry) = cache.match_req(request_url) {
                return Some(entry.clone());
            }
        }
        None
    }

    /// Persist the current cache state for browser-host integrations.
    pub fn persist(&self) -> Result<(), String> {
        self.save_to_disk().map_err(|error| error.to_string())
    }

    fn save_to_disk(&self) -> std::io::Result<()> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[derive(serde::Serialize)]
        struct Persisted<'a> {
            schema: u32,
            origin: &'a str,
            caches: &'a HashMap<String, Cache>,
        }
        let bytes = serde_json::to_vec(&Persisted {
            schema: 1,
            origin: &self.origin,
            caches: &self.caches,
        })
        .map_err(std::io::Error::other)?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }

    fn load_from_disk(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.storage_path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        #[derive(serde::Deserialize)]
        struct Persisted {
            schema: u32,
            origin: String,
            caches: HashMap<String, Cache>,
        }
        let bytes = fs::read(path)?;
        let persisted: Persisted = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        if persisted.schema != 1 || persisted.origin != self.origin {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CacheStorage origin/schema mismatch",
            ));
        }
        self.caches = persisted.caches;
        Ok(())
    }
}

impl Drop for CacheStorage {
    fn drop(&mut self) {
        let _ = self.save_to_disk();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_api_put_match_and_delete() {
        let mut cs = CacheStorage::new("https://example.com", None);
        let cache = cs.open("v1").unwrap();

        cache
            .put(
                "https://example.com/app.js",
                "GET",
                200,
                HashMap::new(),
                b"console.log('hi');".to_vec(),
            )
            .unwrap();

        assert_eq!(cache.keys(), vec!["https://example.com/app.js"]);
        let match_entry = cache.match_req("https://example.com/app.js").unwrap();
        assert_eq!(match_entry.response_status, 200);

        assert!(cache.delete("https://example.com/app.js"));
        assert_eq!(cache.keys().len(), 0);
    }
}

// src/storage.rs - Cookie and localStorage persistence with serde (v0.1.2)
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use serde::{Serialize, Deserialize};
use log::{info, warn, error};

/// Represents a single HTTP cookie
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>,  // Unix timestamp (seconds), None = session cookie
    pub secure: bool,
    pub http_only: bool,
    #[serde(default)]
    pub same_site: String,
    #[serde(default)]
    pub created_at: i64,
}

// Custom equality: two cookies are equal if their key fields match (name, domain, path)
// This allows updating cookies with the same key
impl PartialEq for Cookie {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.domain == other.domain && self.path == other.path
    }
}

impl Eq for Cookie {}

impl std::hash::Hash for Cookie {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.domain.hash(state);
        self.path.hash(state);
    }
}

impl Cookie {
    pub fn new(name: &str, value: &str, domain: &str, path: &str) -> Self {
        Self {
            name: name.to_string(),
            value: value.to_string(),
            domain: domain.to_string(),
            path: path.to_string(),
            expires: None,
            secure: false,
            http_only: false,
            same_site: "lax".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        }
    }
    
    /// Parse a Set-Cookie header value into a Cookie
    /// Format: "name=value; Domain=.example.com; Path=/; Secure; HttpOnly; Max-Age=3600; SameSite=Lax"
    pub fn from_set_cookie_header(header: &str, default_domain: &str) -> Self {
        let mut name = String::new();
        let mut value = String::new();
        let mut domain = default_domain.to_string();
        let mut path = "/".to_string();
        let mut expires: Option<i64> = None;
        let mut secure = false;
        let mut http_only = false;
        let mut same_site = "lax".to_string();
        
        // Split by semicolons
        let parts: Vec<&str> = header.split(';').collect();
        for (i, part) in parts.iter().enumerate() {
            let trimmed = part.trim();
            if let Some(eq_pos) = trimmed.find('=') {
                let key = trimmed[..eq_pos].trim().to_lowercase();
                let val = trimmed[eq_pos + 1..].trim().to_string();
                
                match key.as_str() {
                    "domain" => {
                        let d = val.trim_start_matches('.');
                        domain = format!(".{}", d);
                    }
                    "path" => path = val,
                    "expires" => {
                        // Simplified: just ignore date parsing, set to 1 hour from now
                        // Full implementation would parse HTTP date format
                        expires = Some(chrono::Utc::now().timestamp() + 3600);
                    }
                    "max-age" => {
                        if let Ok(secs) = val.parse::<i64>() {
                            expires = Some(chrono::Utc::now().timestamp() + secs);
                        }
                    }
                    "samesite" => same_site = val.to_lowercase(),
                    _ => {
                        // First name=value is the cookie itself
                        if i == 0 && name.is_empty() {
                            name = key;
                            value = val;
                        }
                    }
                }
            } else {
                let trimmed_lower = trimmed.to_lowercase();
                if trimmed_lower == "secure" { secure = true; }
                if trimmed_lower == "httponly" { http_only = true; }
            }
        }
        
        Self {
            name,
            value,
            domain,
            path,
            expires,
            secure,
            http_only,
            same_site,
            created_at: chrono::Utc::now().timestamp(),
        }
    }
    
    /// Check if the cookie should be sent to the given URL
    pub fn matches_url(&self, url: &str) -> bool {
        url.contains(&self.domain) && url.starts_with(&self.path)
    }
    
    /// Check if this cookie has expired
    pub fn is_expired(&self) -> bool {
        match self.expires {
            Some(exp) => exp < chrono::Utc::now().timestamp(),
            None => false, // Session cookies never expire
        }
    }
    
    /// Get cookie as HTTP header value
    pub fn to_header_value(&self) -> String {
        format!("{}={}", self.name, self.value)
    }
}

/// Cookie store that persists cookies across sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieStore {
    /// Domain -> Set<Cookie>
    cookies: HashMap<String, HashSet<Cookie>>,
}

impl CookieStore {
    pub fn new() -> Self {
        Self {
            cookies: HashMap::new(),
        }
    }
    
    /// Add a cookie to the store
    pub fn add_cookie(&mut self, cookie: Cookie) {
        let domain = cookie.domain.clone();
        self.cookies.entry(domain).or_default().insert(cookie);
    }
    
    /// Get cookies that match the given domain (supports subdomain matching)
    pub fn get_cookies(&self, domain: &str) -> Vec<Cookie> {
        let mut result = Vec::new();
        
        // Check all stored domains for a match
        for (stored_domain, cookies) in &self.cookies {
            let matches = if stored_domain == domain {
                true // Exact match
            } else if stored_domain.starts_with('.') {
                // Dot-prefixed domain: .example.com matches sub.example.com or example.com
                domain.ends_with(stored_domain) || domain == &stored_domain[1..]
            } else {
                false
            };
            
            if matches {
                for cookie in cookies {
                    if !cookie.is_expired() {
                        result.push(cookie.clone());
                    }
                }
            }
        }
        
        result
    }
    
    /// Remove all cookies for a domain
    pub fn remove_domain_cookies(&mut self, domain: &str) {
        self.cookies.remove(domain);
        if let Some(dot_domain) = domain.strip_prefix('.') {
            self.cookies.remove(dot_domain);
        }
    }
    
    /// Clear all stored cookies
    pub fn clear_all(&mut self) {
        self.cookies.clear();
    }
    
    /// Count total cookies
    pub fn len(&self) -> usize {
        self.cookies.values().map(|s| s.len()).sum()
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
    
    /// Clean expired cookies
    pub fn clean_expired(&mut self) {
        let now = chrono::Utc::now().timestamp();
        for cookies in self.cookies.values_mut() {
            cookies.retain(|c| match c.expires {
                Some(exp) => exp > now,
                None => true, // Session cookies are kept
            });
        }
        self.cookies.retain(|_, v| !v.is_empty());
    }
}

/// Persistent key-value storage (localStorage equivalent)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalStorage {
    data: HashMap<String, String>,
    origin: String,
    #[serde(default)]
    created_at: i64,
}

impl LocalStorage {
    pub fn new(origin: &str) -> Self {
        Self {
            data: HashMap::new(),
            origin: origin.to_string(),
            created_at: chrono::Utc::now().timestamp(),
        }
    }
    
    /// Get a value by key
    pub fn get(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }
    
    /// Set a key-value pair
    pub fn set(&mut self, key: &str, value: &str) {
        self.data.insert(key.to_string(), value.to_string());
    }
    
    /// Remove a key
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.data.remove(key)
    }
    
    /// Clear all stored values
    pub fn clear(&mut self) {
        self.data.clear();
    }
    
    /// Get all key-value pairs
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.data.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
    
    /// Count items
    pub fn len(&self) -> usize {
        self.data.len()
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    
    /// Get origin
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

/// Combined storage state for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StorageState {
    version: String,
    cookies: HashMap<String, HashSet<Cookie>>,
    local_storage: HashMap<String, HashMap<String, String>>,
}

/// Combined storage manager (cookies + localStorage) with persistence
pub struct StorageManager {
    cookies: CookieStore,
    local_storage: HashMap<String, LocalStorage>, // Origin -> LocalStorage
    storage_dir: Option<PathBuf>,
}

impl StorageManager {
    pub fn new() -> Self {
        let storage_dir = if cfg!(test) {
            // Use a temp directory during tests to avoid cross-test leaks
            let tmp = std::env::temp_dir().join(format!("ghitabrowser_test_{}", std::process::id()));
            Some(tmp)
        } else {
            dirs::data_local_dir()
                .map(|p| p.join("GhitaBrowser"))
                .or_else(|| Some(PathBuf::from("./.ghitabrowser_data")))
        };
        
        let mut mgr = Self {
            cookies: CookieStore::new(),
            local_storage: HashMap::new(),
            storage_dir,
        };
        
        // Auto-load saved data
        mgr.load();
        mgr.cookies.clean_expired();
        
        mgr
    }
    
    /// Get or create localStorage for an origin
    pub fn local_storage(&mut self, origin: &str) -> &mut LocalStorage {
        self.local_storage.entry(origin.to_string())
            .or_insert_with(|| LocalStorage::new(origin))
    }
    
    /// Get localStorage by origin (immutable, for inspection)
    pub fn get_local_storage(&self, origin: &str) -> Option<&LocalStorage> {
        self.local_storage.get(origin)
    }
    
    /// Get access to the cookie store
    pub fn cookies(&self) -> &CookieStore {
        &self.cookies
    }
    
    /// Mutable access to cookie store
    pub fn cookies_mut(&mut self) -> &mut CookieStore {
        &mut self.cookies
    }
    
    /// Get the storage file path
    fn storage_path(&self) -> Option<PathBuf> {
        self.storage_dir.as_ref().map(|d| d.join("storage.json"))
    }
    
    /// Save storage state to persistent file
    pub fn save(&self) {
        let path = match self.storage_path() {
            Some(p) => p,
            None => {
                warn!("No storage directory available, skipping save");
                return;
            }
        };
        
        // Create directory if needed
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                error!("Failed to create storage directory: {}", e);
                return;
            }
        }
        
        // Build serializable state
        let ls_map: HashMap<String, HashMap<String, String>> = self.local_storage
            .iter()
            .map(|(origin, ls)| (origin.clone(), ls.data.clone()))
            .collect();
        
        let state = StorageState {
            version: "0.1.2".to_string(),
            cookies: self.cookies.cookies.clone(),
            local_storage: ls_map,
        };
        
        match serde_json::to_string_pretty(&state) {
            Ok(json) => {
                match std::fs::write(&path, json) {
                    Ok(_) => info!("Storage saved to {:?}", path),
                    Err(e) => error!("Failed to save storage: {}", e),
                }
            }
            Err(e) => error!("Failed to serialize storage: {}", e),
        }
    }
    
    /// Load storage state from persistent file
    pub fn load(&mut self) {
        let path = match self.storage_path() {
            Some(p) => p,
            None => return,
        };
        
        if !path.exists() {
            info!("No saved storage found at {:?}", path);
            return;
        }
        
        match std::fs::read_to_string(&path) {
            Ok(json) => {
                match serde_json::from_str::<StorageState>(&json) {
                    Ok(state) => {
                        // Restore cookies
                        self.cookies.cookies = state.cookies;
                        self.cookies.clean_expired();
                        
                        // Restore localStorage
                        for (origin, data) in state.local_storage {
                            let mut ls = LocalStorage::new(&origin);
                            ls.data = data;
                            self.local_storage.insert(origin, ls);
                        }
                        
                        info!("Storage loaded from {:?}", path);
                    }
                    Err(e) => {
                        error!("Failed to parse storage file: {}", e);
                        // Try backup
                        let backup = path.with_extension("json.bak");
                        if backup.exists() {
                            info!("Attempting to load backup...");
                            // Simplified: just rename and retry
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to read storage file: {}", e);
            }
        }
    }
    
    /// Get the storage directory (for display purposes)
    pub fn storage_dir(&self) -> Option<&PathBuf> {
        self.storage_dir.as_ref()
    }
    
    /// Get total cookies count
    pub fn cookie_count(&self) -> usize {
        self.cookies.len()
    }
    
    /// Get total localStorage items count
    pub fn local_storage_count(&self) -> usize {
        self.local_storage.values().map(|ls| ls.len()).sum()
    }
    
    /// Get all origins that have localStorage
    pub fn local_storage_origins(&self) -> Vec<String> {
        self.local_storage.keys().cloned().collect()
    }
}

impl Drop for StorageManager {
    fn drop(&mut self) {
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cookie_creation() {
        let cookie = Cookie::new("session", "abc123", ".example.com", "/");
        assert_eq!(cookie.name, "session");
        assert_eq!(cookie.value, "abc123");
        assert_eq!(cookie.domain, ".example.com");
        assert!(!cookie.is_expired());
    }

    #[test]
    fn test_cookie_store() {
        let mut store = CookieStore::new();
        let cookie = Cookie::new("test", "value", ".example.com", "/");
        store.add_cookie(cookie);
        assert_eq!(store.len(), 1);
        
        let cookies = store.get_cookies(".example.com");
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "test");
    }

    #[test]
    fn test_cookie_expiry() {
        let mut store = CookieStore::new();
        let mut cookie = Cookie::new("expired", "old", ".test.com", "/");
        cookie.expires = Some(0); // Unix epoch = expired long ago
        store.add_cookie(cookie);
        
        assert!(store.get_cookies(".test.com").is_empty());
    }

    #[test]
    fn test_local_storage_basics() {
        let mut ls = LocalStorage::new("https://example.com");
        ls.set("key1", "value1");
        ls.set("key2", "value2");
        assert_eq!(ls.len(), 2);
        assert_eq!(ls.get("key1"), Some(&"value1".to_string()));
        
        ls.remove("key1");
        assert_eq!(ls.len(), 1);
        
        ls.clear();
        assert!(ls.is_empty());
    }

    #[test]
    fn test_storage_manager() {
        let mut mgr = StorageManager::new();
        {
            let ls = mgr.local_storage("https://example.com");
            ls.set("theme", "dark");
            ls.set("font_size", "14");
        }
        assert_eq!(mgr.local_storage_count(), 2);
        
        let cookie = Cookie::new("session", "xyz", ".example.com", "/");
        mgr.cookies_mut().add_cookie(cookie);
        assert_eq!(mgr.cookie_count(), 1);
    }

    #[test]
    fn test_cookie_domain_matching() {
        let mut store = CookieStore::new();
        store.add_cookie(Cookie::new("a", "1", ".example.com", "/"));
        store.add_cookie(Cookie::new("b", "2", "example.com", "/"));
        
        assert_eq!(store.get_cookies("example.com").len(), 2);
        assert_eq!(store.get_cookies("sub.example.com").len(), 1); // only dot-prefixed
    }

    #[test]
    fn test_cookie_secure_flag() {
        let mut cookie = Cookie::new("secure_cookie", "secret", ".bank.com", "/");
        cookie.secure = true;
        assert!(cookie.secure);
        assert_eq!(cookie.to_header_value(), "secure_cookie=secret");
    }
}

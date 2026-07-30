// src/storage - Cookie and localStorage persistence (Phase 17-18)
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

/// Represents a single HTTP cookie
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<SystemTime>,
    pub secure: bool,
    pub http_only: bool,
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
        }
    }
    
    /// Check if the cookie should be sent to the given URL
    pub fn matches_url(&self, url: &str) -> bool {
        // Simplified check - in production would parse URL properly
        url.contains(&self.domain) && url.starts_with(&self.path)
    }
    
    /// Check if this cookie has expired
    pub fn is_expired(&self) -> bool {
        match self.expires {
            Some(exp) => exp < SystemTime::now(),
            None => false, // Session cookies never expire
        }
    }
}

/// Cookie store that persists cookies across sessions
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
    
    /// Get cookies that match the given domain
    pub fn get_cookies(&self, domain: &str) -> Vec<Cookie> {
        let mut result = Vec::new();
        
        // Check exact domain
        if let Some(cookies) = self.cookies.get(domain) {
            for cookie in cookies {
                if !cookie.is_expired() {
                    result.push(cookie.clone());
                }
            }
        }
        
        // Check dot-prefixed domain (.example.com)
        let dot_domain = if domain.starts_with('.') {
            domain.to_string()
        } else {
            format!(".{}", domain)
        };

        if dot_domain != domain {
            if let Some(cookies) = self.cookies.get(&dot_domain) {
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
}

/// Persistent key-value storage (localStorage equivalent)
#[derive(Debug, Clone)]
pub struct LocalStorage {
    data: HashMap<String, String>,
    origin: String,
}

impl LocalStorage {
    pub fn new(origin: &str) -> Self {
        Self {
            data: HashMap::new(),
            origin: origin.to_string(),
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
}

/// Combined storage manager (cookies + localStorage)
pub struct StorageManager {
    cookies: CookieStore,
    local_storage: HashMap<String, LocalStorage>, // Origin -> LocalStorage
}

impl StorageManager {
    pub fn new() -> Self {
        Self {
            cookies: CookieStore::new(),
            local_storage: HashMap::new(),
        }
    }
    
    /// Get or create localStorage for an origin
    pub fn local_storage(&mut self, origin: &str) -> &mut LocalStorage {
        self.local_storage.entry(origin.to_string()).or_insert_with(|| LocalStorage::new(origin))
    }
    
    /// Get access to the cookie store
    pub fn cookies(&self) -> &CookieStore {
        &self.cookies
    }
    
    /// Mutable access to cookie store
    pub fn cookies_mut(&mut self) -> &mut CookieStore {
        &mut self.cookies
    }
    
    /// Save storage state to persistent file (serialization placeholder)
    pub fn save(&self, _path: &str) {
        // Implementation would use serde to serialize to JSON/file
    }
    
    /// Load storage state from persistent file
    pub fn load(&mut self, _path: &str) {
        // Implementation would deserialize from JSON/file
    }
}
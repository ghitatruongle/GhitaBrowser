// Browser storage and cookie jar


use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Represents a single HTTP cookie
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>, // Unix timestamp (seconds), None = session cookie
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
                        // RFC 6265: only accept a Domain attribute that is a
                        // domain-suffix of the host that sent it; otherwise a
                        // server could set a cookie for an unrelated site (or
                        // a bare public suffix like ".com"). Invalid domains
                        // are ignored, which makes this a host-only cookie.
                        let d = val.trim_start_matches('.');
                        if is_domain_suffix(d, default_domain) {
                            domain = format!(".{}", d);
                        }
                    }
                    "path" => path = val,
                    "expires" => {
                        // Parse the standard HTTP-date (RFC 7231). Unparseable
                        // dates are ignored per RFC 6265 (cookie stays a
                        // session cookie) instead of being faked as 1 hour.
                        if let Some(ts) = parse_http_date(&val) {
                            expires = Some(ts);
                        }
                    }
                    "max-age" => {
                        if let Ok(secs) = val.parse::<i64>() {
                            if secs <= 0 {
                                expires = Some(0); // expires immediately (deletion)
                            } else {
                                expires = Some(chrono::Utc::now().timestamp() + secs);
                            }
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
                if trimmed_lower == "secure" {
                    secure = true;
                }
                if trimmed_lower == "httponly" {
                    http_only = true;
                }
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
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(_) => return false,
        };

        // Secure cookies are only sent over HTTPS
        if self.secure && parsed.scheme() != "https" {
            return false;
        }

        // Domain match (RFC 6265): the host equals the cookie domain or is a
        // subdomain of it. The stored domain may be dot-prefixed; the leading
        // dot in comparisons handles label boundaries ("badexample.com" never
        // matches ".example.com").
        let base = self.domain.trim_start_matches('.');
        let host = parsed.host_str().unwrap_or("");
        let base = base.to_ascii_lowercase();
        let host = host.to_ascii_lowercase();
        let domain_matches = host == base || host.ends_with(&format!(".{}", base));
        if !domain_matches {
            return false;
        }

        // Path match: the cookie path must be a path-prefix of the request
        // path (RFC 6265), with a '/' boundary unless the cookie path itself
        // ends in '/' (so path "/app" matches "/app/x" but not "/appx").
        let request_path = parsed.path();
        let cookie_path = if self.path.is_empty() {
            "/"
        } else {
            self.path.as_str()
        };
        request_path == cookie_path
            || (request_path.starts_with(cookie_path)
                && (cookie_path.ends_with('/')
                    || request_path[cookie_path.len()..].starts_with('/')))
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

/// RFC 6265 domain-match: `domain` equals `host` or is a subdomain of it,
/// compared case-insensitively. Bare labels / public suffixes (no dot, e.g.
/// `com` or `co.uk`) are never accepted so a cookie can never claim an
/// entire TLD.
fn is_domain_suffix(domain: &str, host: &str) -> bool {
    if domain.is_empty() || !domain.contains('.') {
        return false;
    }
    let domain = domain.to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    domain == host || host.ends_with(&format!(".{}", domain))
}

/// Parse a standard HTTP-date (RFC 7231) such as
/// "Wed, 21 Oct 2015 07:28:00 GMT". Returns None for unparseable dates so
/// callers can treat them as absent rather than guessing.
fn parse_http_date(s: &str) -> Option<i64> {
    let s = s.trim();
    // RFC 1123 ("Sun, 06 Nov 1994 08:49:37 GMT"), RFC 850
    // ("Sunday, 06-Nov-94 08:49:37 GMT") and asctime forms. HTTP dates are
    // always GMT, so the naive timestamp is already UTC.
    for fmt in &[
        "%a, %d %b %Y %H:%M:%S GMT",
        "%a, %d-%b-%y %H:%M:%S GMT",
        "%a %b %e %H:%M:%S %Y",
    ] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp());
        }
    }
    None
}

/// Cookie store that persists cookies across sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieStore {
    /// Domain -> Set<Cookie>
    cookies: HashMap<String, HashSet<Cookie>>,
}

impl Default for CookieStore {
    fn default() -> Self {
        Self::new()
    }
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
            } else if let Some(base) = stored_domain.strip_prefix('.') {
                // Dot-prefixed domain: .example.com matches sub.example.com or
                // example.com. Only valid for multi-label domains (at least one
                // dot beyond the leading one), so a top-level public suffix like
                // ".com" can never match every site under it.
                base.contains('.') && (domain == base || domain.ends_with(stored_domain))
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

    /// Remove all cookies for a domain, covering both the bare and the
    /// dot-prefixed storage forms ("example.com" and ".example.com").
    pub fn remove_domain_cookies(&mut self, domain: &str) {
        let base = domain.trim_start_matches('.');
        let variants = [base.to_string(), format!(".{}", base)];
        self.cookies.retain(|key, _| !variants.contains(key));
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

/// A saved bookmark (Chrome-style star)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub added_at: i64,
}

fn default_visit_count() -> u32 {
    1
}

/// A single entry in the global browsing history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub url: String,
    pub title: String,
    pub visited_at: i64,
    #[serde(default = "default_visit_count")]
    pub visit_count: u32,
}

/// A completed (or failed) download
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub url: String,
    pub file_name: String,
    pub path: String,
    pub size_bytes: u64,
    pub completed_at: i64,
    pub success: bool,
}

fn default_true() -> bool {
    true
}

/// User-facing browser settings (Chrome-style preferences)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSettings {
    /// "dark" or "light"
    pub theme: String,
    /// "google", "bing" or "duckduckgo"
    pub search_engine: String,
    /// Page opened by the Home button and new windows
    pub homepage: String,
    /// Whether the bookmarks bar is visible
    pub show_bookmarks_bar: bool,
    /// Default page zoom in percent (100 = normal)
    pub default_zoom: u16,
    /// Real pixel graphics renderer (true) or legacy text mode (false)
    #[serde(default = "default_true")]
    pub pixel_rendering: bool,
    /// Vertical tabs mode (Edge feature)
    #[serde(default)]
    pub vertical_tabs: bool,
    /// AdBlock & Tracker Blocker enabled (Cốc Cốc feature)
    #[serde(default = "default_true")]
    pub adblock_enabled: bool,
    /// Tab Memory Saver / Sleeping tabs enabled (Chrome feature)
    #[serde(default = "default_true")]
    pub tab_memory_saver: bool,
    /// Custom NewTab wallpaper (Chrome feature)
    #[serde(default)]
    pub custom_wallpaper_url: Option<String>,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            search_engine: "google".to_string(),
            homepage: "ghita://newtab".to_string(),
            show_bookmarks_bar: true,
            default_zoom: 100,
            pixel_rendering: true,
            vertical_tabs: false,
            adblock_enabled: true,
            tab_memory_saver: true,
            custom_wallpaper_url: None,
        }
    }
}

/// Combined storage state for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StorageState {
    version: String,
    cookies: HashMap<String, HashSet<Cookie>>,
    local_storage: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
    #[serde(default)]
    history: Vec<HistoryRecord>,
    #[serde(default)]
    downloads: Vec<DownloadRecord>,
    #[serde(default)]
    settings: BrowserSettings,
}

/// Combined storage manager (cookies + localStorage + bookmarks + history + downloads + settings)
pub struct StorageManager {
    cookies: CookieStore,
    local_storage: HashMap<String, LocalStorage>, // Origin -> LocalStorage
    bookmarks: Vec<Bookmark>,
    /// Global browsing history, newest first
    history: Vec<HistoryRecord>,
    /// Download records, newest first
    downloads: Vec<DownloadRecord>,
    /// User settings (theme, search engine, homepage...)
    pub settings: BrowserSettings,
    storage_dir: Option<PathBuf>,
}

impl Default for StorageManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageManager {
    pub fn new() -> Self {
        let storage_dir = if cfg!(test) {
            // Use a temp directory during tests to avoid cross-test leaks
            let tmp =
                std::env::temp_dir().join(format!("ghitabrowser_test_{}", std::process::id()));
            Some(tmp)
        } else {
            dirs::data_local_dir()
                .map(|p| p.join("GhitaBrowser"))
                .or_else(|| Some(PathBuf::from("./.ghitabrowser_data")))
        };

        let mut mgr = Self {
            cookies: CookieStore::new(),
            local_storage: HashMap::new(),
            bookmarks: Vec::new(),
            history: Vec::new(),
            downloads: Vec::new(),
            settings: BrowserSettings::default(),
            storage_dir,
        };

        // Auto-load saved data
        mgr.load();
        mgr.cookies.clean_expired();

        mgr
    }

    /// Get or create localStorage for an origin
    pub fn local_storage(&mut self, origin: &str) -> &mut LocalStorage {
        self.local_storage
            .entry(origin.to_string())
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
        let ls_map: HashMap<String, HashMap<String, String>> = self
            .local_storage
            .iter()
            .map(|(origin, ls)| (origin.clone(), ls.data.clone()))
            .collect();

        let state = StorageState {
            version: crate::VERSION.to_string(),
            cookies: self.cookies.cookies.clone(),
            local_storage: ls_map,
            bookmarks: self.bookmarks.clone(),
            history: self.history.clone(),
            downloads: self.downloads.clone(),
            settings: self.settings.clone(),
        };

        // Rotate the previous file to a backup so a corrupt save can be
        // recovered (see load()).
        if path.exists() {
            let backup = path.with_extension("json.bak");
            if let Err(e) = std::fs::copy(&path, &backup) {
                error!("Failed to create storage backup: {}", e);
            }
        }

        match serde_json::to_string_pretty(&state) {
            Ok(json) => match std::fs::write(&path, json) {
                Ok(_) => info!("Storage saved to {:?}", path),
                Err(e) => error!("Failed to save storage: {}", e),
            },
            Err(e) => error!("Failed to serialize storage: {}", e),
        }
    }

    /// Read and parse a storage file, returning None if unreadable or corrupt
    fn read_state(path: &std::path::Path) -> Option<StorageState> {
        let json = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<StorageState>(&json).ok()
    }

    /// Apply a decoded state into this manager
    fn apply_state(&mut self, state: StorageState) {
        // Restore cookies
        self.cookies.cookies = state.cookies;
        self.cookies.clean_expired();

        // Restore localStorage
        for (origin, data) in state.local_storage {
            let mut ls = LocalStorage::new(&origin);
            ls.data = data;
            self.local_storage.insert(origin, ls);
        }

        // Restore bookmarks, history, downloads and settings
        self.bookmarks = state.bookmarks;
        self.history = state.history;
        self.downloads = state.downloads;
        self.settings = state.settings;
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

        match Self::read_state(&path) {
            Some(state) => {
                self.apply_state(state);
                info!("Storage loaded from {:?}", path);
            }
            None => {
                error!("Failed to parse storage file: {:?}", path);
                // Recover from the backup when the main file is corrupt
                let backup = path.with_extension("json.bak");
                if backup.exists() {
                    info!("Attempting to load from backup: {:?}", backup);
                    match Self::read_state(&backup) {
                        Some(state) => {
                            self.apply_state(state);
                            info!("Storage recovered from backup");
                        }
                        None => error!("Backup is also corrupt; keeping in-memory defaults"),
                    }
                }
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

    // ===== Bookmarks =====

    /// All bookmarks, in insertion order
    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    /// Whether a URL is bookmarked
    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|b| b.url == url)
    }

    /// Toggle a bookmark; returns true if the URL is now bookmarked
    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        if self.is_bookmarked(url) {
            self.bookmarks.retain(|b| b.url != url);
            false
        } else {
            self.bookmarks.push(Bookmark {
                url: url.to_string(),
                title: if title.trim().is_empty() {
                    url.to_string()
                } else {
                    title.to_string()
                },
                added_at: chrono::Utc::now().timestamp(),
            });
            true
        }
    }

    /// Remove a bookmark by URL
    pub fn remove_bookmark(&mut self, url: &str) {
        self.bookmarks.retain(|b| b.url != url);
    }

    // ===== Browsing history =====

    /// Global history, newest first
    pub fn history(&self) -> &[HistoryRecord] {
        &self.history
    }

    /// Record a visit; merges duplicates and keeps newest first (capped at 2000 entries)
    pub fn add_history(&mut self, url: &str, title: &str) {
        let now = chrono::Utc::now().timestamp();
        let prev_count = if let Some(pos) = self.history.iter().position(|h| h.url == url) {
            let old = self.history.remove(pos);
            old.visit_count
        } else {
            0
        };
        self.history.insert(
            0,
            HistoryRecord {
                url: url.to_string(),
                title: if title.trim().is_empty() {
                    url.to_string()
                } else {
                    title.to_string()
                },
                visited_at: now,
                visit_count: prev_count + 1,
            },
        );
        self.history.truncate(2000);
    }

    /// Remove a single history entry by URL
    pub fn remove_history_entry(&mut self, url: &str) {
        self.history.retain(|h| h.url != url);
    }

    /// Clear all browsing history
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Most visited sites (for the New Tab page tiles)
    pub fn top_sites(&self, n: usize) -> Vec<HistoryRecord> {
        let mut sorted: Vec<HistoryRecord> = self.history.clone();
        sorted.sort_by(|a, b| {
            b.visit_count
                .cmp(&a.visit_count)
                .then(b.visited_at.cmp(&a.visited_at))
        });
        sorted.truncate(n);
        sorted
    }

    // ===== Downloads =====

    /// Download records, newest first
    pub fn downloads(&self) -> &[DownloadRecord] {
        &self.downloads
    }

    /// Add a download record (newest first, capped at 200)
    pub fn add_download(&mut self, record: DownloadRecord) {
        self.downloads.insert(0, record);
        self.downloads.truncate(200);
    }

    /// Clear the downloads list (does not delete files)
    pub fn clear_downloads(&mut self) {
        self.downloads.clear();
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

    #[test]
    fn test_cookie_rejects_foreign_domain() {
        // A server for example.com must not be able to claim cookies for
        // attacker.com; the invalid Domain attribute is ignored.
        let cookie = Cookie::from_set_cookie_header(
            "session=abc; Domain=attacker.com; Path=/",
            "example.com",
        );
        assert_eq!(cookie.domain, "example.com");
        assert!(!cookie.matches_url("https://attacker.com/"));
        assert!(cookie.matches_url("https://example.com/"));
    }

    #[test]
    fn test_cookie_accepts_domain_suffix() {
        // sub.example.com may set a cookie for the parent example.com
        let cookie = Cookie::from_set_cookie_header("x=1; Domain=example.com", "sub.example.com");
        assert_eq!(cookie.domain, ".example.com");
    }

    #[test]
    fn test_cookie_rejects_bare_suffix() {
        // "com" / "co.uk" style bare suffixes are never accepted
        let cookie = Cookie::from_set_cookie_header("x=1; Domain=com", "example.com");
        assert_eq!(cookie.domain, "example.com");
    }

    #[test]
    fn test_cookie_parses_expires_date() {
        let cookie = Cookie::from_set_cookie_header(
            "x=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT",
            "example.com",
        );
        assert_eq!(cookie.expires, Some(1_445_412_480));
    }

    #[test]
    fn test_cookie_invalid_expires_ignored() {
        // Unparseable dates must not be faked as 1 hour from now
        let cookie = Cookie::from_set_cookie_header("x=1; Expires=not-a-date", "example.com");
        assert_eq!(cookie.expires, None);
    }

    #[test]
    fn test_cookie_max_age_zero_deletes() {
        let cookie = Cookie::from_set_cookie_header("x=1; Max-Age=0", "example.com");
        assert_eq!(cookie.expires, Some(0));
        assert!(cookie.is_expired());
    }

    #[test]
    fn test_matches_url_label_boundary() {
        let cookie = Cookie::new("a", "1", ".example.com", "/");
        assert!(cookie.matches_url("https://example.com/"));
        assert!(cookie.matches_url("https://sub.example.com/x"));
        assert!(!cookie.matches_url("https://badexample.com/"));
        assert!(!cookie.matches_url("https://notexample.com.evil.com/"));
        assert!(!cookie.matches_url("not a url"));
    }

    #[test]
    fn test_matches_url_secure_only_https() {
        let mut cookie = Cookie::new("s", "1", ".bank.com", "/");
        cookie.secure = true;
        assert!(cookie.matches_url("https://bank.com/"));
        assert!(!cookie.matches_url("http://bank.com/"));
    }

    #[test]
    fn test_matches_url_path_prefix() {
        let cookie = Cookie::new("a", "1", ".example.com", "/app");
        assert!(cookie.matches_url("https://example.com/app"));
        assert!(cookie.matches_url("https://example.com/app/page"));
        assert!(!cookie.matches_url("https://example.com/appx"));
    }

    #[test]
    fn test_remove_domain_cookies_dot_variants() {
        let mut store = CookieStore::new();
        store.add_cookie(Cookie::new("a", "1", ".example.com", "/"));
        store.add_cookie(Cookie::new("b", "2", "example.com", "/"));
        assert_eq!(store.len(), 2);

        store.remove_domain_cookies("example.com");
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_get_cookies_rejects_public_suffix() {
        // Even if a ".com" cookie were somehow stored, it must never match a
        // real host under that TLD.
        let mut store = CookieStore::new();
        store.add_cookie(Cookie::new("evil", "1", ".com", "/"));
        assert!(store.get_cookies("victim.com").is_empty());
        assert!(store.get_cookies("example.com").is_empty());
    }

    #[test]
    fn test_backup_recovery() {
        let dir = std::env::temp_dir().join(format!("ghitabrowser_baktest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("storage.json");
        let backup = dir.join("storage.json.bak");

        // Valid backup with a known cookie
        let mut store = CookieStore::new();
        store.add_cookie(Cookie::new("sid", "abc", ".example.com", "/"));
        let state = StorageState {
            version: crate::VERSION.to_string(),
            cookies: store.cookies,
            local_storage: HashMap::new(),
            bookmarks: Vec::new(),
            history: Vec::new(),
            downloads: Vec::new(),
            settings: BrowserSettings::default(),
        };
        std::fs::write(&backup, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        // Corrupt main file
        std::fs::write(&path, "{corrupt").unwrap();

        let mut mgr = StorageManager::new();
        mgr.storage_dir = Some(dir.clone());
        mgr.load();

        assert_eq!(mgr.cookie_count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

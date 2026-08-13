// String interning pool for memory-efficient string storage.
//
// String interning deduplicates identical strings by storing only one copy
// and sharing it via Rc<str>. This is especially effective for:
// - HTML tag names ("div", "p", "a", "span", etc.) — repeated thousands of times
// - Attribute names ("class", "id", "href", "src", etc.)
// - Common class names and URL fragments
//
// Typical savings: 10-20% of total DOM memory on tag-heavy pages.

use std::collections::HashMap;
use std::sync::Arc;

/// A string interning pool that deduplicates identical strings.
///
/// Uses `Arc<str>` for thread-safe shared ownership. Once a string is interned,
/// all subsequent requests for the same string return a clone of the same Arc,
/// avoiding duplicate allocations.
#[derive(Debug, Clone, Default)]
pub struct StringPool {
    inner: HashMap<Arc<str>, Arc<str>>,
}

impl StringPool {
    /// Create a new empty string pool.
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Create a new string pool with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: HashMap::with_capacity(capacity),
        }
    }

    /// Intern a string: returns a shared `Arc<str>` that is identical for
    /// identical inputs. If the string was already interned, returns a clone
    /// of the existing allocation. Otherwise, inserts and returns it.
    pub fn intern(&mut self, s: &str) -> Arc<str> {
        if let Some(existing) = self.inner.get(s) {
            return existing.clone();
        }
        let arc: Arc<str> = Arc::from(s);
        self.inner.insert(arc.clone(), arc.clone());
        arc
    }

    /// Intern a string from an owned `String`. Avoids an extra allocation
    /// if the string is already interned.
    pub fn intern_owned(&mut self, s: String) -> Arc<str> {
        if let Some(existing) = self.inner.get(s.as_str()) {
            return existing.clone();
        }
        let arc: Arc<str> = Arc::from(s);
        self.inner.insert(arc.clone(), arc.clone());
        arc
    }

    /// Check if a string has already been interned.
    pub fn contains(&self, s: &str) -> bool {
        self.inner.contains_key(s)
    }

    /// Get the number of unique strings in the pool.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clear all interned strings.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Get the total capacity of the underlying HashMap.
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Pre-intern a set of common strings (HTML tag names, attribute names).
    /// Call this once at startup to ensure the most common strings are shared.
    pub fn preload_common(&mut self) {
        // HTML5 tag names (most common)
        for tag in &[
            "div",
            "span",
            "p",
            "a",
            "img",
            "ul",
            "ol",
            "li",
            "table",
            "tr",
            "td",
            "th",
            "thead",
            "tbody",
            "tfoot",
            "form",
            "input",
            "button",
            "select",
            "option",
            "textarea",
            "label",
            "fieldset",
            "legend",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "header",
            "footer",
            "nav",
            "main",
            "section",
            "article",
            "aside",
            "figure",
            "figcaption",
            "details",
            "summary",
            "dialog",
            "menu",
            "iframe",
            "script",
            "style",
            "link",
            "meta",
            "title",
            "head",
            "body",
            "html",
            "br",
            "hr",
            "wbr",
            "source",
            "track",
            "embed",
            "object",
            "param",
            "video",
            "audio",
            "canvas",
            "map",
            "area",
            "col",
            "colgroup",
            "caption",
            "datalist",
            "optgroup",
            "output",
            "progress",
            "meter",
            "time",
            "data",
            "abbr",
            "address",
            "blockquote",
            "cite",
            "code",
            "del",
            "ins",
            "dfn",
            "em",
            "strong",
            "small",
            "s",
            "sub",
            "sup",
            "mark",
            "q",
            "samp",
            "kbd",
            "var",
            "b",
            "i",
            "u",
            "ruby",
            "rt",
            "rp",
            "bdi",
            "bdo",
            "span",
            "div",
            "root", // synthetic root used by parser
        ] {
            self.intern(tag);
        }

        // Common HTML attribute names
        for attr in &[
            "id",
            "class",
            "style",
            "src",
            "href",
            "alt",
            "title",
            "name",
            "value",
            "type",
            "placeholder",
            "disabled",
            "readonly",
            "required",
            "checked",
            "selected",
            "multiple",
            "size",
            "maxlength",
            "min",
            "max",
            "step",
            "pattern",
            "autofocus",
            "autocomplete",
            "novalidate",
            "formaction",
            "formmethod",
            "formtarget",
            "formenctype",
            "enctype",
            "method",
            "action",
            "target",
            "rel",
            "media",
            "charset",
            "content",
            "http-equiv",
            "property",
            "role",
            "aria-label",
            "aria-hidden",
            "aria-expanded",
            "aria-controls",
            "data-toggle",
            "data-target",
            "data-dismiss",
            "data-backdrop",
            "data-keyboard",
            "data-loading",
            "data-animation",
            "data-delay",
            "data-offset",
            "data-spy",
            "data-offset-top",
            "data-offset-bottom",
            "width",
            "height",
            "colspan",
            "rowspan",
            "scope",
            "headers",
            "abbr",
            "sorted",
            "reversed",
            "start",
            "download",
            "ping",
            "hreflang",
            "referrerpolicy",
            "loading",
            "decoding",
            "crossorigin",
            "integrity",
            "nonce",
            "async",
            "defer",
            "nomodule",
            "preload",
            "prefetch",
            "preconnect",
            "dns-picfetch",
        ] {
            self.intern(attr);
        }
    }
}

/// Global string pool for convenient access.
///
/// This is the primary way to use string interning in the parser.
/// The pool is lazily initialized on first use and shared across all
/// parsing operations via Arc<Mutex<...>>.
pub fn global_pool() -> std::sync::Arc<std::sync::Mutex<StringPool>> {
    use std::sync::{Arc, Mutex, OnceLock};

    static POOL: OnceLock<Arc<Mutex<StringPool>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let mut pool = StringPool::with_capacity(256);
        pool.preload_common();
        Arc::new(Mutex::new(pool))
    })
    .clone()
}

/// Intern a string using the global pool.
pub fn intern(s: &str) -> Arc<str> {
    global_pool().lock().unwrap().intern(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_basic() {
        let mut pool = StringPool::new();
        let s1 = pool.intern("div");
        let s2 = pool.intern("div");
        let s3 = pool.intern("span");

        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn test_pool_deduplication() {
        let mut pool = StringPool::new();
        let s1 = pool.intern("hello world");
        let s2 = pool.intern("hello world");
        let s3 = pool.intern("hello world");

        // All three should point to the same allocation
        assert!(Arc::ptr_eq(&s1, &s2));
        assert!(Arc::ptr_eq(&s2, &s3));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_pool_intern_owned() {
        let mut pool = StringPool::new();
        let s1 = pool.intern("div");
        let s2 = pool.intern_owned("div".to_string());

        assert!(Arc::ptr_eq(&s1, &s2));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_pool_contains() {
        let mut pool = StringPool::new();
        pool.intern("div");

        assert!(pool.contains("div"));
        assert!(!pool.contains("span"));
    }

    #[test]
    fn test_pool_preload() {
        let mut pool = StringPool::new();
        pool.preload_common();

        // Should have preloaded tag names and attribute names
        assert!(pool.len() > 50);
        assert!(pool.contains("div"));
        assert!(pool.contains("class"));
        assert!(pool.contains("href"));
    }

    #[test]
    fn test_pool_clear() {
        let mut pool = StringPool::new();
        pool.intern("div");
        pool.intern("span");
        assert_eq!(pool.len(), 2);

        pool.clear();
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn test_pool_empty() {
        let pool = StringPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn test_pool_capacity() {
        let pool = StringPool::with_capacity(100);
        assert!(pool.capacity() >= 100);
    }

    #[test]
    fn test_global_pool_interning() {
        let s1 = intern("div");
        let s2 = intern("div");
        let s3 = intern("span");

        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }
}

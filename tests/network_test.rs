// Network integration tests
use ghitabrowser::network::{FetchResult, ResourceCache};
use std::collections::HashMap;

fn make_fetch_result(body: &str, url: &str) -> FetchResult {
    FetchResult {
        body: body.to_string(),
        binary_body: None,
        url: url.to_string(),
        status_code: 200,
        content_type: "text/html".to_string(),
        headers: HashMap::new(),
        fetch_time_ms: 10,
        set_cookie_headers: vec![],
    }
}

#[test]
fn test_resource_cache_insert_and_get() {
    let mut cache = ResourceCache::new();
    let url = "https://example.com";
    let result = make_fetch_result("Hello World!", url);

    cache.insert(url, result.clone(), 3600);

    assert!(cache.get(url).is_some());
    assert_eq!(cache.get(url).unwrap().result.body, "Hello World!");
}

#[test]
fn test_resource_cache_expiration() {
    let mut cache = ResourceCache::new();
    let url = "https://test.com";
    let result = make_fetch_result("Expired Test", url);

    cache.insert(url, result, 0); // TTL = 0, expires immediately

    assert!(cache.get(url).unwrap().is_expired());
}

#[test]
fn test_resource_cache_size_limit() {
    let mut cache = ResourceCache::new();
    cache.max_size = 100;

    let large_body = "x".repeat(200);
    let result = make_fetch_result(&large_body, "https://large.com");
    cache.insert("https://large.com", result, 3600);

    assert!(cache.get("https://large.com").is_some());
}

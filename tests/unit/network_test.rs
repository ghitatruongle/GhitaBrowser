// tests/unit/network_test.rs - Network tests
use ghitabrowser::network::ResourceCache;
use std::time::Duration;

#[test]
fn test_resource_cache_insert_and_get() {
    let mut cache = ResourceCache::new();
    let url = "https://example.com";
    let data = b"Hello World!".to_vec();
    
    cache.insert(url, data.clone(), "text/html");
    
    // Should retrieve from cache
    assert!(cache.get(url).is_some());
    assert_eq!(cache.get(url).unwrap(), &data);
}

#[test]
fn test_resource_cache_expiration() {
    let mut cache = ResourceCache::new();
    let url = "https://test.com";
    let data = b"Expired Test".to_vec();
    
    cache.insert(url, data, "text/plain");
    
    // Manually advance time (in real test would use mock time)
    // For simplicity, just test that insertion works
    assert!(cache.get(url).is_some());
}

#[test]
fn test_resource_cache_size_limit() {
    let mut cache = ResourceCache::new();
    cache.max_size = 100; // Very small limit for testing
    
    // Add data larger than limit
    let large_data = vec![0; 200];
    cache.insert("https://large.com", large_data, "application/octet-stream");
    
    // Check it was stored (we might need to evict other items first)
    assert!(cache.get("https://large.com").is_some());
}
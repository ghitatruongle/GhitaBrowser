//! Integration tests for Phase 22 — Storage Quotas, Origin Partitioning, and Private Data Clearing.

use ghitabrowser::storage_quota::{StorageCategory, StorageQuotaManager};

#[test]
fn storage_quota_enforcement_and_exceeded_error() {
    let mut sqm = StorageQuotaManager::new();
    let origin = "https://quota.example.com";

    // 10 MB usage passes under 32 MB default quota
    sqm.update_usage(origin, StorageCategory::LocalStorage, 10 * 1024 * 1024);
    assert!(sqm.check_quota(origin, 10 * 1024 * 1024).is_ok());

    // Additional 20 MB (total 40 MB) exceeds 32 MB quota
    let err = sqm
        .check_quota(origin, 25 * 1024 * 1024)
        .expect_err("quota exceeded");
    assert!(err.contains("QuotaExceededError"));
}

#[test]
fn origin_and_profile_data_clearing() {
    let mut sqm = StorageQuotaManager::new();

    sqm.update_usage("https://origin1.com", StorageCategory::IndexedDB, 1000);
    sqm.update_usage("https://origin2.com", StorageCategory::CacheAPI, 2000);

    assert_eq!(sqm.origins.len(), 2);

    // Clear origin1
    assert!(sqm.clear_origin_data("https://origin1.com"));
    assert_eq!(sqm.origins.len(), 1);
    assert!(sqm.origins.contains_key("https://origin2.com"));

    // Clear entire profile
    sqm.clear_profile_data();
    assert_eq!(sqm.origins.len(), 0);
}

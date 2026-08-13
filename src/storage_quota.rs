//! Bounded clean-room Storage Quota Manager for GhitaBrowser (Phase 22)
//! Implements origin storage quotas, eviction tracking, and private session clear data.

use std::collections::HashMap;

pub const DEFAULT_ORIGIN_QUOTA_BYTES: usize = 32 * 1024 * 1024; // 32 MB default per origin

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCategory {
    LocalStorage,
    IndexedDB,
    CacheAPI,
}

#[derive(Debug, Clone)]
pub struct OriginStorageUsage {
    pub origin: String,
    pub local_storage_bytes: usize,
    pub indexeddb_bytes: usize,
    pub cache_storage_bytes: usize,
    pub quota_bytes: usize,
}

impl OriginStorageUsage {
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            local_storage_bytes: 0,
            indexeddb_bytes: 0,
            cache_storage_bytes: 0,
            quota_bytes: DEFAULT_ORIGIN_QUOTA_BYTES,
        }
    }

    pub fn total_usage(&self) -> usize {
        self.local_storage_bytes + self.indexeddb_bytes + self.cache_storage_bytes
    }
}

#[derive(Debug, Default)]
pub struct StorageQuotaManager {
    pub origins: HashMap<String, OriginStorageUsage>,
}

impl StorageQuotaManager {
    pub fn new() -> Self {
        Self {
            origins: HashMap::new(),
        }
    }

    pub fn check_quota(&mut self, origin: &str, additional_bytes: usize) -> Result<(), String> {
        let usage = self
            .origins
            .entry(origin.to_string())
            .or_insert_with(|| OriginStorageUsage::new(origin));

        if usage.total_usage() + additional_bytes > usage.quota_bytes {
            Err("QuotaExceededError: Origin storage quota exceeded".to_string())
        } else {
            Ok(())
        }
    }

    pub fn update_usage(&mut self, origin: &str, category: StorageCategory, bytes: usize) {
        let usage = self
            .origins
            .entry(origin.to_string())
            .or_insert_with(|| OriginStorageUsage::new(origin));

        match category {
            StorageCategory::LocalStorage => usage.local_storage_bytes = bytes,
            StorageCategory::IndexedDB => usage.indexeddb_bytes = bytes,
            StorageCategory::CacheAPI => usage.cache_storage_bytes = bytes,
        }
    }

    pub fn clear_origin_data(&mut self, origin: &str) -> bool {
        self.origins.remove(origin).is_some()
    }

    pub fn clear_profile_data(&mut self) {
        self.origins.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_quota_enforcement_and_clearing() {
        let mut sqm = StorageQuotaManager::new();
        let origin = "https://example.com";

        // Under quota passes
        sqm.update_usage(origin, StorageCategory::IndexedDB, 10 * 1024 * 1024);
        assert!(sqm.check_quota(origin, 5 * 1024 * 1024).is_ok());

        // Overflow quota fails
        assert!(sqm.check_quota(origin, 30 * 1024 * 1024).is_err());

        // Clear data removes origin entry
        assert!(sqm.clear_origin_data(origin));
        assert!(!sqm.origins.contains_key(origin));
    }
}

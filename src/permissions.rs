// Permissions Framework & Origin Permission Store for GhitaBrowser (Phase 25).
// Implements origin-partitioned Web API permissions (Prompt, Granted, Denied) and JSON persistence.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionType {
    Geolocation,
    Notifications,
    Camera,
    Microphone,
    ClipboardRead,
    ClipboardWrite,
    PersistentStorage,
    Midi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionState {
    Prompt,
    Granted,
    Denied,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct PermissionStore {
    pub store: HashMap<String, HashMap<PermissionType, PermissionState>>,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self {
            store: HashMap::new(),
        }
    }

    pub fn get_permission(&self, origin: &str, perm: PermissionType) -> PermissionState {
        let Some(origin) = canonical_permission_origin(origin) else {
            return PermissionState::Denied;
        };
        if let Some(origin_perms) = self.store.get(&origin) {
            origin_perms
                .get(&perm)
                .copied()
                .unwrap_or(PermissionState::Prompt)
        } else {
            PermissionState::Prompt
        }
    }

    pub fn set_permission(
        &mut self,
        origin: impl Into<String>,
        perm: PermissionType,
        state: PermissionState,
    ) -> Result<(), String> {
        let origin = canonical_permission_origin(&origin.into())
            .ok_or_else(|| "Permissions require a trustworthy HTTP(S) origin".to_string())?;
        let entry = self.store.entry(origin).or_default();
        if state == PermissionState::Prompt {
            entry.remove(&perm);
        } else {
            entry.insert(perm, state);
        }
        Ok(())
    }

    pub fn reset_origin(&mut self, origin: &str) -> bool {
        canonical_permission_origin(origin)
            .is_some_and(|origin| self.store.remove(&origin).is_some())
    }

    pub fn reset_all(&mut self) {
        self.store.clear();
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| format!("Permission serialization failed: {e}"))
    }

    pub fn from_json(json_str: &str) -> Result<Self, String> {
        if json_str.len() > 1024 * 1024 {
            return Err("Permission data exceeds 1 MB".to_string());
        }
        let store: Self = serde_json::from_str(json_str)
            .map_err(|e| format!("Permission parsing failed: {e}"))?;
        if store.store.len() > 4096
            || store
                .store
                .keys()
                .any(|origin| canonical_permission_origin(origin).as_deref() != Some(origin))
        {
            return Err("Permission data contains invalid or excessive origins".to_string());
        }
        Ok(store)
    }
}

fn canonical_permission_origin(input: &str) -> Option<String> {
    let parsed = url::Url::parse(input).ok()?;
    let trustworthy = parsed.scheme() == "https"
        || (parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1")));
    trustworthy
        .then(|| parsed.origin().ascii_serialization())
        .filter(|origin| origin.len() <= 4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_store_grant_deny_reset_and_json() {
        let mut ps = PermissionStore::new();
        let origin = "https://camera-app.org";

        assert_eq!(
            ps.get_permission(origin, PermissionType::Camera),
            PermissionState::Prompt
        );

        ps.set_permission(origin, PermissionType::Camera, PermissionState::Granted)
            .unwrap();
        ps.set_permission(origin, PermissionType::Geolocation, PermissionState::Denied)
            .unwrap();

        assert_eq!(
            ps.get_permission(origin, PermissionType::Camera),
            PermissionState::Granted
        );
        assert_eq!(
            ps.get_permission(origin, PermissionType::Geolocation),
            PermissionState::Denied
        );

        let json = ps.to_json().unwrap();
        let loaded = PermissionStore::from_json(&json).unwrap();
        assert_eq!(
            loaded.get_permission(origin, PermissionType::Camera),
            PermissionState::Granted
        );

        assert!(ps.reset_origin(origin));
        assert_eq!(
            ps.get_permission(origin, PermissionType::Camera),
            PermissionState::Prompt
        );
    }
}

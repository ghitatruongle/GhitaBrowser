//! Bounded clean-room Service Worker lifecycle manager for GhitaBrowser (Phase 22).
//! Implements SW registration, state transitions (installing->installed->activating->active), and fetch interception.

use crate::cache_api::{CacheEntry, CacheStorage};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceWorkerState {
    Installing,
    Installed,
    Activating,
    Active,
    Redundant,
}

#[derive(Debug, Clone)]
pub struct ServiceWorkerRegistrationOptions {
    pub scope: String,
}

#[derive(Debug, Clone)]
pub struct ServiceWorkerRegistration {
    pub script_url: String,
    pub scope: String,
    pub state: ServiceWorkerState,
    pub installing_worker: Option<String>,
    pub waiting_worker: Option<String>,
    pub active_worker: Option<String>,
}

impl ServiceWorkerRegistration {
    pub fn new(script_url: impl Into<String>, scope: impl Into<String>) -> Self {
        let script_url = script_url.into();
        let scope = scope.into();
        Self {
            script_url: script_url.clone(),
            scope,
            state: ServiceWorkerState::Installing,
            installing_worker: Some(script_url),
            waiting_worker: None,
            active_worker: None,
        }
    }

    pub fn transition_to(&mut self, next_state: ServiceWorkerState) {
        self.state = next_state;
        match next_state {
            ServiceWorkerState::Installing => {
                self.installing_worker = Some(self.script_url.clone());
            }
            ServiceWorkerState::Installed => {
                self.installing_worker = None;
                self.waiting_worker = Some(self.script_url.clone());
            }
            ServiceWorkerState::Activating | ServiceWorkerState::Active => {
                self.installing_worker = None;
                self.waiting_worker = None;
                self.active_worker = Some(self.script_url.clone());
            }
            ServiceWorkerState::Redundant => {
                self.installing_worker = None;
                self.waiting_worker = None;
                self.active_worker = None;
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct ServiceWorkerContainer {
    pub origin: String,
    pub registrations: HashMap<String, ServiceWorkerRegistration>, // key is scope
    pub cache_storage: CacheStorage,
}

impl ServiceWorkerContainer {
    pub fn new(origin: impl Into<String>) -> Self {
        let origin = origin.into();
        Self {
            cache_storage: CacheStorage::new(&origin, None),
            origin,
            registrations: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        script_url: &str,
        options: Option<ServiceWorkerRegistrationOptions>,
    ) -> Result<&ServiceWorkerRegistration, String> {
        let origin = url::Url::parse(&self.origin)
            .map_err(|_| "Invalid service worker origin".to_string())?;
        if origin.scheme() != "https" && origin.host_str() != Some("localhost") {
            return Err("Service workers require a secure origin".to_string());
        }
        let script = origin
            .join(script_url)
            .map_err(|_| "Invalid service worker script URL".to_string())?;
        if script.origin() != origin.origin() {
            return Err("Cross-origin service worker registration rejected".to_string());
        }
        let requested_scope = options.map(|o| o.scope).unwrap_or_else(|| {
            script
                .path()
                .rsplit_once('/')
                .map(|(parent, _)| format!("{parent}/"))
                .unwrap_or_else(|| "/".to_string())
        });
        let scope_url = origin
            .join(&requested_scope)
            .map_err(|_| "Invalid service worker scope".to_string())?;
        if scope_url.origin() != origin.origin() {
            return Err("Cross-origin service worker scope rejected".to_string());
        }
        let scope = scope_url.path().to_string();

        let reg = ServiceWorkerRegistration::new(script.as_str(), &scope);
        // Spec-correct: stay Installing until explicit activate(). Auto-activation
        // would let an unvetted worker intercept fetches immediately.
        // Callers must drive Installing -> Installed -> Activating -> Active
        // via `transition_to` after install checks pass.

        self.registrations.insert(scope.clone(), reg);
        Ok(self
            .registrations
            .get(&scope)
            .ok_or_else(|| "InvalidStateError: registration vanished".to_string())?)
    }

    pub fn unregister(&mut self, scope: &str) -> bool {
        if let Some(mut reg) = self.registrations.remove(scope) {
            reg.transition_to(ServiceWorkerState::Redundant);
            true
        } else {
            false
        }
    }

    pub fn get_registration(&self, client_url: &str) -> Option<&ServiceWorkerRegistration> {
        let client = url::Url::parse(client_url).ok()?;
        let origin = url::Url::parse(&self.origin).ok()?;
        if client.origin() != origin.origin() {
            return None;
        }
        let mut best_match: Option<(&String, &ServiceWorkerRegistration)> = None;
        for (scope, reg) in &self.registrations {
            // Scope match requires exact match or boundary `/` so `/app`
            // does not match `/application`. A scope ending in `/` matches
            // any path beneath it.
            let scope_match = if *scope == "/" {
                client.path().starts_with('/')
            } else if scope.ends_with('/') {
                client.path() == *scope || client.path().starts_with(scope)
            } else {
                client.path() == *scope
                    || client.path().starts_with(scope)
                        && client.path()[scope.len()..].starts_with('/')
            };
            if scope_match {
                if let Some((best_scope, _)) = best_match {
                    if scope.len() > best_scope.len() {
                        best_match = Some((scope, reg));
                    }
                } else {
                    best_match = Some((scope, reg));
                }
            }
        }
        best_match.map(|(_, reg)| reg)
    }

    pub fn intercept_fetch(&self, request_url: &str) -> Option<CacheEntry> {
        if let Some(reg) = self.get_registration(request_url) {
            if reg.state == ServiceWorkerState::Active {
                return self.cache_storage.match_all(request_url);
            }
        }
        None
    }

    /// Resolve an active registration against a browser-owned shared CacheStorage.
    pub fn intercept_fetch_with_cache_storage(
        &self,
        request_url: &str,
        cache_storage: &CacheStorage,
    ) -> Option<CacheEntry> {
        self.get_registration(request_url)
            .filter(|registration| registration.state == ServiceWorkerState::Active)
            .and_then(|_| cache_storage.match_all(request_url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sw_registration_and_lifecycle_transitions() {
        let mut container = ServiceWorkerContainer::new("https://example.com");
        let scope = {
            let reg = container
                .register(
                    "https://example.com/sw.js",
                    Some(ServiceWorkerRegistrationOptions {
                        scope: "/app/".to_string(),
                    }),
                )
                .unwrap();

            assert_eq!(reg.state, ServiceWorkerState::Installing);
            assert_eq!(
                reg.installing_worker.as_deref(),
                Some("https://example.com/sw.js")
            );
            reg.scope.clone()
        };
        // Drive explicit lifecycle: Installing -> Installed -> Activating -> Active.
        {
            let reg = container.registrations.get_mut(&scope).unwrap();
            reg.transition_to(ServiceWorkerState::Installed);
            reg.transition_to(ServiceWorkerState::Activating);
            reg.transition_to(ServiceWorkerState::Active);
        }
        let reg = container.registrations.get(&scope).unwrap();
        assert_eq!(reg.state, ServiceWorkerState::Active);
        assert_eq!(
            reg.active_worker.as_deref(),
            Some("https://example.com/sw.js")
        );

        let found = container
            .get_registration("https://example.com/app/dashboard")
            .unwrap();
        assert_eq!(found.scope, "/app/");
        // Scope boundary: /app must not match /application.
        assert!(
            container
                .get_registration("https://example.com/application")
                .is_none()
                || container
                    .get_registration("https://example.com/application")
                    .unwrap()
                    .scope
                    != "/app/"
        );
    }
}

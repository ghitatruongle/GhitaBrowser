//! Browser-owned protected-media session policy for approved local key systems.
//!
//! This is intentionally not a DRM bypass or a software CDM. It validates
//! origin, key-system approval, license/session lifetime and encrypted-sample
//! budgets before a platform CDM/media backend is allowed to receive data.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const MAX_MEDIA_KEY_SESSIONS_PER_ORIGIN: usize = 32;
pub const MAX_MEDIA_KEY_SESSIONS_TOTAL: usize = 256;
pub const MAX_APPROVED_KEY_SYSTEMS: usize = 64;
pub const MAX_LICENSE_BYTES: usize = 512 * 1024;
pub const MAX_INIT_DATA_BYTES: usize = 256 * 1024;
pub const MAX_ENCRYPTED_SAMPLE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKeySessionState {
    Pending,
    Usable,
    Expired,
    Closed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedKeySystem {
    pub name: String,
    pub persistent_state_allowed: bool,
    pub distinctive_identifier_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaKeySession {
    pub id: String,
    pub origin: String,
    pub key_system: String,
    pub state: MediaKeySessionState,
    pub persistent: bool,
    pub expires_at_ms: Option<u64>,
    license_fingerprint: Option<[u8; 32]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedSampleDescriptor {
    pub key_id: Vec<u8>,
    pub initialization_vector: Vec<u8>,
    pub encrypted_byte_count: usize,
}

#[derive(Debug, Default)]
pub struct ProtectedMediaController {
    approved_key_systems: BTreeMap<String, ApprovedKeySystem>,
    sessions: BTreeMap<String, MediaKeySession>,
    next_session_id: u64,
}

impl ProtectedMediaController {
    pub fn approve_key_system(&mut self, key_system: ApprovedKeySystem) -> Result<(), String> {
        if key_system.name.is_empty() || key_system.name.len() > 256 {
            return Err("DataError: invalid key-system name".to_string());
        }
        if self.approved_key_systems.len() >= MAX_APPROVED_KEY_SYSTEMS
            && !self.approved_key_systems.contains_key(&key_system.name)
        {
            return Err("QuotaExceededError: approved key-system budget exceeded".to_string());
        }
        self.approved_key_systems
            .insert(key_system.name.clone(), key_system);
        Ok(())
    }

    pub fn create_session(
        &mut self,
        origin: &str,
        key_system: &str,
        persistent: bool,
        init_data: &[u8],
    ) -> Result<MediaKeySession, String> {
        let origin = canonical_origin(origin)?;
        if init_data.is_empty() || init_data.len() > MAX_INIT_DATA_BYTES {
            return Err("DataError: encrypted-media initialization data is invalid".to_string());
        }
        let approved = self
            .approved_key_systems
            .get(key_system)
            .ok_or_else(|| "NotSupportedError: key system is not approved".to_string())?;
        if persistent && !approved.persistent_state_allowed {
            return Err("NotAllowedError: persistent media sessions are not approved".to_string());
        }
        if self.sessions.len() >= MAX_MEDIA_KEY_SESSIONS_TOTAL {
            self.sessions
                .retain(|_, session| session.state != MediaKeySessionState::Closed);
        }
        if self.sessions.len() >= MAX_MEDIA_KEY_SESSIONS_TOTAL {
            return Err("QuotaExceededError: global media-key session budget exceeded".to_string());
        }
        if self
            .sessions
            .values()
            .filter(|session| {
                session.origin == origin && session.state != MediaKeySessionState::Closed
            })
            .count()
            >= MAX_MEDIA_KEY_SESSIONS_PER_ORIGIN
        {
            return Err("QuotaExceededError: media-key session budget exceeded".to_string());
        }
        let id = self
            .next_session_id
            .checked_add(1)
            .ok_or_else(|| "media-key session id overflow".to_string())?;
        self.next_session_id = id;
        let session = MediaKeySession {
            id: format!("ghita-eme-{id}"),
            origin,
            key_system: key_system.to_string(),
            state: MediaKeySessionState::Pending,
            persistent,
            expires_at_ms: None,
            license_fingerprint: None,
        };
        self.sessions.insert(session.id.clone(), session.clone());
        Ok(session)
    }

    pub fn update_license(
        &mut self,
        session_id: &str,
        license: &[u8],
        expires_at_ms: Option<u64>,
    ) -> Result<(), String> {
        if license.is_empty() || license.len() > MAX_LICENSE_BYTES {
            return Err("DataError: media license is invalid".to_string());
        }
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| "InvalidStateError: media-key session is detached".to_string())?;
        if matches!(
            session.state,
            MediaKeySessionState::Closed | MediaKeySessionState::Expired
        ) {
            return Err("InvalidStateError: media-key session cannot accept a license".to_string());
        }
        let mut fingerprint = [0_u8; 32];
        fingerprint.copy_from_slice(&Sha256::digest(license));
        session.license_fingerprint = Some(fingerprint);
        session.expires_at_ms = expires_at_ms;
        session.state = MediaKeySessionState::Usable;
        Ok(())
    }

    pub fn authorize_encrypted_sample(
        &mut self,
        session_id: &str,
        sample: &EncryptedSampleDescriptor,
        now_ms: u64,
    ) -> Result<(), String> {
        if sample.key_id.is_empty()
            || sample.key_id.len() > 256
            || sample.initialization_vector.is_empty()
            || sample.initialization_vector.len() > 64
            || sample.encrypted_byte_count == 0
            || sample.encrypted_byte_count > MAX_ENCRYPTED_SAMPLE_BYTES
        {
            return Err("DataError: encrypted sample descriptor is invalid".to_string());
        }
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| "InvalidStateError: media-key session is detached".to_string())?;
        if session
            .expires_at_ms
            .is_some_and(|expires| now_ms >= expires)
        {
            session.state = MediaKeySessionState::Expired;
        }
        if session.state != MediaKeySessionState::Usable || session.license_fingerprint.is_none() {
            return Err(
                "NotAllowedError: encrypted sample has no usable media-key session".to_string(),
            );
        }
        Ok(())
    }

    pub fn close_session(&mut self, session_id: &str) -> bool {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.state = MediaKeySessionState::Closed;
            session.license_fingerprint = None;
            true
        } else {
            false
        }
    }

    pub fn clear_origin(&mut self, origin: &str) -> Result<usize, String> {
        let origin = canonical_origin(origin)?;
        let ids: Vec<String> = self
            .sessions
            .iter()
            .filter_map(|(id, session)| (session.origin == origin).then_some(id.clone()))
            .collect();
        for id in &ids {
            self.close_session(id);
            self.sessions.remove(id);
        }
        Ok(ids.len())
    }
}

fn canonical_origin(value: &str) -> Result<String, String> {
    let url =
        url::Url::parse(value).map_err(|_| "SecurityError: invalid media origin".to_string())?;
    if url.scheme() != "https" && url.host_str() != Some("localhost") {
        return Err("SecurityError: encrypted media requires a secure origin".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

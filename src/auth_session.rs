//! Origin-partitioned authenticated-session boundary for controlled local apps.
//!
//! Tokens remain private to this module. Callers receive only opaque session
//! identifiers and a redacted audit view, preventing credentials from being
//! accidentally written to browser reports, fixtures or diagnostics.

use std::collections::BTreeMap;

pub const MAX_SESSIONS_PER_ORIGIN: usize = 64;
pub const MAX_SESSIONS_TOTAL: usize = 1_024;
pub const MAX_TOKEN_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSessionState {
    Active,
    Expired,
    LoggedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSessionAudit {
    pub id: String,
    pub origin: String,
    pub state: AuthSessionState,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct AuthSession {
    id: String,
    origin: String,
    token: Vec<u8>,
    state: AuthSessionState,
    expires_at_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub struct AuthSessionStore {
    next_id: u64,
    sessions: BTreeMap<String, AuthSession>,
}

impl AuthSessionStore {
    pub fn create(
        &mut self,
        origin: &str,
        token: Vec<u8>,
        expires_at_ms: Option<u64>,
    ) -> Result<AuthSessionAudit, String> {
        let origin = canonical_origin(origin)?;
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return Err("DataError: session token is invalid".to_string());
        }
        if self.sessions.len() >= MAX_SESSIONS_TOTAL {
            self.sessions
                .retain(|_, session| session.state == AuthSessionState::Active);
        }
        if self.sessions.len() >= MAX_SESSIONS_TOTAL {
            return Err(
                "QuotaExceededError: global authenticated session budget exceeded".to_string(),
            );
        }
        if self
            .sessions
            .values()
            .filter(|session| session.origin == origin && session.state == AuthSessionState::Active)
            .count()
            >= MAX_SESSIONS_PER_ORIGIN
        {
            return Err("QuotaExceededError: authenticated session budget exceeded".to_string());
        }
        let id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "session id overflow".to_string())?;
        self.next_id = id;
        let session = AuthSession {
            id: format!("ghita-session-{id}"),
            origin,
            token,
            state: AuthSessionState::Active,
            expires_at_ms,
        };
        let audit = audit(&session);
        self.sessions.insert(session.id.clone(), session);
        Ok(audit)
    }

    /// Returns a clone of the bearer token only to the browser-owned network
    /// policy layer after checking origin and expiry. No UI/report caller
    /// receives a token through this API.
    // Reserved for the browser-owned network adapter; keeping this crate-only
    // prevents page/report code from obtaining bearer credentials.
    #[allow(dead_code)]
    pub(crate) fn token_for_origin(
        &mut self,
        id: &str,
        origin: &str,
        now_ms: u64,
    ) -> Result<Vec<u8>, String> {
        let origin = canonical_origin(origin)?;
        let session = self
            .sessions
            .get_mut(id)
            .ok_or_else(|| "NotFoundError: authenticated session does not exist".to_string())?;
        if session.origin != origin {
            return Err("SecurityError: session origin mismatch".to_string());
        }
        if session
            .expires_at_ms
            .is_some_and(|expires| now_ms >= expires)
        {
            session.token.fill(0);
            session.token.clear();
            session.state = AuthSessionState::Expired;
        }
        if session.state != AuthSessionState::Active {
            return Err("NotAllowedError: authenticated session is inactive".to_string());
        }
        Ok(session.token.clone())
    }

    pub fn logout(&mut self, id: &str) -> bool {
        if let Some(session) = self.sessions.get_mut(id) {
            session.token.fill(0);
            session.token.clear();
            session.state = AuthSessionState::LoggedOut;
            true
        } else {
            false
        }
    }

    pub fn audit(&mut self, id: &str, now_ms: u64) -> Option<AuthSessionAudit> {
        let session = self.sessions.get_mut(id)?;
        if session
            .expires_at_ms
            .is_some_and(|expires| now_ms >= expires)
            && session.state == AuthSessionState::Active
        {
            session.token.fill(0);
            session.token.clear();
            session.state = AuthSessionState::Expired;
        }
        Some(audit(session))
    }

    pub fn clear_origin(&mut self, origin: &str) -> Result<usize, String> {
        let origin = canonical_origin(origin)?;
        let ids: Vec<String> = self
            .sessions
            .iter()
            .filter_map(|(id, session)| (session.origin == origin).then_some(id.clone()))
            .collect();
        for id in &ids {
            self.logout(id);
            self.sessions.remove(id);
        }
        Ok(ids.len())
    }
}

fn audit(session: &AuthSession) -> AuthSessionAudit {
    AuthSessionAudit {
        id: session.id.clone(),
        origin: session.origin.clone(),
        state: session.state,
        expires_at_ms: session.expires_at_ms,
    }
}

fn canonical_origin(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value)
        .map_err(|_| "SecurityError: invalid authenticated origin".to_string())?;
    if url.scheme() != "https" && url.host_str() != Some("localhost") {
        return Err("SecurityError: authenticated sessions require a secure origin".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

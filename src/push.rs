//! Origin-partitioned Push API model for deterministic local application tests.
//!
//! Network delivery is intentionally separated from subscription state. A
//! provider adapter must validate transport authenticity before handing a
//! [`PushMessage`] to this browser-owned module.

use std::collections::{BTreeMap, VecDeque};

pub const MAX_SUBSCRIPTIONS_PER_ORIGIN: usize = 32;
pub const MAX_SUBSCRIPTIONS_TOTAL: usize = 256;
pub const MAX_PUSH_PAYLOAD_BYTES: usize = 256 * 1024;
pub const MAX_PUSH_ENDPOINT_BYTES: usize = 4 * 1024;
pub const MAX_SEEN_NONCES_PER_SUBSCRIPTION: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscription {
    pub id: u64,
    pub origin: String,
    pub worker_id: u64,
    pub endpoint: String,
    pub application_server_key: Vec<u8>,
    pub expires_at_ms: Option<u64>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushMessage {
    pub subscription_id: u64,
    pub payload: Vec<u8>,
    pub issued_at_ms: u64,
    pub nonce: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRecord {
    pub origin: String,
    pub title: String,
    pub body: String,
    pub worker_id: u64,
}

#[derive(Debug, Default)]
pub struct PushManager {
    next_id: u64,
    subscriptions: BTreeMap<u64, PushSubscription>,
    seen_nonces: BTreeMap<u64, VecDeque<u64>>,
}

impl PushManager {
    pub fn subscribe(
        &mut self,
        origin: &str,
        worker_id: u64,
        endpoint: impl Into<String>,
        application_server_key: Vec<u8>,
        expires_at_ms: Option<u64>,
    ) -> Result<PushSubscription, String> {
        let origin = canonical_origin(origin)?;
        if application_server_key.is_empty() || application_server_key.len() > 512 {
            return Err("DataError: invalid application server key".to_string());
        }
        if self.subscriptions.len() >= MAX_SUBSCRIPTIONS_TOTAL {
            return Err("QuotaExceededError: global push subscription budget exceeded".to_string());
        }
        if self
            .subscriptions
            .values()
            .filter(|subscription| subscription.origin == origin && subscription.active)
            .count()
            >= MAX_SUBSCRIPTIONS_PER_ORIGIN
        {
            return Err("QuotaExceededError: push subscription budget exceeded".to_string());
        }
        let endpoint = endpoint.into();
        if endpoint.len() > MAX_PUSH_ENDPOINT_BYTES {
            return Err("DataError: push endpoint exceeds budget".to_string());
        }
        let parsed = url::Url::parse(&endpoint)
            .map_err(|_| "SyntaxError: invalid push endpoint".to_string())?;
        if parsed.scheme() != "https" {
            return Err("SecurityError: push endpoint must use HTTPS".to_string());
        }
        let id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "push subscription id overflow".to_string())?;
        self.next_id = id;
        let subscription = PushSubscription {
            id,
            origin,
            worker_id,
            endpoint,
            application_server_key,
            expires_at_ms,
            active: true,
        };
        self.subscriptions.insert(id, subscription.clone());
        Ok(subscription)
    }

    pub fn get(&self, id: u64) -> Option<&PushSubscription> {
        self.subscriptions
            .get(&id)
            .filter(|subscription| subscription.active)
    }

    pub fn unsubscribe(&mut self, id: u64) -> bool {
        self.seen_nonces.remove(&id);
        self.subscriptions.remove(&id).is_some()
    }

    pub fn deliver(&mut self, message: PushMessage, now_ms: u64) -> Result<(u64, Vec<u8>), String> {
        if message.payload.len() > MAX_PUSH_PAYLOAD_BYTES {
            return Err("QuotaExceededError: push payload exceeds budget".to_string());
        }
        let subscription = self
            .get(message.subscription_id)
            .cloned()
            .ok_or_else(|| "NotFoundError: push subscription is inactive".to_string())?;
        if subscription
            .expires_at_ms
            .is_some_and(|expires| now_ms >= expires)
        {
            self.unsubscribe(message.subscription_id);
            return Err("NotAllowedError: push subscription expired".to_string());
        }
        let nonces = self.seen_nonces.entry(message.subscription_id).or_default();
        if nonces.contains(&message.nonce) {
            return Err("SecurityError: replayed push message rejected".to_string());
        }
        if nonces.len() >= MAX_SEEN_NONCES_PER_SUBSCRIPTION {
            nonces.pop_front();
        }
        nonces.push_back(message.nonce);
        Ok((subscription.worker_id, message.payload))
    }

    pub fn clear_origin(&mut self, origin: &str) -> Result<usize, String> {
        let origin = canonical_origin(origin)?;
        let ids: Vec<u64> = self
            .subscriptions
            .iter()
            .filter_map(|(id, subscription)| (subscription.origin == origin).then_some(*id))
            .collect();
        for id in &ids {
            self.subscriptions.remove(id);
            self.seen_nonces.remove(id);
        }
        Ok(ids.len())
    }
}

fn canonical_origin(value: &str) -> Result<String, String> {
    let url =
        url::Url::parse(value).map_err(|_| "SecurityError: invalid push origin".to_string())?;
    if url.scheme() != "https" && url.host_str() != Some("localhost") {
        return Err("SecurityError: Push requires a secure origin".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

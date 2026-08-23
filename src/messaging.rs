//! Bounded clean-room BroadcastChannel and structuredClone implementation for GhitaBrowser (Phase 22).
//! Implements origin-partitioned cross-context pub/sub messaging and deep object cloning.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

static NEXT_CHANNEL_ID: AtomicU64 = AtomicU64::new(1);

fn global_bus() -> &'static Mutex<BroadcastChannelBus> {
    static BUS: OnceLock<Mutex<BroadcastChannelBus>> = OnceLock::new();
    BUS.get_or_init(|| Mutex::new(BroadcastChannelBus::new()))
}

/// Lock the global bus, recovering from a poisoned lock.
///
/// A panic while a thread held the bus lock leaves it poisoned; every later
/// `.expect("bus lock")` would then panic in turn and abort the whole
/// browser process in release builds. The bus has no invariant that a panic
/// can violate beyond losing one in-flight message, so the poison is safely
/// cleared and the (still consistent) state is reused.
fn lock_bus() -> std::sync::MutexGuard<'static, BroadcastChannelBus> {
    global_bus()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
pub struct BroadcastChannelBus {
    // origin -> channel_name -> list of (channel_id)
    channels: HashMap<String, HashMap<String, Vec<u64>>>,
    // channel_id -> receive_queue
    queues: HashMap<u64, Vec<String>>,
}

impl BroadcastChannelBus {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
            queues: HashMap::new(),
        }
    }

    pub fn register(&mut self, origin: &str, name: &str) -> u64 {
        let id = NEXT_CHANNEL_ID.fetch_add(1, Ordering::SeqCst);
        self.channels
            .entry(origin.to_string())
            .or_default()
            .entry(name.to_string())
            .or_default()
            .push(id);
        self.queues.insert(id, Vec::new());
        id
    }

    pub fn unregister(&mut self, origin: &str, name: &str, id: u64) {
        if let Some(origin_map) = self.channels.get_mut(origin) {
            if let Some(ids) = origin_map.get_mut(name) {
                ids.retain(|&x| x != id);
            }
        }
        self.queues.remove(&id);
    }

    pub fn post_message(&mut self, origin: &str, name: &str, sender_id: u64, message: String) {
        if let Some(origin_map) = self.channels.get(origin) {
            if let Some(ids) = origin_map.get(name) {
                for &id in ids {
                    if id != sender_id {
                        if let Some(q) = self.queues.get_mut(&id) {
                            if q.len() < 100 {
                                // max 100 queued messages per channel
                                q.push(message.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn poll_message(&mut self, id: u64) -> Option<String> {
        if let Some(q) = self.queues.get_mut(&id) {
            if !q.is_empty() {
                return Some(q.remove(0));
            }
        }
        None
    }
}

#[derive(Debug)]
pub struct BroadcastChannel {
    pub id: u64,
    pub origin: String,
    pub name: String,
    pub closed: bool,
}

impl BroadcastChannel {
    pub fn new(origin: impl Into<String>, name: impl Into<String>) -> Self {
        let origin = origin.into();
        let name = name.into();
        let id = lock_bus().register(&origin, &name);
        Self {
            id,
            origin,
            name,
            closed: false,
        }
    }

    pub fn post_message(&mut self, message: String) -> Result<(), String> {
        if self.closed {
            return Err("BroadcastChannel is closed".to_string());
        }
        if message.len() > 1_048_576 {
            return Err("BroadcastChannel message exceeds 1 MB limit".to_string());
        }
        lock_bus().post_message(&self.origin, &self.name, self.id, message);
        Ok(())
    }

    pub fn poll_message(&mut self) -> Option<String> {
        if self.closed {
            return None;
        }
        lock_bus().poll_message(self.id)
    }

    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            lock_bus().unregister(&self.origin, &self.name, self.id);
        }
    }
}

impl Drop for BroadcastChannel {
    fn drop(&mut self) {
        self.close();
    }
}

/// WHATWG HTML Structured Clone Algorithm implementation
pub fn structured_clone(
    value: &serde_json::Value,
    depth: usize,
) -> Result<serde_json::Value, String> {
    if depth > 64 {
        return Err("DataCloneError: Exceeded maximum cloning depth of 64".to_string());
    }

    match value {
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => Ok(value.clone()),
        serde_json::Value::Array(arr) => {
            let mut cloned_arr = Vec::with_capacity(arr.len());
            for item in arr {
                cloned_arr.push(structured_clone(item, depth + 1)?);
            }
            Ok(serde_json::Value::Array(cloned_arr))
        }
        serde_json::Value::Object(map) => {
            let mut cloned_map = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                cloned_map.insert(k.clone(), structured_clone(v, depth + 1)?);
            }
            Ok(serde_json::Value::Object(cloned_map))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_channel_delivers_messages_across_same_origin() {
        let mut ch1 = BroadcastChannel::new("https://example.com", "sync");
        let mut ch2 = BroadcastChannel::new("https://example.com", "sync");
        let mut ch3 = BroadcastChannel::new("https://other.com", "sync");

        ch1.post_message("{\"type\":\"update\"}".to_string())
            .unwrap();

        // Same origin channel ch2 receives message
        assert_eq!(ch2.poll_message().as_deref(), Some("{\"type\":\"update\"}"));
        // Sender ch1 does not receive its own message
        assert_eq!(ch1.poll_message(), None);
        // Different origin channel ch3 receives nothing (isolated)
        assert_eq!(ch3.poll_message(), None);
    }

    #[test]
    fn structured_clone_deep_clones_values() {
        let original: serde_json::Value =
            serde_json::from_str("{\"a\":[1,2,{\"b\":true}]}").unwrap();
        let cloned = structured_clone(&original, 0).unwrap();
        assert_eq!(original, cloned);
    }
}

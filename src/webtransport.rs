//! Bounded WebTransport session model.
//!
//! The browser policy layer validates origin and certificate decisions before
//! creating a session. This module owns only page-visible streams, datagrams,
//! backpressure and deterministic teardown; it never opens a socket itself.

use std::collections::{BTreeMap, VecDeque};

pub const MAX_SESSIONS_PER_ORIGIN: usize = 16;
pub const MAX_SESSIONS_TOTAL: usize = 128;
pub const MAX_STREAMS_PER_SESSION: usize = 128;
pub const MAX_DATAGRAM_BYTES: usize = 64 * 1024;
pub const MAX_STREAM_CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_BUFFERED_BYTES_PER_SESSION: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebTransportState {
    Connecting,
    Connected,
    Closing,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebTransportStreamDirection {
    Bidirectional,
    Unidirectional,
}

#[derive(Debug, Clone)]
pub struct WebTransportStream {
    pub id: u64,
    pub direction: WebTransportStreamDirection,
    pub readable: bool,
    pub writable: bool,
    inbound: VecDeque<Vec<u8>>,
    outbound: VecDeque<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct WebTransportSession {
    pub id: u64,
    pub origin: String,
    pub url: String,
    pub state: WebTransportState,
    pub close_code: Option<u32>,
    pub close_reason: Option<String>,
    streams: BTreeMap<u64, WebTransportStream>,
    next_stream_id: u64,
    datagrams_inbound: VecDeque<Vec<u8>>,
    datagrams_outbound: VecDeque<Vec<u8>>,
    buffered_bytes: usize,
}

#[derive(Debug, Default)]
pub struct WebTransportRegistry {
    next_session_id: u64,
    sessions: BTreeMap<u64, WebTransportSession>,
}

impl WebTransportRegistry {
    pub fn connect(&mut self, origin: &str, url: &str) -> Result<u64, String> {
        let origin = canonical_origin(origin)?;
        let endpoint = url::Url::parse(url)
            .map_err(|_| "SyntaxError: invalid WebTransport URL".to_string())?;
        if endpoint.scheme() != "https" || endpoint.origin().ascii_serialization() != origin {
            return Err(
                "SecurityError: WebTransport requires a same-origin HTTPS endpoint".to_string(),
            );
        }
        if self.sessions.len() >= MAX_SESSIONS_TOTAL {
            self.sessions
                .retain(|_, session| session.state != WebTransportState::Closed);
        }
        if self.sessions.len() >= MAX_SESSIONS_TOTAL {
            return Err(
                "QuotaExceededError: global WebTransport session budget exceeded".to_string(),
            );
        }
        if self
            .sessions
            .values()
            .filter(|session| {
                session.origin == origin && session.state != WebTransportState::Closed
            })
            .count()
            >= MAX_SESSIONS_PER_ORIGIN
        {
            return Err("QuotaExceededError: WebTransport session budget exceeded".to_string());
        }
        let id = self
            .next_session_id
            .checked_add(1)
            .ok_or_else(|| "WebTransport session id overflow".to_string())?;
        self.next_session_id = id;
        self.sessions.insert(
            id,
            WebTransportSession {
                id,
                origin,
                url: endpoint.to_string(),
                state: WebTransportState::Connected,
                close_code: None,
                close_reason: None,
                streams: BTreeMap::new(),
                next_stream_id: 1,
                datagrams_inbound: VecDeque::new(),
                datagrams_outbound: VecDeque::new(),
                buffered_bytes: 0,
            },
        );
        Ok(id)
    }

    pub fn session(&self, id: u64) -> Option<&WebTransportSession> {
        self.sessions.get(&id)
    }

    pub fn create_stream(
        &mut self,
        session_id: u64,
        direction: WebTransportStreamDirection,
    ) -> Result<u64, String> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| "InvalidStateError: WebTransport session is detached".to_string())?;
        if session.state != WebTransportState::Connected {
            return Err("InvalidStateError: WebTransport session is not connected".to_string());
        }
        if session.streams.len() >= MAX_STREAMS_PER_SESSION {
            return Err("QuotaExceededError: WebTransport stream budget exceeded".to_string());
        }
        let id = session.next_stream_id;
        session.next_stream_id = session.next_stream_id.saturating_add(1);
        session.streams.insert(
            id,
            WebTransportStream {
                id,
                direction,
                readable: direction == WebTransportStreamDirection::Bidirectional,
                writable: true,
                inbound: VecDeque::new(),
                outbound: VecDeque::new(),
            },
        );
        Ok(id)
    }

    pub fn send_datagram(&mut self, session_id: u64, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() > MAX_DATAGRAM_BYTES {
            return Err("QuotaExceededError: WebTransport datagram exceeds budget".to_string());
        }
        let session = self.connected_session_mut(session_id)?;
        Self::charge(session, bytes.len())?;
        session.datagrams_outbound.push_back(bytes);
        Ok(())
    }

    pub fn send_stream_data(
        &mut self,
        session_id: u64,
        stream_id: u64,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        if bytes.len() > MAX_STREAM_CHUNK_BYTES {
            return Err("QuotaExceededError: WebTransport stream chunk exceeds budget".to_string());
        }
        let session = self.connected_session_mut(session_id)?;
        let writable = session
            .streams
            .get(&stream_id)
            .ok_or_else(|| "InvalidStateError: WebTransport stream is detached".to_string())?
            .writable;
        if !writable {
            return Err("InvalidStateError: WebTransport stream is not writable".to_string());
        }
        Self::charge(session, bytes.len())?;
        session
            .streams
            .get_mut(&stream_id)
            .expect("stream was validated")
            .outbound
            .push_back(bytes);
        Ok(())
    }

    pub fn receive_stream_data(
        &mut self,
        session_id: u64,
        stream_id: u64,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        if bytes.len() > MAX_STREAM_CHUNK_BYTES {
            return Err("QuotaExceededError: WebTransport stream chunk exceeds budget".to_string());
        }
        let session = self.connected_session_mut(session_id)?;
        let readable = session
            .streams
            .get(&stream_id)
            .ok_or_else(|| "InvalidStateError: WebTransport stream is detached".to_string())?
            .readable;
        if !readable {
            return Err("InvalidStateError: WebTransport stream is not readable".to_string());
        }
        Self::charge(session, bytes.len())?;
        session
            .streams
            .get_mut(&stream_id)
            .expect("stream was validated")
            .inbound
            .push_back(bytes);
        Ok(())
    }

    pub fn read_stream_data(
        &mut self,
        session_id: u64,
        stream_id: u64,
    ) -> Result<Option<Vec<u8>>, String> {
        let session = self.connected_session_mut(session_id)?;
        let value = session
            .streams
            .get_mut(&stream_id)
            .ok_or_else(|| "InvalidStateError: WebTransport stream is detached".to_string())?
            .inbound
            .pop_front();
        if let Some(bytes) = &value {
            session.buffered_bytes = session.buffered_bytes.saturating_sub(bytes.len());
        }
        Ok(value)
    }

    pub fn take_outbound_stream_data(
        &mut self,
        session_id: u64,
        stream_id: u64,
    ) -> Result<Option<Vec<u8>>, String> {
        let session = self.connected_session_mut(session_id)?;
        let value = session
            .streams
            .get_mut(&stream_id)
            .ok_or_else(|| "InvalidStateError: WebTransport stream is detached".to_string())?
            .outbound
            .pop_front();
        if let Some(bytes) = &value {
            session.buffered_bytes = session.buffered_bytes.saturating_sub(bytes.len());
        }
        Ok(value)
    }

    pub fn receive_datagram(&mut self, session_id: u64, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() > MAX_DATAGRAM_BYTES {
            return Err("QuotaExceededError: WebTransport datagram exceeds budget".to_string());
        }
        let session = self.connected_session_mut(session_id)?;
        Self::charge(session, bytes.len())?;
        session.datagrams_inbound.push_back(bytes);
        Ok(())
    }

    pub fn read_datagram(&mut self, session_id: u64) -> Result<Option<Vec<u8>>, String> {
        let session = self.connected_session_mut(session_id)?;
        let value = session.datagrams_inbound.pop_front();
        if let Some(bytes) = &value {
            session.buffered_bytes = session.buffered_bytes.saturating_sub(bytes.len());
        }
        Ok(value)
    }

    /// Drain one browser-to-network datagram and release its backpressure
    /// charge. A platform transport adapter should call this after accepting
    /// ownership of the datagram.
    pub fn take_outbound_datagram(&mut self, session_id: u64) -> Result<Option<Vec<u8>>, String> {
        let session = self.connected_session_mut(session_id)?;
        let value = session.datagrams_outbound.pop_front();
        if let Some(bytes) = &value {
            session.buffered_bytes = session.buffered_bytes.saturating_sub(bytes.len());
        }
        Ok(value)
    }

    pub fn close(
        &mut self,
        session_id: u64,
        code: u32,
        reason: impl Into<String>,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| "InvalidStateError: WebTransport session is detached".to_string())?;
        let reason = reason.into();
        if reason.len() > 1_024 {
            return Err("TypeError: WebTransport close reason exceeds budget".to_string());
        }
        session.state = WebTransportState::Closed;
        session.close_code = Some(code);
        session.close_reason = Some(reason);
        session.streams.clear();
        session.datagrams_inbound.clear();
        session.datagrams_outbound.clear();
        session.buffered_bytes = 0;
        Ok(())
    }

    fn connected_session_mut(&mut self, id: u64) -> Result<&mut WebTransportSession, String> {
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| "InvalidStateError: WebTransport session is detached".to_string())?;
        if session.state != WebTransportState::Connected {
            return Err("InvalidStateError: WebTransport session is not connected".to_string());
        }
        Ok(session)
    }

    fn charge(session: &mut WebTransportSession, bytes: usize) -> Result<(), String> {
        let projected = session.buffered_bytes.saturating_add(bytes);
        if projected > MAX_BUFFERED_BYTES_PER_SESSION {
            return Err(
                "QuotaExceededError: WebTransport buffered byte budget exceeded".to_string(),
            );
        }
        session.buffered_bytes = projected;
        Ok(())
    }
}

fn canonical_origin(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value)
        .map_err(|_| "SecurityError: invalid WebTransport origin".to_string())?;
    if url.scheme() != "https" && url.host_str() != Some("localhost") {
        return Err("SecurityError: WebTransport requires a secure origin".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

//! Bounded real WebSocket and EventSource transports for GhitaBrowser.
//!
//! RFC 6455 framing/TLS is delegated to the audited `tungstenite` protocol
//! adapter. Browser policy, JavaScript bindings, queues, limits, lifecycle,
//! cancellation and origin enforcement remain browser-owned.

use std::io::Read;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::Duration;

const MAX_REALTIME_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_REALTIME_QUEUE: usize = 100;
const MAX_EVENTS_PER_PUMP: usize = 128;
const MAX_SSE_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum WebSocketReadyState {
    Connecting = 0,
    Open = 1,
    Closing = 2,
    Closed = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMessage {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Debug)]
enum WebSocketCommand {
    Send(WebSocketMessage),
    Close(Option<u16>, Option<String>),
}

#[derive(Debug)]
enum WebSocketTransportEvent {
    Open(String),
    Message(WebSocketMessage),
    Sent(usize),
    Closed,
    Error(String),
}

#[derive(Debug)]
pub struct WebSocketClient {
    pub url: String,
    pub protocol: String,
    pub ready_state: WebSocketReadyState,
    pub buffered_amount: usize,
    pub receive_queue: Vec<WebSocketMessage>,
    pub sent_queue: Vec<WebSocketMessage>,
    pub last_error: Option<String>,
    commands: SyncSender<WebSocketCommand>,
    events: Receiver<WebSocketTransportEvent>,
}

impl WebSocketClient {
    /// Start a real connection on a background transport thread. Construction
    /// never blocks the page/script thread; readiness is observed by polling
    /// `ready_state` or calling `poll_incoming`/`pump_transport`.
    pub fn new(url: impl Into<String>, protocol: Option<&str>) -> Result<Self, String> {
        Self::with_origin(url, protocol, None)
    }

    /// Like [`WebSocketClient::new`] but also sends the page origin on the
    /// handshake so Origin-based server protections work (RFC 6455 SHOULD).
    pub fn with_origin(
        url: impl Into<String>,
        protocol: Option<&str>,
        origin: Option<String>,
    ) -> Result<Self, String> {
        let url = url.into();
        let parsed = url::Url::parse(&url).map_err(|_| "Invalid WebSocket URL".to_string())?;
        if !matches!(parsed.scheme(), "ws" | "wss") || parsed.host_str().is_none() {
            return Err("WebSocket URL must use ws:// or wss://".to_string());
        }
        let protocol = protocol.unwrap_or("").trim().to_string();
        if protocol.len() > 256
            || protocol
                .bytes()
                .any(|byte| !(0x21..=0x7e).contains(&byte) || matches!(byte, b',' | b' '))
        {
            return Err("Invalid WebSocket subprotocol".to_string());
        }

        let (command_tx, command_rx) = mpsc::sync_channel(MAX_REALTIME_QUEUE);
        let (event_tx, event_rx) = mpsc::sync_channel(MAX_REALTIME_QUEUE);
        let worker_url = url.clone();
        let worker_protocol = protocol.clone();
        std::thread::Builder::new()
            .name("ghita-websocket".to_string())
            .spawn(move || {
                websocket_transport(worker_url, worker_protocol, origin, command_rx, event_tx)
            })
            .map_err(|error| format!("Cannot start WebSocket transport: {error}"))?;

        Ok(Self {
            url,
            protocol,
            ready_state: WebSocketReadyState::Connecting,
            buffered_amount: 0,
            receive_queue: Vec::new(),
            sent_queue: Vec::new(),
            last_error: None,
            commands: command_tx,
            events: event_rx,
        })
    }

    pub fn pump_transport(&mut self) {
        for _ in 0..MAX_EVENTS_PER_PUMP {
            let event = match self.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.ready_state != WebSocketReadyState::Closed {
                        self.ready_state = WebSocketReadyState::Closed;
                    }
                    break;
                }
            };
            match event {
                WebSocketTransportEvent::Open(protocol) => {
                    self.protocol = protocol;
                    self.ready_state = WebSocketReadyState::Open;
                }
                WebSocketTransportEvent::Message(message) => {
                    if self.receive_queue.len() < MAX_REALTIME_QUEUE {
                        self.receive_queue.push(message);
                    }
                }
                WebSocketTransportEvent::Sent(bytes) => {
                    self.buffered_amount = self.buffered_amount.saturating_sub(bytes);
                    // Retire the corresponding queued message; otherwise the
                    // queue grows forever and every send fails once
                    // MAX_REALTIME_QUEUE is reached on a healthy socket.
                    if !self.sent_queue.is_empty() {
                        self.sent_queue.remove(0);
                    }
                }
                WebSocketTransportEvent::Closed => {
                    self.ready_state = WebSocketReadyState::Closed;
                }
                WebSocketTransportEvent::Error(error) => {
                    self.last_error = Some(error);
                    self.ready_state = WebSocketReadyState::Closed;
                }
            }
        }
    }

    pub fn send(&mut self, message: WebSocketMessage) -> Result<(), String> {
        self.pump_transport();
        if self.ready_state != WebSocketReadyState::Open {
            return Err("WebSocket is not in OPEN state".to_string());
        }
        let message_bytes = message_len(&message);
        if message_bytes > MAX_REALTIME_MESSAGE_BYTES {
            return Err("WebSocket frame exceeds 1 MB limit".to_string());
        }
        if self.sent_queue.len() >= MAX_REALTIME_QUEUE {
            return Err("WebSocket send queue is full".to_string());
        }
        self.commands
            .try_send(WebSocketCommand::Send(message.clone()))
            .map_err(|error| match error {
                TrySendError::Full(_) => "WebSocket transport queue is full".to_string(),
                TrySendError::Disconnected(_) => "WebSocket transport is closed".to_string(),
            })?;
        self.buffered_amount = self.buffered_amount.saturating_add(message_bytes);
        self.sent_queue.push(message);
        Ok(())
    }

    pub fn close(&mut self, code: Option<u16>, reason: Option<String>) -> Result<(), String> {
        self.pump_transport();
        if self.ready_state == WebSocketReadyState::Closed {
            return Ok(());
        }
        if let Some(code) = code {
            if !(1000..=4999).contains(&code) || matches!(code, 1004 | 1005 | 1006 | 1015) {
                return Err("Invalid WebSocket close code".to_string());
            }
        }
        if reason.as_ref().is_some_and(|value| value.len() > 123) {
            return Err("WebSocket close reason exceeds 123 bytes".to_string());
        }
        self.ready_state = WebSocketReadyState::Closing;
        match self
            .commands
            .try_send(WebSocketCommand::Close(code, reason))
        {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("WebSocket transport queue is full".to_string()),
            Err(TrySendError::Disconnected(_)) => {
                self.ready_state = WebSocketReadyState::Closed;
                Ok(())
            }
        }
    }

    pub fn poll_incoming(&mut self) -> Option<WebSocketMessage> {
        self.pump_transport();
        if self.receive_queue.is_empty() {
            None
        } else {
            Some(self.receive_queue.remove(0))
        }
    }
}

fn websocket_transport(
    url: String,
    protocol: String,
    origin: Option<String>,
    commands: Receiver<WebSocketCommand>,
    events: SyncSender<WebSocketTransportEvent>,
) {
    use tungstenite::client::{connect_with_config, ClientRequestBuilder};
    use tungstenite::protocol::{CloseFrame, WebSocketConfig};
    use tungstenite::Message;

    let uri = match url.parse::<tungstenite::http::Uri>() {
        Ok(uri) => uri,
        Err(error) => {
            let _ = events.try_send(WebSocketTransportEvent::Error(error.to_string()));
            return;
        }
    };
    let mut request = ClientRequestBuilder::new(uri);
    if !protocol.is_empty() {
        request = request.with_sub_protocol(&protocol);
    }
    // Servers relying on Origin-based cross-site protections need the page
    // origin on the handshake (RFC 6455 SHOULD).
    if let Some(origin) = origin {
        request = request.with_header("Origin", origin);
    }
    let config = WebSocketConfig::default()
        .read_buffer_size(8 * 1024)
        .write_buffer_size(8 * 1024)
        .max_write_buffer_size(MAX_REALTIME_MESSAGE_BYTES + 16 * 1024)
        .max_message_size(Some(MAX_REALTIME_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_REALTIME_MESSAGE_BYTES));
    let (mut socket, response) = match connect_with_config(request, Some(config), 3) {
        Ok(connection) => connection,
        Err(error) => {
            let _ = events.try_send(WebSocketTransportEvent::Error(error.to_string()));
            return;
        }
    };
    set_websocket_read_timeout(socket.get_mut(), Some(Duration::from_millis(25)));
    let selected_protocol = response
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    // A non-conforming echo silently changing the subprotocol is a protocol
    // violation; fail instead of adopting it.
    if !protocol.is_empty() && selected_protocol != protocol {
        let _ = events.try_send(WebSocketTransportEvent::Error(format!(
            "server selected unexpected subprotocol {selected_protocol:?}"
        )));
        return;
    }
    let _ = events.try_send(WebSocketTransportEvent::Open(selected_protocol));

    loop {
        for _ in 0..MAX_EVENTS_PER_PUMP {
            match commands.try_recv() {
                Ok(WebSocketCommand::Send(message)) => {
                    let bytes = message_len(&message);
                    let frame = match message {
                        WebSocketMessage::Text(text) => Message::text(text),
                        WebSocketMessage::Binary(bytes) => Message::binary(bytes),
                    };
                    if let Err(error) = socket.send(frame) {
                        let _ = events.try_send(WebSocketTransportEvent::Error(error.to_string()));
                        return;
                    }
                    let _ = events.try_send(WebSocketTransportEvent::Sent(bytes));
                }
                Ok(WebSocketCommand::Close(code, reason)) => {
                    let frame = code.map(|code| CloseFrame {
                        code: code.into(),
                        reason: reason.unwrap_or_default().into(),
                    });
                    let _ = socket.close(frame);
                    let _ = events.try_send(WebSocketTransportEvent::Closed);
                    return;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    return;
                }
            }
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                let _ = events.try_send(WebSocketTransportEvent::Message(WebSocketMessage::Text(
                    text.to_string(),
                )));
            }
            Ok(Message::Binary(bytes)) => {
                let _ = events.try_send(WebSocketTransportEvent::Message(
                    WebSocketMessage::Binary(bytes.to_vec()),
                ));
            }
            Ok(Message::Close(_)) => {
                let _ = events.try_send(WebSocketTransportEvent::Closed);
                return;
            }
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                let _ = events.try_send(WebSocketTransportEvent::Closed);
                return;
            }
            Err(error) => {
                let _ = events.try_send(WebSocketTransportEvent::Error(error.to_string()));
                return;
            }
        }
    }
}

fn set_websocket_read_timeout(
    stream: &mut tungstenite::stream::MaybeTlsStream<std::net::TcpStream>,
    timeout: Option<Duration>,
) {
    match stream {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => {
            let _ = stream.set_read_timeout(timeout);
        }
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            let _ = stream.sock.set_read_timeout(timeout);
        }
        _ => {}
    }
}

fn message_len(message: &WebSocketMessage) -> usize {
    match message {
        WebSocketMessage::Text(text) => text.len(),
        WebSocketMessage::Binary(bytes) => bytes.len(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EventSourceReadyState {
    Connecting = 0,
    Open = 1,
    Closed = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSourceEvent {
    pub event_type: String,
    pub data: String,
    pub last_event_id: String,
}

#[derive(Debug)]
enum EventSourceTransportEvent {
    Open,
    Event(EventSourceEvent),
    Retry(u64),
    Closed,
    Error(String),
}

#[derive(Debug)]
pub struct EventSourceClient {
    pub url: String,
    pub ready_state: EventSourceReadyState,
    pub last_event_id: Option<String>,
    pub reconnect_interval_ms: u64,
    pub receive_queue: Vec<EventSourceEvent>,
    pub last_error: Option<String>,
    close_tx: SyncSender<()>,
    events: Receiver<EventSourceTransportEvent>,
}

impl EventSourceClient {
    pub fn new(url: impl Into<String>) -> Result<Self, String> {
        let url = url.into();
        let parsed = url::Url::parse(&url).map_err(|_| "Invalid EventSource URL".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err("EventSource URL must use http:// or https://".to_string());
        }
        let (close_tx, close_rx) = mpsc::sync_channel(1);
        let (event_tx, event_rx) = mpsc::sync_channel(MAX_REALTIME_QUEUE);
        let worker_url = url.clone();
        std::thread::Builder::new()
            .name("ghita-eventsource".to_string())
            .spawn(move || eventsource_transport(worker_url, close_rx, event_tx))
            .map_err(|error| format!("Cannot start EventSource transport: {error}"))?;
        Ok(Self {
            url,
            ready_state: EventSourceReadyState::Connecting,
            last_event_id: None,
            reconnect_interval_ms: 3000,
            receive_queue: Vec::new(),
            last_error: None,
            close_tx,
            events: event_rx,
        })
    }

    pub fn pump_transport(&mut self) {
        for _ in 0..MAX_EVENTS_PER_PUMP {
            let event = match self.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.ready_state = EventSourceReadyState::Closed;
                    break;
                }
            };
            match event {
                EventSourceTransportEvent::Open => {
                    self.ready_state = EventSourceReadyState::Open;
                }
                EventSourceTransportEvent::Event(event) => {
                    self.last_event_id = Some(event.last_event_id.clone());
                    if self.receive_queue.len() < MAX_REALTIME_QUEUE {
                        self.receive_queue.push(event);
                    }
                }
                EventSourceTransportEvent::Retry(milliseconds) => {
                    self.reconnect_interval_ms = milliseconds.clamp(100, 60_000);
                }
                EventSourceTransportEvent::Closed => {
                    self.ready_state = EventSourceReadyState::Closed;
                }
                EventSourceTransportEvent::Error(error) => {
                    self.last_error = Some(error);
                    self.ready_state = EventSourceReadyState::Closed;
                }
            }
        }
    }

    pub fn close(&mut self) {
        let _ = self.close_tx.try_send(());
        self.ready_state = EventSourceReadyState::Closed;
    }

    /// Feed an already received bounded SSE chunk. This remains public for
    /// embedders that own HTTP transport; normal page construction uses the
    /// background HTTP connection started by `new`.
    pub fn parse_sse_stream(&mut self, stream_text: &str) {
        if stream_text.len() > MAX_SSE_BUFFER_BYTES {
            self.last_error = Some("EventSource buffer exceeds 1 MB".to_string());
            self.ready_state = EventSourceReadyState::Closed;
            return;
        }
        let mut last_id = self.last_event_id.clone().unwrap_or_default();
        for item in parse_sse_events(stream_text, &mut last_id) {
            match item {
                EventSourceTransportEvent::Event(event) => {
                    self.last_event_id = Some(event.last_event_id.clone());
                    if self.receive_queue.len() < MAX_REALTIME_QUEUE {
                        self.receive_queue.push(event);
                    }
                }
                EventSourceTransportEvent::Retry(milliseconds) => {
                    self.reconnect_interval_ms = milliseconds.clamp(100, 60_000);
                }
                _ => {}
            }
        }
    }

    pub fn poll_event(&mut self) -> Option<EventSourceEvent> {
        self.pump_transport();
        if self.receive_queue.is_empty() {
            None
        } else {
            Some(self.receive_queue.remove(0))
        }
    }
}

/// Default delay before an EventSource reconnect attempt (spec default 3s),
/// overridable by the server's `retry:` field.
const SSE_RECONNECT_DEFAULT_MS: u64 = 3000;

fn eventsource_transport(
    url: String,
    close_rx: Receiver<()>,
    events: SyncSender<EventSourceTransportEvent>,
) {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_millis(500))
        .redirects(3)
        .build();
    let mut reconnect_ms = SSE_RECONNECT_DEFAULT_MS;
    let mut last_id = String::new();
    // WHATWG EventSource: the stream reconnects on any network end or error,
    // sending Last-Event-ID so the server can resume where it left off.
    loop {
        if close_rx.try_recv().is_ok() {
            let _ = events.try_send(EventSourceTransportEvent::Closed);
            return;
        }
        let mut request = agent
            .get(&url)
            .set("Accept", "text/event-stream")
            .set("Cache-Control", "no-cache");
        if !last_id.is_empty() {
            request = request.set("Last-Event-ID", &last_id);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(error) => {
                let _ = events.try_send(EventSourceTransportEvent::Error(error.to_string()));
                if !sse_backoff_wait(&close_rx, reconnect_ms) {
                    let _ = events.try_send(EventSourceTransportEvent::Closed);
                    return;
                }
                continue;
            }
        };
        let content_type = response.header("Content-Type").unwrap_or("");
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            let _ = events.try_send(EventSourceTransportEvent::Error(
                "EventSource response is not text/event-stream".to_string(),
            ));
            if !sse_backoff_wait(&close_rx, reconnect_ms) {
                let _ = events.try_send(EventSourceTransportEvent::Closed);
                return;
            }
            continue;
        }
        let _ = events.try_send(EventSourceTransportEvent::Open);
        let mut reader = response.into_reader();
        let mut chunk = [0_u8; 8192];
        // Incremental UTF-8 decoder: multi-byte characters split across read
        // boundaries used to become U+FFFD in both chunks.
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut pending = String::new();
        loop {
            if close_rx.try_recv().is_ok() {
                let _ = events.try_send(EventSourceTransportEvent::Closed);
                return;
            }
            match reader.read(&mut chunk) {
                Ok(0) => {
                    // Flush any buffered partial character, dispatch what is
                    // queued, then reconnect per spec.
                    let mut flushed = String::new();
                    let _ = decoder.decode_to_string(b"", &mut flushed, true);
                    pending.push_str(&flushed);
                    if !pending.trim().is_empty() {
                        for event in parse_sse_events(&pending, &mut last_id) {
                            sse_dispatch(event, &events, &mut reconnect_ms);
                        }
                    }
                    pending.clear();
                    let _ = events.try_send(EventSourceTransportEvent::Closed);
                    break;
                }
                Ok(read) => {
                    pending.reserve(read);
                    let (_, _, _) =
                        decoder.decode_to_string(&chunk[..read], &mut pending, false);
                    if pending.len() > MAX_SSE_BUFFER_BYTES {
                        let _ = events.try_send(EventSourceTransportEvent::Error(
                            "EventSource buffer exceeds 1 MB".to_string(),
                        ));
                        return;
                    }
                    while let Some(boundary) = sse_event_boundary(&pending) {
                        let payload = pending[..boundary].to_string();
                        let consumed = if pending[boundary..].starts_with("\r\n\r\n") {
                            boundary + 4
                        } else {
                            boundary + 2
                        };
                        pending.drain(..consumed);
                        for event in parse_sse_events(&payload, &mut last_id) {
                            sse_dispatch(event, &events, &mut reconnect_ms);
                        }
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => {
                    let _ = events.try_send(EventSourceTransportEvent::Error(error.to_string()));
                    break;
                }
            }
        }
        if !sse_backoff_wait(&close_rx, reconnect_ms) {
            let _ = events.try_send(EventSourceTransportEvent::Closed);
            return;
        }
    }
}

/// Forward a parsed transport event, learning `retry:` intervals locally so
/// reconnections honor the server-requested pacing.
fn sse_dispatch(
    event: EventSourceTransportEvent,
    events: &SyncSender<EventSourceTransportEvent>,
    reconnect_ms: &mut u64,
) {
    if let EventSourceTransportEvent::Retry(milliseconds) = event {
        *reconnect_ms = milliseconds.clamp(100, 60_000);
    }
    let _ = events.try_send(event);
}

/// Wait out the reconnect delay, aborting early when the stream is closed.
/// Returns false when the caller should shut down.
fn sse_backoff_wait(close_rx: &Receiver<()>, delay_ms: u64) -> bool {
    let waited_for = Duration::from_millis(delay_ms.min(60_000));
    let deadline = std::time::Instant::now() + waited_for;
    while std::time::Instant::now() < deadline {
        if close_rx.try_recv().is_ok() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

fn sse_event_boundary(buffer: &str) -> Option<usize> {
    match (buffer.find("\n\n"), buffer.find("\r\n\r\n")) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(index), None) | (None, Some(index)) => Some(index),
        (None, None) => None,
    }
}

fn parse_sse_events(text: &str, last_id: &mut String) -> Vec<EventSourceTransportEvent> {
    let mut events = Vec::new();
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let mut event_type = "message".to_string();
        let mut data = Vec::new();
        let mut event_id = last_id.clone();
        for line in block.lines() {
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => event_type = value.to_string(),
                "data" => data.push(value.to_string()),
                "id" if !value.contains('\0') => event_id = value.to_string(),
                "retry" => {
                    if let Ok(milliseconds) = value.parse::<u64>() {
                        events.push(EventSourceTransportEvent::Retry(milliseconds));
                    }
                }
                _ => {}
            }
        }
        if !data.is_empty() {
            *last_id = event_id.clone();
            events.push(EventSourceTransportEvent::Event(EventSourceEvent {
                event_type,
                data: data.join("\n"),
                last_event_id: event_id,
            }));
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eventsource_parser_tracks_fields_and_retry() {
        let mut client = EventSourceClient::new("http://127.0.0.1:9/events").unwrap();
        client.ready_state = EventSourceReadyState::Open;
        client.parse_sse_stream("event: update\ndata: {\"score\":10}\nid: 101\nretry: 5000\n\n");
        let event = client.receive_queue.remove(0);
        assert_eq!(event.event_type, "update");
        assert_eq!(event.last_event_id, "101");
        assert_eq!(client.reconnect_interval_ms, 5000);
    }

    #[test]
    fn realtime_urls_and_budgets_fail_closed() {
        assert!(WebSocketClient::new("https://example.test", None).is_err());
        assert!(EventSourceClient::new("file:///events").is_err());
        let long_protocol = "x".repeat(257);
        assert!(WebSocketClient::new("ws://127.0.0.1:9/", Some(&long_protocol)).is_err());
    }
}

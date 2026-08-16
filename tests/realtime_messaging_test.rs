//! Phase 22 real-time transport and cross-context messaging integration tests.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use ghitabrowser::messaging::{structured_clone, BroadcastChannel};
use ghitabrowser::realtime::{
    EventSourceClient, EventSourceReadyState, WebSocketClient, WebSocketMessage,
    WebSocketReadyState,
};

#[test]
fn websocket_connection_and_frame_exchange_uses_real_loopback_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("WebSocket listener");
    let address = listener.local_addr().expect("listener address");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("WebSocket accept");
        let mut socket = tungstenite::accept(stream).expect("server handshake");
        let message = socket.read().expect("server receive");
        assert_eq!(message, tungstenite::Message::text("ping"));
        socket
            .send(tungstenite::Message::text("pong"))
            .expect("server echo");
        let _ = socket.read();
    });

    let mut ws = WebSocketClient::new(format!("ws://{address}/echo"), None).expect("ws new");
    let deadline = Instant::now() + Duration::from_secs(5);
    while ws.ready_state == WebSocketReadyState::Connecting && Instant::now() < deadline {
        ws.pump_transport();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        ws.ready_state,
        WebSocketReadyState::Open,
        "{:?}",
        ws.last_error
    );

    ws.send(WebSocketMessage::Text("ping".to_string()))
        .expect("send");
    let incoming = loop {
        if let Some(message) = ws.poll_incoming() {
            break message;
        }
        assert!(Instant::now() < deadline, "WebSocket echo timed out");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(incoming, WebSocketMessage::Text("pong".to_string()));

    ws.close(None, None).expect("close");
    while ws.ready_state != WebSocketReadyState::Closed && Instant::now() < deadline {
        ws.pump_transport();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(ws.ready_state, WebSocketReadyState::Closed);
    server.join().expect("WebSocket server");
}

#[test]
fn eventsource_reads_real_http_event_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("SSE listener");
    let address = listener.local_addr().expect("listener address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("SSE accept");
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).expect("read SSE request");
        let body =
            "event: notification\ndata: {\"alert\":\"new_mail\"}\nid: evt_42\nretry: 5000\n\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write SSE response");
        stream.flush().expect("flush SSE response");
    });

    let mut es = EventSourceClient::new(format!("http://{address}/feed")).expect("es new");
    let deadline = Instant::now() + Duration::from_secs(5);
    let event = loop {
        if let Some(event) = es.poll_event() {
            break event;
        }
        assert!(
            Instant::now() < deadline,
            "SSE event timed out: {:?}",
            es.last_error
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(event.event_type, "notification");
    assert_eq!(event.data, "{\"alert\":\"new_mail\"}");
    assert_eq!(event.last_event_id, "evt_42");
    assert_eq!(es.reconnect_interval_ms, 5000);
    assert!(matches!(
        es.ready_state,
        EventSourceReadyState::Open | EventSourceReadyState::Closed
    ));
    server.join().expect("SSE server");
}

#[test]
fn broadcast_channel_origin_partitioning() {
    let mut sender = BroadcastChannel::new("https://app.com", "chat");
    let mut receiver_same = BroadcastChannel::new("https://app.com", "chat");
    let mut receiver_diff = BroadcastChannel::new("https://other.com", "chat");

    sender
        .post_message("{\"user\":\"Alice\",\"text\":\"Hi\"}".to_string())
        .expect("post_message");
    assert!(receiver_same
        .poll_message()
        .expect("poll message")
        .contains("Alice"));
    assert!(sender.poll_message().is_none());
    assert!(receiver_diff.poll_message().is_none());
}

#[test]
fn structured_clone_deep_copy_and_depth_limit() {
    let value: serde_json::Value = serde_json::json!({
        "number": 42,
        "string": "text",
        "nested": { "array": [1, 2, 3] }
    });
    assert_eq!(structured_clone(&value, 0).expect("cloned"), value);
    let error = structured_clone(&value, 65).expect_err("depth cap");
    assert!(error.contains("DataCloneError"));
}

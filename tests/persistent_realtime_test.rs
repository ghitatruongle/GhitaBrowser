use ghitabrowser::background_worker::{
    BackgroundTask, BackgroundTaskKind, BackgroundWorkerManager, BackgroundWorkerPolicy,
    MAX_QUEUED_PAYLOAD_BYTES_PER_WORKER, MAX_TASK_PAYLOAD_BYTES,
};
use ghitabrowser::indexeddb::{
    IDBCursorDirection, IDBIndexConfig, IDBKey, IDBKeyRange, IDBObjectStore,
};
use ghitabrowser::push::{PushManager, PushMessage};
use ghitabrowser::webtransport::{WebTransportRegistry, WebTransportStreamDirection};

#[test]
fn indexeddb_unique_indexes_and_cursors_are_bounded_and_deterministic() {
    let mut store = IDBObjectStore::new("notes", None, false);
    store
        .create_index(IDBIndexConfig {
            name: "slug".into(),
            key_path: "slug".into(),
            unique: true,
            multi_entry: false,
        })
        .unwrap();
    store
        .put(
            Some(IDBKey::Number(1.0)),
            r#"{"slug":"first","body":"hello"}"#.into(),
        )
        .unwrap();
    assert!(store
        .put(
            Some(IDBKey::Number(2.0)),
            r#"{"slug":"first","body":"duplicate"}"#.into(),
        )
        .is_err());
    let cursor = store.open_cursor(
        Some(&IDBKeyRange::only(IDBKey::Number(1.0))),
        IDBCursorDirection::Next,
    );
    assert_eq!(cursor.current().unwrap().key, IDBKey::Number(1.0));
    assert_eq!(cursor.remaining(), 1);
}

#[test]
fn background_push_and_webtransport_enforce_origin_and_lifetime_boundaries() {
    let origin = "https://localhost";
    let mut workers = BackgroundWorkerManager::default();
    let worker = workers
        .register(origin, "/app/", BackgroundWorkerPolicy::default())
        .unwrap();
    workers
        .enqueue(
            worker,
            BackgroundTask {
                kind: BackgroundTaskKind::Sync,
                payload: b"sync".to_vec(),
                created_at_ms: 1,
            },
        )
        .unwrap();
    assert_eq!(workers.wake(worker, 1).unwrap().len(), 1);

    let full_tasks = MAX_QUEUED_PAYLOAD_BYTES_PER_WORKER / MAX_TASK_PAYLOAD_BYTES;
    for nonce in 0..full_tasks {
        workers
            .enqueue(
                worker,
                BackgroundTask {
                    kind: BackgroundTaskKind::Message,
                    payload: vec![0; MAX_TASK_PAYLOAD_BYTES],
                    created_at_ms: nonce as u64,
                },
            )
            .unwrap();
    }
    assert!(workers
        .enqueue(
            worker,
            BackgroundTask {
                kind: BackgroundTaskKind::Message,
                payload: vec![0],
                created_at_ms: 99,
            },
        )
        .is_err());
    assert_eq!(workers.wake(worker, 1).unwrap().len(), full_tasks);

    let mut push = PushManager::default();
    let subscription = push
        .subscribe(origin, worker, "https://localhost/push", vec![7; 32], None)
        .unwrap();
    assert_eq!(
        push.deliver(
            PushMessage {
                subscription_id: subscription.id,
                payload: b"message".to_vec(),
                issued_at_ms: 2,
                nonce: 1,
            },
            2,
        )
        .unwrap()
        .0,
        worker
    );
    assert!(push.unsubscribe(subscription.id));
    assert!(push.get(subscription.id).is_none());

    let mut transport = WebTransportRegistry::default();
    let session = transport
        .connect(origin, "https://localhost/realtime")
        .unwrap();
    let stream = transport
        .create_stream(session, WebTransportStreamDirection::Bidirectional)
        .unwrap();
    transport
        .send_stream_data(session, stream, b"stream-out".to_vec())
        .unwrap();
    assert_eq!(
        transport
            .take_outbound_stream_data(session, stream)
            .unwrap(),
        Some(b"stream-out".to_vec())
    );
    transport
        .receive_stream_data(session, stream, b"stream-in".to_vec())
        .unwrap();
    assert_eq!(
        transport.read_stream_data(session, stream).unwrap(),
        Some(b"stream-in".to_vec())
    );
    transport.send_datagram(session, b"ping".to_vec()).unwrap();
    assert_eq!(
        transport.take_outbound_datagram(session).unwrap(),
        Some(b"ping".to_vec())
    );
    transport
        .receive_datagram(session, b"pong".to_vec())
        .unwrap();
    assert_eq!(
        transport.read_datagram(session).unwrap(),
        Some(b"pong".to_vec())
    );
}

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ghitabrowser::network_scheduler::{
    fetch_document_bundle, CancellationToken, NetworkScheduler, RequestPriority, ReqwestTransport,
    ResponseMode, ScheduledError, ScheduledRequest, SchedulerLimits,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_hundred_mixed_loopback_requests_do_not_starve_navigation() {
    const BACKGROUND_REQUESTS: usize = 200;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let server_observed = Arc::clone(&observed);
    let server = std::thread::spawn(move || {
        for _ in 0..=BACKGROUND_REQUESTS {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0u8; 4_096];
            let length = stream.read(&mut request).unwrap();
            let first_line = String::from_utf8_lossy(&request[..length])
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            server_observed.lock().unwrap().push(first_line);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .unwrap();
        }
    });

    let scheduler = NetworkScheduler::new(
        ReqwestTransport::new().unwrap(),
        SchedulerLimits {
            max_concurrency: 8,
            max_queued: 256,
            max_response_bytes: 1_024,
            request_timeout: Duration::from_secs(10),
        },
    )
    .unwrap();
    let mut requests = (0..BACKGROUND_REQUESTS)
        .map(|id| {
            (
                ScheduledRequest {
                    id: id as u64,
                    url: format!("http://{address}/background/{id}"),
                    cookie_header: String::new(),
                    max_retries: 0,
                    priority: RequestPriority::Background,
                    response_mode: ResponseMode::Document,
                },
                CancellationToken::default(),
            )
        })
        .collect::<Vec<_>>();
    requests.push((
        ScheduledRequest {
            id: 10_000,
            url: format!("http://{address}/navigation"),
            cookie_header: String::new(),
            max_retries: 0,
            priority: RequestPriority::Navigation,
            response_mode: ResponseMode::Document,
        },
        CancellationToken::default(),
    ));

    let responses = scheduler.execute_batch(requests).await;
    assert_eq!(responses.len(), BACKGROUND_REQUESTS + 1);
    assert!(responses.iter().all(|response| response.result.is_ok()));
    assert_eq!(responses[0].request_id, 10_000);
    assert_eq!(scheduler.queued_len(), 0);
    server.join().unwrap();
    let observed = observed.lock().unwrap();
    let navigation_position = observed
        .iter()
        .position(|line| line.contains("/navigation"))
        .unwrap();
    assert!(navigation_position < 8, "navigation started too late");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_streaming_response_closes_the_in_flight_async_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (streaming_tx, streaming_rx) = std::sync::mpsc::channel();
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4_096];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
            )
            .unwrap();
        streaming_tx.send(()).unwrap();
        let payload = vec![b'x'; 16 * 1024];
        loop {
            let header = format!("{:x}\r\n", payload.len());
            let result = stream
                .write_all(header.as_bytes())
                .and_then(|_| stream.write_all(&payload))
                .and_then(|_| stream.write_all(b"\r\n"))
                .and_then(|_| stream.flush());
            if result.is_err() {
                let _ = closed_tx.send(());
                break;
            }
        }
    });

    let scheduler = NetworkScheduler::new(
        ReqwestTransport::new().unwrap(),
        SchedulerLimits {
            max_concurrency: 1,
            max_queued: 4,
            max_response_bytes: 50 * 1024 * 1024,
            request_timeout: Duration::from_secs(10),
        },
    )
    .unwrap();
    let cancellation = CancellationToken::default();
    let operation_cancellation = cancellation.clone();
    let operation = tokio::spawn(async move {
        scheduler
            .fetch(
                ScheduledRequest {
                    id: 77,
                    url: format!("http://{address}/stream"),
                    cookie_header: String::new(),
                    max_retries: 0,
                    priority: RequestPriority::Media,
                    response_mode: ResponseMode::Binary,
                },
                operation_cancellation,
            )
            .await
    });

    tokio::task::spawn_blocking(move || streaming_rx.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    cancellation.cancel();
    let response = tokio::time::timeout(Duration::from_secs(2), operation)
        .await
        .expect("cancelled request must return promptly")
        .unwrap();
    assert!(matches!(response.result, Err(ScheduledError::Cancelled)));
    tokio::task::spawn_blocking(move || closed_rx.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .expect("server must observe the cancelled socket closing");
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_bundle_fetches_and_inlines_external_script_and_style() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let server_observed = Arc::clone(&observed);
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0_u8; 8_192];
            let length = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..length]).to_string();
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            server_observed
                .lock()
                .unwrap()
                .push((path.clone(), request.clone()));
            let (content_type, body) = match path.as_str() {
                "/app.js" => ("text/javascript", "window.bundleLoaded = true;"),
                "/app.css" => ("text/css", "main { color: rgb(1, 2, 3); }"),
                _ => (
                    "text/html; charset=utf-8",
                    "<!doctype html><html><head><link rel=\"stylesheet\" href=\"/app.css\"></head><body><main>ready</main><script src=\"/app.js\"></script></body></html>",
                ),
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let result = fetch_document_bundle(
        format!("http://{address}/index.html"),
        "session=phase19".to_string(),
        0,
        CancellationToken::default(),
    )
    .await
    .unwrap();

    assert!(result.body.contains("window.bundleLoaded = true;"));
    assert!(result.body.contains("main { color: rgb(1, 2, 3); }"));
    assert!(!result.body.contains("src=\"/app.js\""));
    assert!(!result.body.contains("href=\"/app.css\""));
    assert_eq!(
        result
            .headers
            .get("x-ghita-external-resource-failures")
            .map(String::as_str),
        Some("0")
    );
    server.join().unwrap();
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 3);
    assert!(observed
        .iter()
        .all(|(_, request)| request.contains("cookie: session=phase19")));
}

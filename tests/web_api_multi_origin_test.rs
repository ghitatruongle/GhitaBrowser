use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use ghitabrowser::web_api::{
    AbortController, CredentialsMode, FetchError, FetchPromiseState, FetchRuntime, RedirectMode,
    RequestMode, ResponseType, WebRequest, XhrEvent, XhrReadyState, XmlHttpRequest,
};

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let count = stream.read(&mut chunk).expect("read request");
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header_text = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
        let content_length = header_text
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or_default();
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

fn http_response(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    response.push_str(body);
    response
}

fn spawn_server<F>(
    request_count: usize,
    handler: F,
) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>)
where
    F: Fn(usize, &str) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
    let address = listener.local_addr().expect("local address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let server = std::thread::spawn(move || {
        for index in 0..request_count {
            let (mut stream, _) = listener.accept().expect("accept request");
            let request = read_http_request(&mut stream);
            recorded.lock().unwrap().push(request.clone());
            let response = handler(index, &request);
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        }
    });
    (format!("http://{address}"), requests, server)
}

#[test]
fn fetch_promise_is_pending_then_same_origin_response_is_fulfilled() {
    let (origin, requests, server) = spawn_server(1, |_, _| {
        http_response(
            "200 OK",
            &[("Content-Type", "application/json")],
            r#"{"phase":12}"#,
        )
    });
    let mut runtime = FetchRuntime::new(&origin).unwrap();
    let promise = runtime
        .fetch(WebRequest::get(&format!("{origin}/data")).unwrap())
        .unwrap();
    assert_eq!(runtime.promise(promise), Some(&FetchPromiseState::Pending));
    assert!(runtime.run_one());
    let FetchPromiseState::Fulfilled(response) = runtime.promise(promise).unwrap() else {
        panic!("fetch promise should be fulfilled")
    };
    assert_eq!(response.response_type, ResponseType::Basic);
    assert_eq!(response.json().unwrap()["phase"], 12);
    server.join().unwrap();
    assert!(requests.lock().unwrap()[0].starts_with("GET /data "));
}

#[test]
fn cors_preflight_allows_declared_method_and_header_and_filters_response_headers() {
    let client_origin = "http://127.0.0.1:65530".to_string();
    let allowed_origin = client_origin.clone();
    let (server_origin, requests, server) = spawn_server(2, move |index, _| {
        if index == 0 {
            http_response(
                "204 No Content",
                &[
                    ("Access-Control-Allow-Origin", &allowed_origin),
                    ("Access-Control-Allow-Methods", "PUT"),
                    ("Access-Control-Allow-Headers", "x-phase"),
                ],
                "",
            )
        } else {
            http_response(
                "200 OK",
                &[
                    ("Access-Control-Allow-Origin", &allowed_origin),
                    ("Access-Control-Expose-Headers", "x-phase"),
                    ("X-Phase", "12"),
                    ("X-Private", "hidden"),
                ],
                "updated",
            )
        }
    });
    let mut request = WebRequest::get(&format!("{server_origin}/resource")).unwrap();
    request.set_method("PUT").unwrap();
    request.headers.set("X-Phase", "12").unwrap();
    request.set_body(b"payload".to_vec()).unwrap();

    let mut runtime = FetchRuntime::new(&client_origin).unwrap();
    let promise = runtime.fetch(request).unwrap();
    runtime.drain(8);
    let state = runtime.promise(promise).unwrap();
    let FetchPromiseState::Fulfilled(response) = state else {
        panic!("CORS fetch should be fulfilled, got {state:?} for {server_origin}")
    };
    assert_eq!(response.response_type, ResponseType::Cors);
    assert_eq!(response.text().unwrap(), "updated");
    assert_eq!(response.headers.get("x-phase").as_deref(), Some("12"));
    assert!(!response.headers.has("x-private"));

    server.join().unwrap();
    let requests = requests.lock().unwrap();
    let preflight = requests[0].to_ascii_lowercase();
    assert!(preflight.starts_with("options /resource "));
    assert!(preflight.contains("origin: http://127.0.0.1:65530"));
    assert!(preflight.contains("access-control-request-method: put"));
    assert!(preflight.contains("access-control-request-headers: x-phase"));
    assert!(requests[1].starts_with("PUT /resource "));
}

#[test]
fn cors_denial_wildcard_credentials_no_cors_and_abort_fail_closed() {
    let client_origin = "http://127.0.0.1:65529";
    let (denied_origin, _, denied_server) = spawn_server(1, |_, _| {
        http_response("200 OK", &[("Content-Type", "text/plain")], "secret")
    });
    let mut runtime = FetchRuntime::new(client_origin).unwrap();
    let denied = runtime
        .fetch(WebRequest::get(&format!("{denied_origin}/denied")).unwrap())
        .unwrap();
    runtime.run_one();
    assert!(matches!(
        runtime.promise(denied),
        Some(FetchPromiseState::Rejected(FetchError::Cors(_)))
    ));
    denied_server.join().unwrap();

    let (wildcard_origin, _, wildcard_server) = spawn_server(1, |_, _| {
        http_response(
            "200 OK",
            &[("Access-Control-Allow-Origin", "*")],
            "credential leak",
        )
    });
    let mut credentialed = WebRequest::get(&format!("{wildcard_origin}/private")).unwrap();
    credentialed.credentials = CredentialsMode::Include;
    let wildcard = runtime.fetch(credentialed).unwrap();
    runtime.run_one();
    assert!(matches!(
        runtime.promise(wildcard),
        Some(FetchPromiseState::Rejected(FetchError::Cors(_)))
    ));
    wildcard_server.join().unwrap();

    let (opaque_origin, _, opaque_server) = spawn_server(1, |_, _| {
        http_response("200 OK", &[("X-Secret", "not visible")], "opaque")
    });
    let mut no_cors = WebRequest::get(&format!("{opaque_origin}/pixel")).unwrap();
    no_cors.mode = RequestMode::NoCors;
    let opaque = runtime.fetch(no_cors).unwrap();
    runtime.run_one();
    let FetchPromiseState::Fulfilled(response) = runtime.promise(opaque).unwrap() else {
        panic!("no-cors request should return an opaque response")
    };
    assert_eq!(response.response_type, ResponseType::Opaque);
    assert_eq!(response.status, 0);
    assert!(response.body.is_empty());
    opaque_server.join().unwrap();

    let controller = AbortController::new();
    let mut aborted_request = WebRequest::get("http://127.0.0.1:9/never").unwrap();
    aborted_request.signal = Some(controller.signal());
    let aborted = runtime.fetch(aborted_request).unwrap();
    controller.abort();
    runtime.run_one();
    assert_eq!(
        runtime.promise(aborted),
        Some(&FetchPromiseState::Rejected(FetchError::Aborted))
    );
}

#[test]
fn cross_origin_redirect_strips_authorization_and_manual_mode_is_opaque() {
    let target_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let target_address = target_listener.local_addr().unwrap();
    let redirect_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let redirect_address = redirect_listener.local_addr().unwrap();
    let client_origin = format!("http://{redirect_address}");
    let allow_origin = client_origin.clone();

    let target_request = Arc::new(Mutex::new(String::new()));
    let captured_target = Arc::clone(&target_request);
    let target_server = std::thread::spawn(move || {
        let (mut stream, _) = target_listener.accept().unwrap();
        *captured_target.lock().unwrap() = read_http_request(&mut stream);
        let response = http_response(
            "200 OK",
            &[("Access-Control-Allow-Origin", &allow_origin)],
            "redirected",
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    let redirect_server = std::thread::spawn(move || {
        let (mut stream, _) = redirect_listener.accept().unwrap();
        let request = read_http_request(&mut stream).to_ascii_lowercase();
        assert!(request.contains("authorization: bearer phase12"));
        let location = format!("http://{target_address}/final");
        let response = http_response("302 Found", &[("Location", &location)], "");
        stream.write_all(response.as_bytes()).unwrap();
    });

    let mut runtime = FetchRuntime::new(&client_origin).unwrap();
    let mut request = WebRequest::get(&format!("{client_origin}/start")).unwrap();
    request
        .headers
        .set("Authorization", "Bearer phase12")
        .unwrap();
    let promise = runtime.fetch(request).unwrap();
    runtime.run_one();
    let FetchPromiseState::Fulfilled(response) = runtime.promise(promise).unwrap() else {
        panic!("redirect should be followed")
    };
    assert!(response.redirected);
    assert_eq!(response.response_type, ResponseType::Cors);
    redirect_server.join().unwrap();
    target_server.join().unwrap();
    assert!(!target_request
        .lock()
        .unwrap()
        .to_ascii_lowercase()
        .contains("authorization:"));

    let (manual_origin, _, manual_server) = spawn_server(1, |_, _| {
        http_response("302 Found", &[("Location", "/final")], "")
    });
    let mut manual_runtime = FetchRuntime::new(&manual_origin).unwrap();
    let mut manual_request = WebRequest::get(&format!("{manual_origin}/start")).unwrap();
    manual_request.redirect = RedirectMode::Manual;
    let manual = manual_runtime.fetch(manual_request).unwrap();
    manual_runtime.run_one();
    let FetchPromiseState::Fulfilled(response) = manual_runtime.promise(manual).unwrap() else {
        panic!("manual redirect should settle")
    };
    assert_eq!(response.response_type, ResponseType::OpaqueRedirect);
    assert_eq!(response.status, 0);
    manual_server.join().unwrap();
}

#[test]
fn xhr_compatibility_reports_ready_states_and_load() {
    let (origin, _, server) = spawn_server(1, |_, _| {
        http_response("200 OK", &[("Content-Type", "text/plain")], "xhr ready")
    });
    let mut runtime = FetchRuntime::new(&origin).unwrap();
    let mut xhr = XmlHttpRequest::new();
    xhr.open("GET", &format!("{origin}/xhr")).unwrap();
    xhr.send(&mut runtime, None).unwrap();
    assert_eq!(xhr.ready_state(), XhrReadyState::Opened);
    runtime.run_one();
    assert!(xhr.poll(&mut runtime));
    assert_eq!(xhr.ready_state(), XhrReadyState::Done);
    assert_eq!(xhr.status(), 200);
    assert_eq!(xhr.response_text().unwrap(), "xhr ready");
    assert!(xhr.events().contains(&XhrEvent::Load));
    server.join().unwrap();
}

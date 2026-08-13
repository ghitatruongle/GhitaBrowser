use std::io::{Read, Write};
use std::net::TcpListener;

fn fetch_local_response(
    body: Vec<u8>,
    content_type: &str,
    content_encoding: Option<&str>,
) -> ghitabrowser::network::FetchResult {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
    let address = listener.local_addr().expect("local address");
    let content_type = content_type.to_string();
    let content_encoding = content_encoding.map(str::to_string);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let mut request = [0_u8; 4096];
        let count = stream.read(&mut request).expect("read request");
        let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
        assert!(request.contains("accept-encoding: gzip, br"));
        let encoding_header = content_encoding
            .map(|value| format!("Content-Encoding: {value}\r\n"))
            .unwrap_or_default();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{encoding_header}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).expect("write header");
        stream.write_all(&body).expect("write body");
    });
    let result = ghitabrowser::network::fetch_url(&format!("http://{address}/document"))
        .expect("fetch encoded local response");
    server.join().expect("server exits cleanly");
    result
}

#[test]
fn follows_relative_redirect_on_local_server() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
    let address = listener.local_addr().expect("local address");
    let server = std::thread::spawn(move || {
        for expected_path in ["/start", "/final"] {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 2048];
            let count = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.starts_with(&format!("GET {expected_path} ")));

            if expected_path == "/start" {
                stream
                    .write_all(
                        b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .expect("write redirect");
            } else {
                let body = b"<html><body>local success</body></html>";
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).expect("write header");
                stream.write_all(body).expect("write body");
            }
        }
    });

    let start = format!("http://{address}/start");
    let result = ghitabrowser::network::fetch_url(&start).expect("fetch local redirect");
    assert_eq!(result.status_code, 200);
    assert!(result.url.ends_with("/final"));
    assert!(result.body.contains("local success"));
    server.join().expect("server exits cleanly");
}

#[test]
fn decodes_declared_charset_gzip_and_brotli() {
    let latin1 = fetch_local_response(
        b"<html><body>Xin vui l\xf2ng</body></html>".to_vec(),
        "text/html; charset=ISO-8859-1",
        None,
    );
    assert!(latin1.body.contains("lòng"));

    let gzip = fetch_local_response(
        vec![
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xb3, 0xc9, 0x28, 0xc9,
            0xcd, 0xb1, 0xb3, 0x49, 0xca, 0x4f, 0xa9, 0xb4, 0x73, 0xaf, 0xca, 0x2c, 0x50, 0x28,
            0x2e, 0x4d, 0x4e, 0x4e, 0x2d, 0x2e, 0xb6, 0xd1, 0x07, 0x0b, 0xd9, 0xe8, 0x83, 0xe5,
            0x01, 0x35, 0xf8, 0x34, 0x6f, 0x26, 0x00, 0x00, 0x00,
        ],
        "text/html; charset=UTF-8",
        Some("gzip"),
    );
    assert!(gzip.body.contains("Gzip success"));

    let brotli = fetch_local_response(
        vec![
            0x1f, 0x27, 0x00, 0xf8, 0x1d, 0x89, 0x71, 0x2c, 0xc5, 0x8a, 0x26, 0x84, 0x97, 0x74,
            0x3c, 0x20, 0x2d, 0x84, 0xf6, 0x26, 0xa7, 0x7d, 0xd2, 0xd8, 0xda, 0x24, 0x2d, 0xb5,
            0x08, 0x93, 0x2c, 0x70, 0x02, 0x0c, 0xeb, 0x98, 0xcb, 0xc9, 0x4c, 0x02,
        ],
        "text/html; charset=UTF-8",
        Some("br"),
    );
    assert!(brotli.body.contains("Brotli success"));
}

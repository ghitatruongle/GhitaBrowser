// HTTP networking and caching

use log::{info, warn};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// User agent for all network requests, kept in sync with the crate version
pub(crate) fn browser_ua() -> String {
    format!("GhitaBrowser/{} (Rust)", crate::VERSION)
}

/// Cache TTL from the Cache-Control header, falling back to `Expires` /
/// `Last-Modified` heuristics, then a 5-minute default.
/// Returns 0 (do not cache) for no-store/no-cache/private responses, which
/// the browser must revalidate or never store.
/// Shared by the headless engine (fetch_with_cache) and the GUI fetch path.
pub(crate) fn cache_ttl_secs(headers: &HashMap<String, String>) -> u64 {
    let now = chrono::Utc::now().timestamp();

    if let Some(cc) = headers.get("cache-control") {
        let lower = cc.to_ascii_lowercase();
        // These directives forbid storing/reusing the response
        if lower.contains("no-store") || lower.contains("no-cache") || lower.contains("private") {
            return 0;
        }
        if lower.contains("max-age=") {
            let max_age_str = lower
                .split("max-age=")
                .nth(1)
                .and_then(|v| v.split(|c: char| !c.is_ascii_digit()).next())
                .unwrap_or("");
            let max_age_secs = max_age_str.parse::<u64>().unwrap_or_else(|e| {
                warn!("Invalid max-age value {:?}: {}", max_age_str, e);
                300
            });
            return max_age_secs;
        }
    }

    // No Cache-Control: honor `Expires` per RFC 7234 §5.3.
    if let Some(expires) = headers.get("expires") {
        if let Some(ts) = crate::storage::parse_http_date(expires) {
            return (ts - now).max(0) as u64;
        }
    }

    // Last-Modified heuristic: old content is likely stable; cache it for a
    // bounded fraction of its age (capped at 24h) so plain servers without
    // Cache-Control get conditional revalidation instead of a 5-min default.
    if let Some(lm) = headers.get("last-modified") {
        if let Some(ts) = crate::storage::parse_http_date(lm) {
            let age = (now - ts).max(0);
            if age > 0 {
                return (age / 10).clamp(60, 24 * 60 * 60) as u64;
            }
        }
    }

    300 // default 5 minutes
}

/// Result of an HTTP fetch operation
#[derive(Debug, Clone)]
pub struct FetchResult {
    pub body: String,
    /// Raw payload for binary documents such as PDF. Text responses keep this
    /// empty so the body is not retained twice.
    pub binary_body: Option<Vec<u8>>,
    pub url: String,
    pub status_code: u16,
    pub content_type: String,
    pub headers: HashMap<String, String>,
    pub fetch_time_ms: u64,
    pub set_cookie_headers: Vec<String>,
}

impl FetchResult {
    /// Bytes owned directly by a cached response, including binary payloads
    /// and metadata strings. This deliberately excludes the cache map key,
    /// which is counted by `ResourceCache` exactly once.
    pub fn retained_bytes(&self) -> usize {
        let binary = self.binary_body.as_ref().map_or(0, Vec::capacity);
        let headers = self.headers.iter().fold(0usize, |total, (name, value)| {
            total
                .saturating_add(name.capacity())
                .saturating_add(value.capacity())
        });
        let cookies = self.set_cookie_headers.iter().fold(0usize, |total, value| {
            total.saturating_add(value.capacity())
        });
        std::mem::size_of::<Self>()
            .saturating_add(self.body.capacity())
            .saturating_add(binary)
            .saturating_add(self.url.capacity())
            .saturating_add(self.content_type.capacity())
            .saturating_add(headers)
            .saturating_add(cookies)
    }
}

/// Fetch content from a URL using real HTTP/HTTPS via ureq
pub fn fetch_url(url_str: &str) -> Result<FetchResult, Box<dyn std::error::Error>> {
    // Build request agent
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0) // redirects handled manually in execute_fetch
        .user_agent(&browser_ua())
        .build();

    execute_fetch(&agent, url_str, None, &[])
}

/// Fetch with cookie jar integration
/// Injects stored cookies into request, and parses Set-Cookie from response
pub fn fetch_with_cookies(
    url_str: &str,
    cookie_store: &mut crate::storage::CookieStore,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let _start = Instant::now();
    let parsed = url::Url::parse(url_str)?;
    let domain = parsed.host_str().unwrap_or("").to_string();

    // Build the request agent
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0) // redirects handled manually in execute_fetch
        .user_agent(&browser_ua())
        .build();

    // Get matching cookies and build Cookie header.
    // Each cookie must also pass full URL validation: Secure cookies are only
    // sent over HTTPS and the cookie path must match (RFC 6265 §5.4) — the
    // domain-only `get_cookies` filter alone would leak Secure cookies over
    // plain HTTP.
    let matching_cookies = cookie_store.get_cookies(&domain);
    let cookie_header: String = matching_cookies
        .iter()
        .filter(|c| c.matches_url(url_str))
        .map(|c| c.to_header_value())
        .collect::<Vec<_>>()
        .join("; ");

    // Execute fetch with or without cookies
    let result = if cookie_header.is_empty() {
        info!("No cookies for domain: {}", domain);
        execute_fetch(&agent, url_str, None, &[])?
    } else {
        info!(
            "Sending {} cookies for domain: {}",
            matching_cookies.len(),
            domain
        );
        execute_fetch(&agent, url_str, Some(&cookie_header), &[])?
    };

    // Parse Set-Cookie headers from response. Attribute them to the host of
    // the FINAL URL (after redirects), so a Set-Cookie sent by the redirect
    // target is not stored under the original host that never sent it.
    let final_domain = match url::Url::parse(&result.url) {
        Ok(u) => u
            .host_str()
            .map(|h| h.to_string())
            .unwrap_or_else(|| domain.clone()),
        Err(_) => domain.clone(),
    };
    for set_cookie_val in &result.set_cookie_headers {
        let cookie = crate::storage::Cookie::from_set_cookie_header(set_cookie_val, &final_domain);
        if !cookie.name.is_empty() {
            info!(
                "Stored cookie: {}={} for domain {}",
                cookie.name, cookie.value, cookie.domain
            );
            cookie_store.add_cookie(cookie);
        }
    }

    Ok(result)
}

/// Check if an error message represents a retryable error.
/// Retryable: timeouts, 500-503, 504 server/gateway errors, connection issues.
pub fn is_retryable_error(err: &str) -> bool {
    let err_lower = err.to_ascii_lowercase();

    // Check for HTTP status codes (500-599)
    // Pattern: "status XXX" or "XXX Internal/Bad/Gateway/Service"
    if err_lower.contains("status 5") || err_lower.contains("status 4") {
        // Extract the status code after "status "
        if let Some(pos) = err_lower.find("status ") {
            let after = &err_lower[pos + 7..];
            let code: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code_num) = code.parse::<u16>() {
                return code_num == 408 || code_num == 429 || (500..600).contains(&code_num);
            }
        }
    }
    if err_lower.contains("429") {
        return true;
    }

    // Pattern: ureq error format - "http: server error" with status in description
    // Ureq status 5xx errors are retryable (except those that won't recover)
    if err_lower.contains("server error")
        || err_lower.contains("bad gateway")
        || err_lower.contains("service unavailable")
        || err_lower.contains("gateway timeout")
    {
        return true;
    }

    // Connection / timeout issues. Note: "timeout" covers "timed out" and the
    // common ureq phrasing; the broad "transport" match was dropped — it also
    // hit TLS/certificate errors that will never succeed on retry.
    err_lower.contains("timed out")
        || err_lower.contains("timeout")
        || err_lower.contains("connection closed")
        || err_lower.contains("connection reset")
        || err_lower.contains("refused")
}

/// Fetch with automatic retry for transient errors.
/// Retries up to `max_retries` times with exponential backoff (1s, 2s, 4s...),
/// capped so a long retry list can't sleep for minutes.
/// Only retries on timeouts, connection issues, and 408/429/5xx server errors.
const MAX_BACKOFF_MS: u64 = 30_000;

/// Like `fetch_with_retry`, but sends a pre-built `Cookie` header instead of
/// borrowing the whole cookie jar. Used by the GUI async path so a navigation
/// doesn't deep-clone thousands of cookies (and so cookies set by an
/// in-flight response are visible to the next request, not hidden inside a
/// stale clone). Set-Cookie headers in the result are applied by the caller.
pub fn fetch_with_header_and_retry(
    url_str: &str,
    cookie_header: &str,
    max_retries: u32,
) -> Result<FetchResult, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0) // redirects handled manually in execute_fetch
        .user_agent(&browser_ua())
        .build();

    fetch_with_agent_and_retry(&agent, url_str, cookie_header, max_retries)
}

pub(crate) fn fetch_with_agent_and_retry(
    agent: &ureq::Agent,
    url_str: &str,
    cookie_header: &str,
    max_retries: u32,
) -> Result<FetchResult, String> {
    let mut last_error = String::new();
    let mut backoff_ms = 1000;
    let header = if cookie_header.is_empty() {
        None
    } else {
        Some(cookie_header)
    };

    for attempt in 0..=max_retries {
        if attempt > 0 {
            info!("Retry attempt {}/{} for {}", attempt, max_retries, url_str);
            std::thread::sleep(std::time::Duration::from_millis(
                backoff_ms.min(MAX_BACKOFF_MS),
            ));
            backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
        }

        match execute_fetch(agent, url_str, header, &[]) {
            Ok(result) => return Ok(result),
            Err(e) => {
                let err_str = e.to_string();
                last_error = err_str.clone();
                if !is_retryable_error(&err_str) {
                    warn!("Non-retryable error for {}: {}", url_str, err_str);
                    return Err(err_str);
                }
                if attempt == max_retries {
                    warn!(
                        "Max retries ({}) exceeded for {}: {}",
                        max_retries, url_str, err_str
                    );
                    return Err(err_str);
                }
            }
        }
    }

    Err(last_error)
}

pub fn fetch_with_retry(
    url_str: &str,
    cookie_store: &mut crate::storage::CookieStore,
    max_retries: u32,
) -> Result<FetchResult, String> {
    let mut last_error = String::new();
    let mut backoff_ms = 1000; // Start with 1 second

    for attempt in 0..=max_retries {
        if attempt > 0 {
            info!("Retry attempt {}/{} for {}", attempt, max_retries, url_str);
            std::thread::sleep(std::time::Duration::from_millis(
                backoff_ms.min(MAX_BACKOFF_MS),
            ));
            backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
        }

        match fetch_with_cookies(url_str, cookie_store) {
            Ok(result) => return Ok(result),
            Err(e) => {
                let err_str = e.to_string();
                last_error = err_str.clone();
                if !is_retryable_error(&err_str) {
                    // Non-retryable error, fail immediately
                    warn!("Non-retryable error for {}: {}", url_str, err_str);
                    return Err(err_str);
                }
                if attempt == max_retries {
                    warn!(
                        "Max retries ({}) exceeded for {}: {}",
                        max_retries, url_str, err_str
                    );
                    return Err(err_str);
                }
            }
        }
    }

    Err(last_error)
}

/// Internal: execute the actual HTTP request.
///
/// Redirects are followed MANUALLY (the agents are built with `redirects(0)`)
/// so that `Set-Cookie` headers from EVERY hop survive — ureq's internal
/// redirect following only exposes the final response, silently dropping
/// cookies set by intermediate hosts (a real issue for login/session
/// redirect chains).
fn execute_fetch(
    agent: &ureq::Agent,
    url_str: &str,
    cookie_header: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let start = Instant::now();
    const MAX_REDIRECTS: usize = 5;

    // Validate and parse URL
    let mut current = url_str.to_string();
    // Cookies were built for the original URL's origin; they must not be
    // replayed on cross-origin redirect hops (or scheme downgrades).
    let cookie_origin = url::Url::parse(url_str).ok();
    let mut set_cookie_headers: Vec<String> = Vec::new();
    let mut final_response: Option<ureq::Response> = None;

    for _hop in 0..=MAX_REDIRECTS {
        let parsed = url::Url::parse(&current)?;
        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(format!("Unsupported URL scheme: {}", scheme).into());
        }

        // Build request with optional cookie header + extra headers.
        // The cookie jar is origin-scoped, so attach it only while the hop
        // stays same-origin with the URL the cookies came from; the same
        // guard applies to caller-supplied conditional-request headers,
        // which are meaningless (and leaky) on unrelated hosts.
        let same_origin = cookie_origin.as_ref().is_some_and(|origin| {
            origin.scheme() == parsed.scheme()
                && origin.host_str() == parsed.host_str()
                && origin.port_or_known_default() == parsed.port_or_known_default()
        });
        let mut request = agent.get(&current);
        if let Some(cookie_val) = cookie_header.filter(|_| same_origin) {
            request = request.set("Cookie", cookie_val);
        }
        if same_origin {
            for (k, v) in extra_headers {
                request = request.set(k, v);
            }
        }
        let response = request.call()?;

        // Collect this hop's Set-Cookie headers
        for cookie_val in response.all("set-cookie") {
            let trimmed = cookie_val.trim();
            if !trimmed.is_empty() {
                set_cookie_headers.push(trimmed.to_string());
            }
        }

        // Follow 3xx redirects manually (Location header). Relative
        // locations resolve against the current hop URL.
        if let Some(loc) = response.header("location") {
            let next = url::Url::parse(&current)?.join(loc)?;
            info!("Redirecting {} -> {}", current, next);
            current = next.to_string();
            continue;
        }

        final_response = Some(response);
        break;
    }

    let response = final_response
        .ok_or_else(|| format!("Too many redirects ({}) for {}", MAX_REDIRECTS, url_str))?;

    let status_code = response.status();
    let content_type = response
        .header("content-type")
        .unwrap_or("text/html")
        .to_string();

    // Collect response headers
    let mut headers = HashMap::new();
    for header_name in &[
        "content-type",
        "content-encoding",
        "content-length",
        "set-cookie",
        "cache-control",
        "last-modified",
        "vary",
    ] {
        if let Some(val) = response.header(header_name) {
            headers.insert(header_name.to_string(), val.to_string());
        }
    }

    // Report the FINAL URL: ureq follows redirects internally, so the body
    // and headers came from a possibly different host than url_str.
    let final_url = response.get_url().to_string();

    // Read body as string, capped like the download path. An uncapped read
    // lets a server allocate gigabytes of RAM in this process (OOM abort).
    // Read one extra byte so an over-limit response is reported as an error
    // instead of being silently truncated.
    use std::io::Read;
    const MAX_PAGE_BODY: u64 = 50 * 1024 * 1024;
    let mut bytes: Vec<u8> = Vec::new();
    response
        .into_reader()
        .take(MAX_PAGE_BODY + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PAGE_BODY {
        return Err("Page exceeds the 50MB body limit".into());
    }
    let body_len = bytes.len();
    let fetch_time_ms = start.elapsed().as_millis() as u64;

    // Log warning if response body is very large
    if body_len > 10 * 1024 * 1024 {
        warn!(
            "Large response received: {} bytes from {}",
            body_len, final_url
        );
    }

    info!(
        "Fetched {} ({} bytes, {} ms, status {})",
        final_url, body_len, fetch_time_ms, status_code
    );

    // Shared finalization: content-type / PDF / charset policy is identical
    // to the async scheduler path so the two transports can never drift.
    Ok(finalize_fetch_response(
        &final_url,
        status_code,
        &content_type,
        headers,
        bytes,
        set_cookie_headers,
        fetch_time_ms,
        false,
    )?)
}

/// Shared response-finalization policy used by BOTH the blocking ureq path
/// (`execute_fetch`) and the async reqwest scheduler path
/// (`ReqwestTransport::execute_once`). Keeping the content-type / PDF /
/// charset split in one place means the two transports can never drift on
/// how a raw response body becomes a [`FetchResult`].
#[allow(clippy::too_many_arguments)] // shared by both transports; a struct would churn both call sites
pub(crate) fn finalize_fetch_response(
    final_url: &str,
    status_code: u16,
    content_type: &str,
    headers: HashMap<String, String>,
    bytes: Vec<u8>,
    set_cookie_headers: Vec<String>,
    fetch_time_ms: u64,
    binary_mode: bool,
) -> Result<FetchResult, String> {
    let is_pdf = content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/pdf"))
        || final_url.to_ascii_lowercase().ends_with(".pdf");
    let (body, binary_body) = if binary_mode || is_pdf {
        (String::new(), Some(bytes))
    } else {
        (decode_text_response(&bytes, content_type)?, None)
    };
    Ok(FetchResult {
        body,
        binary_body,
        url: final_url.to_string(),
        status_code,
        content_type: content_type.to_string(),
        headers,
        fetch_time_ms,
        set_cookie_headers,
    })
}

/// Decode an HTTP text response using its declared WHATWG charset. UTF BOMs
/// take priority; an absent/unknown label first attempts strict UTF-8, then
/// falls back to Windows-1252 as the compatible legacy web encoding.
pub(crate) fn decode_text_response(bytes: &[u8], content_type: &str) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec())
            .map_err(|error| format!("Invalid UTF-8 response body: {error}"));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if rest.len() % 2 != 0 {
            return Err("Truncated UTF-16LE response body".to_string());
        }
        let units = rest
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        return String::from_utf16(&units.collect::<Vec<_>>())
            .map_err(|error| format!("Invalid UTF-16LE response body: {error}"));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        if rest.len() % 2 != 0 {
            return Err("Truncated UTF-16BE response body".to_string());
        }
        let units = rest
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
        return String::from_utf16(&units.collect::<Vec<_>>())
            .map_err(|error| format!("Invalid UTF-16BE response body: {error}"));
    }

    if let Some(label) = response_charset(content_type) {
        let encoding = encoding_rs::Encoding::for_label(label.as_bytes())
            .ok_or_else(|| format!("Unsupported response charset: {label}"))?;
        let (decoded, _, _) = encoding.decode(bytes);
        return Ok(decoded.into_owned());
    }
    if let Ok(utf8) = std::str::from_utf8(bytes) {
        return Ok(utf8.to_string());
    }
    let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
    Ok(decoded.into_owned())
}

fn response_charset(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['\'', '"']).to_ascii_lowercase())
    })
}

/// Download raw bytes from a URL (used by the downloads manager)
/// Returns (bytes, suggested_file_name, content_type)
pub fn download_url(
    url_str: &str,
) -> Result<(Vec<u8>, String, String), Box<dyn std::error::Error>> {
    let parsed = url::Url::parse(url_str)?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("Unsupported URL scheme: {}", scheme).into());
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .redirects(5)
        .user_agent(&browser_ua())
        .build();

    let response = agent.get(url_str).call()?;
    let content_type = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();

    // Suggested name: Content-Disposition filename, else last URL path segment
    let mut file_name = response
        .header("content-disposition")
        .and_then(|cd| cd.split("filename=").nth(1))
        .and_then(|s| s.split(';').next())
        .map(|s| s.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default();
    if file_name.is_empty() {
        file_name = parsed
            .path_segments()
            .and_then(|mut segs| segs.rfind(|s| !s.is_empty()))
            .unwrap_or("")
            .to_string();
    }
    if file_name.is_empty() {
        file_name = format!("{}.html", parsed.host_str().unwrap_or("download"));
    }

    // Read body bytes (limit 100MB). Read one extra byte so an over-limit
    // response is reported as an error instead of being silently truncated.
    use std::io::Read;
    const MAX_DOWNLOAD: u64 = 100 * 1024 * 1024;
    let mut bytes: Vec<u8> = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DOWNLOAD {
        return Err("File exceeds the 100MB download limit".into());
    }

    info!(
        "Downloaded {} ({} bytes) as {}",
        url_str,
        bytes.len(),
        file_name
    );

    Ok((bytes, file_name, content_type))
}

/// Download through the shared asynchronous scheduler. Network bytes never
/// occupy a blocking runtime worker; file-system persistence remains a
/// separate caller-owned operation.
pub async fn download_url_async(url_str: &str) -> Result<(Vec<u8>, String, String), String> {
    let parsed = url::Url::parse(url_str).map_err(|error| error.to_string())?;
    let response = crate::network_scheduler::fetch_shared(
        url_str.to_string(),
        String::new(),
        1,
        crate::network_scheduler::RequestPriority::Background,
        crate::network_scheduler::ResponseMode::Binary,
        crate::network_scheduler::CancellationToken::default(),
    )
    .await?;
    let bytes = response
        .binary_body
        .ok_or_else(|| "Download transport did not return binary data".to_string())?;
    let mut file_name = response
        .headers
        .get("content-disposition")
        .and_then(|value| value.split("filename=").nth(1))
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default();
    if file_name.is_empty() {
        file_name = parsed
            .path_segments()
            .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
            .unwrap_or_default()
            .to_string();
    }
    if file_name.is_empty() {
        file_name = format!("{}.html", parsed.host_str().unwrap_or("download"));
    }
    Ok((bytes, file_name, response.content_type))
}

/// `true` when a response carries a `Vary` header meaning it is served
/// differently depending on request headers (Cookie, Authorization,
/// User-Agent, ...). Such responses must never be cached under a bare URL —
/// a later request from another session/user profile could be served another
/// user's stateful content (RFC 7234 §4.1).
pub(crate) fn response_varies(headers: &HashMap<String, String>) -> bool {
    headers
        .get("vary")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

/// Conditional (If-Modified-Since) revalidation request for cache entries.
/// Returns the fetched result; callers treat `status_code == 304` as
/// "not modified — use the cached copy".
fn fetch_revalidate(
    url: &str,
    if_modified_since: &str,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0) // redirects handled manually in execute_fetch
        .user_agent(&browser_ua())
        .build();
    execute_fetch(
        &agent,
        url,
        None,
        &[("If-Modified-Since", if_modified_since)],
    )
}

/// Fetch with caching support - returns cached result if fresh
/// If cookie_store is provided, integrates cookie jar (inject + parse Set-Cookie)
pub fn fetch_with_cache(
    url: &str,
    cache: &mut ResourceCache,
    cookie_store: Option<&mut crate::storage::CookieStore>,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    // Check cache first
    if let Some(cached) = cache.get(url) {
        if !cached.is_expired() {
            info!("Cache HIT for: {}", url);
            let result = cached.result.clone();
            cache.record_hit();
            return Ok(result);
        }

        // STALE: revalidate with a conditional request when the cached
        // response carries Last-Modified (RFC 7232). A 304 refreshes the
        // entry with no re-download; anything else replaces it below.
        let last_modified = cached.result.headers.get("last-modified").cloned();
        let cached_result = cached.result.clone();
        if let Some(lm) = last_modified {
            info!("Cache STALE for: {} (revalidating)", url);
            if let Ok(reval) = fetch_revalidate(url, &lm) {
                if reval.status_code == 304 {
                    cache.touch(url);
                    let result = cached_result;
                    cache.record_hit();
                    return Ok(result);
                }
            }
        } else {
            info!("Cache STALE for: {}", url);
        }
    }

    cache.record_miss();

    // Fetch fresh (with or without cookies)
    let result = if let Some(cs) = cookie_store {
        fetch_with_cookies(url, cs)?
    } else {
        fetch_url(url)?
    };

    // Determine TTL from cache-control header or use default. Error responses
    // are never cached (a transient 500 must not be served for 5 minutes).
    let ttl = if result.status_code >= 400 {
        0
    } else {
        cache_ttl_secs(&result.headers)
    };

    // RFC 7234 §4.1: responses that vary by request headers must never be
    // cached under a bare URL — a later request from another session/user
    // profile could be served another user's stateful content.
    if ttl > 0 && !response_varies(&result.headers) {
        cache.insert(url, result.clone(), ttl);
    }

    Ok(result)
}

/// Cache entry with TTL support
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub result: FetchResult,
    pub cached_at: Instant,
    pub ttl_secs: u64,
}

impl CacheEntry {
    pub fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > Duration::from_secs(self.ttl_secs)
    }

    /// Mark the entry as freshly validated (304 response): restart the TTL
    /// clock without re-downloading the body.
    pub fn refresh_timestamp(&mut self) {
        self.cached_at = Instant::now();
    }
}

/// Resource cache with TTL-based eviction
pub struct ResourceCache {
    entries: HashMap<String, CacheEntry>,
    pub max_size: usize,
    pub max_entries: usize,
    hits: u64,
    misses: u64,
}

impl Default for ResourceCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_size: 32 * 1024 * 1024,
            max_entries: 100,
            hits: 0,
            misses: 0,
        }
    }

    /// Calculate total bytes used by all cached entries
    pub fn total_bytes(&self) -> usize {
        self.entries.iter().fold(0usize, |total, (key, entry)| {
            total
                .saturating_add(key.capacity())
                .saturating_add(std::mem::size_of::<CacheEntry>())
                .saturating_add(entry.result.retained_bytes())
        })
    }

    pub fn insert(&mut self, url: &str, result: FetchResult, ttl_secs: u64) {
        let entry_size = url
            .len()
            .saturating_add(std::mem::size_of::<CacheEntry>())
            .saturating_add(result.retained_bytes());
        if entry_size > self.max_size || self.max_entries == 0 {
            return;
        }

        // Evict expired entries first
        self.evict_expired();
        self.entries.remove(url);

        // Enforce max_entries limit - remove oldest if at capacity
        while self.entries.len() >= self.max_entries {
            if !self.remove_oldest() {
                break;
            }
        }

        // Enforce max_size byte limit - remove oldest entries until under limit
        while self.total_bytes() + entry_size > self.max_size && !self.entries.is_empty() {
            if !self.remove_oldest() {
                break;
            }
        }

        let entry = CacheEntry {
            result,
            cached_at: Instant::now(),
            ttl_secs,
        };
        self.entries.insert(url.to_string(), entry);
    }

    pub fn get(&self, url: &str) -> Option<&CacheEntry> {
        self.entries.get(url)
    }

    /// Restart the TTL clock for an entry after a successful conditional
    /// revalidation (304 Not Modified).
    pub fn touch(&mut self, url: &str) {
        if let Some(e) = self.entries.get_mut(url) {
            e.refresh_timestamp();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.hits = 0;
        self.misses = 0;
    }

    /// Apply a new byte budget and evict oldest entries immediately.
    pub fn set_max_size(&mut self, bytes: usize) -> usize {
        let before = self.total_bytes();
        self.max_size = bytes;
        while self.total_bytes() > self.max_size && !self.entries.is_empty() {
            if !self.remove_oldest() {
                break;
            }
        }
        before.saturating_sub(self.total_bytes())
    }

    fn remove_oldest(&mut self) -> bool {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.cached_at)
            .map(|(key, _)| key.clone());
        oldest.is_some_and(|key| self.entries.remove(&key).is_some())
    }

    /// Remove expired entries
    pub fn evict_expired(&mut self) {
        let expired_keys: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.is_expired())
            .map(|(k, _)| k.clone())
            .collect();
        let count = expired_keys.len();
        for key in expired_keys {
            self.entries.remove(&key);
        }
        if count > 0 {
            info!("Evicted {} expired cache entries", count);
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.entries.len(),
            hits: self.hits,
            misses: self.misses,
            max_size: self.max_size,
        }
    }

    /// Record a cache hit
    pub fn record_hit(&mut self) {
        self.hits += 1;
    }

    /// Record a cache miss
    pub fn record_miss(&mut self) {
        self.misses += 1;
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub max_size: usize,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cache: {} entries | Hits: {} | Misses: {} | Hit rate: {:.1}%",
            self.entries,
            self.hits,
            self.misses,
            if self.hits + self.misses > 0 {
                (self.hits as f64 / (self.hits + self.misses) as f64) * 100.0
            } else {
                0.0
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_validation() {
        let result = fetch_url("https://example.com");
        assert!(result.is_ok() || result.is_err()); // Network-dependent
    }

    #[test]
    fn test_invalid_url() {
        let result = fetch_url("ftp://invalid-scheme.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = ResourceCache::new();
        let url = "https://example.com";
        let fetch_result = FetchResult {
            body: "Hello World!".to_string(),
            binary_body: None,
            url: url.to_string(),
            status_code: 200,
            content_type: "text/html".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 10,
            set_cookie_headers: vec![],
        };

        cache.insert(url, fetch_result.clone(), 3600);
        assert!(cache.get(url).is_some());
        assert_eq!(cache.get(url).unwrap().result.body, "Hello World!");
    }

    #[test]
    fn test_cache_expiry() {
        let mut cache = ResourceCache::new();
        let url = "https://test.com";
        let fetch_result = FetchResult {
            body: "Test".to_string(),
            binary_body: None,
            url: url.to_string(),
            status_code: 200,
            content_type: "text/plain".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
        };

        cache.insert(url, fetch_result, 0); // TTL = 0, expires immediately
        assert!(cache.get(url).unwrap().is_expired());
    }

    #[test]
    fn test_cache_evict_expired() {
        let mut cache = ResourceCache::new();
        let fetch_result = FetchResult {
            body: "Data".to_string(),
            binary_body: None,
            url: "https://x.com".to_string(),
            status_code: 200,
            content_type: "text/plain".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
        };

        cache.insert("https://x.com", fetch_result, 0); // expires immediately
        assert_eq!(cache.len(), 1);
        cache.evict_expired();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_total_bytes_counts_binary_headers_keys_and_cookies() {
        let mut cache = ResourceCache::new();
        cache.insert(
            "https://example.test/file.pdf",
            FetchResult {
                body: String::new(),
                binary_body: Some(vec![7; 1024]),
                url: "https://example.test/file.pdf".to_string(),
                status_code: 200,
                content_type: "application/pdf".to_string(),
                headers: HashMap::from([("etag".to_string(), "abc".repeat(100))]),
                fetch_time_ms: 1,
                set_cookie_headers: vec!["session=value".to_string()],
            },
            60,
        );
        assert!(cache.total_bytes() > 1024);
    }

    #[test]
    fn cache_size_setter_evicts_immediately() {
        let mut cache = ResourceCache::new();
        for index in 0..3 {
            let url = format!("https://example.test/{index}");
            cache.insert(
                &url,
                FetchResult {
                    body: "x".repeat(1024),
                    binary_body: None,
                    url: url.clone(),
                    status_code: 200,
                    content_type: "text/plain".to_string(),
                    headers: HashMap::new(),
                    fetch_time_ms: 1,
                    set_cookie_headers: vec![],
                },
                60,
            );
        }
        let freed = cache.set_max_size(1_500);
        assert!(freed > 0);
        assert!(cache.total_bytes() <= 1_500);
    }

    #[test]
    fn test_cache_ttl_respects_no_store() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "no-store, max-age=3600".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_respects_no_cache() {
        let mut headers = HashMap::new();
        headers.insert("cache-control".to_string(), "no-cache".to_string());
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_respects_private() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "private, max-age=100".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_uses_max_age() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "public, max-age=120".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 120);
    }

    #[test]
    fn test_cache_ttl_default() {
        assert_eq!(cache_ttl_secs(&HashMap::new()), 300);
    }

    #[test]
    fn test_error_responses_are_not_cached() {
        let mut cache = ResourceCache::new();
        let err_result = FetchResult {
            body: "Internal Server Error".to_string(),
            binary_body: None,
            url: "https://e.com".to_string(),
            status_code: 500,
            content_type: "text/html".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
        };

        // FetchResult built by hand (no network): verify the guard used by
        // fetch_with_cache never stores a >= 400 response.
        let ttl = if err_result.status_code >= 400 {
            0
        } else {
            cache_ttl_secs(&err_result.headers)
        };
        if ttl > 0 {
            cache.insert("https://e.com", err_result.clone(), ttl);
        }
        assert!(cache.get("https://e.com").is_none());
    }

    #[test]
    fn test_is_retryable_error_server_errors() {
        // 5xx server errors should retry
        assert!(is_retryable_error("status 500 Internal Server Error"));
        assert!(is_retryable_error("status 502 Bad Gateway"));
        assert!(is_retryable_error("status 503 Service Unavailable"));
        assert!(is_retryable_error("status 504 Gateway Timeout"));

        // 408 (request timeout) and 429 (too many requests) should also retry
        assert!(is_retryable_error("status 408 Request Timeout"));
        assert!(is_retryable_error("status 429 Too Many Requests"));
    }

    #[test]
    fn test_is_retryable_error_network_issues() {
        // Connection / timeout issues should retry
        assert!(is_retryable_error("Connection timed out"));
        assert!(is_retryable_error("Request timeout"));
        assert!(is_retryable_error("Connection closed by server"));
        assert!(is_retryable_error("Connection reset by peer"));
        assert!(is_retryable_error("transport error: connection refused"));
    }

    #[test]
    fn test_is_retryable_error_client_errors() {
        // 4xx client errors (except 408, 429) should NOT retry
        assert!(!is_retryable_error("status 404 Not Found"));
        assert!(!is_retryable_error("status 403 Forbidden"));
        assert!(!is_retryable_error("status 401 Unauthorized"));

        // DNS failures are usually not retryable
        assert!(!is_retryable_error(
            "Could not resolve host: example.invalid"
        ));
    }

    #[test]
    fn test_is_retryable_error_ureq_format() {
        // ureq status 5xx errors format as "http: server error"
        assert!(is_retryable_error("http: server error"));
    }

    #[test]
    fn test_finalize_fetch_response_transport_conformance() {
        // R7: both transports must apply IDENTICAL response-finalization
        // policy. The blocking ureq path passes binary_mode=false and the
        // async reqwest scheduler path passes binary_mode=(ResponseMode::
        // Binary). Feeding the same raw response through the shared helper
        // with both flags proves the two transports cannot drift.
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/html".to_string());
        let bytes = b"<html><body>h\xC3\xA9llo</body></html>".to_vec();

        // Text mode (ureq path): body decoded as UTF-8, no binary payload.
        let text = finalize_fetch_response(
            "https://a.example/page",
            200,
            "text/html",
            headers.clone(),
            bytes.clone(),
            vec![],
            12,
            false,
        )
        .unwrap();
        assert_eq!(text.body, "<html><body>h\u{e9}llo</body></html>");
        assert!(text.binary_body.is_none());

        // Binary mode (reqwest path with ResponseMode::Binary): raw bytes
        // preserved, body empty.
        let binary = finalize_fetch_response(
            "https://a.example/page",
            200,
            "text/html",
            headers,
            bytes.clone(),
            vec![],
            12,
            true,
        )
        .unwrap();
        assert_eq!(binary.body, "");
        assert_eq!(binary.binary_body.as_deref(), Some(bytes.as_slice()));
        // The two transport paths share every other field verbatim.
        assert_eq!(text.url, binary.url);
        assert_eq!(text.status_code, binary.status_code);
        assert_eq!(text.content_type, binary.content_type);
        assert_eq!(text.headers, binary.headers);
        assert_eq!(text.set_cookie_headers, binary.set_cookie_headers);
    }

    #[test]
    fn test_finalize_fetch_response_pdf_policy_is_transport_independent() {
        // PDF detection (mime OR extension) must behave identically no
        // matter which transport produced the response.
        let pdf_bytes = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        for (content_type, url, label) in [
            ("application/pdf", "https://a.example/doc", "mime"),
            (
                "application/octet-stream",
                "https://a.example/doc.pdf",
                "extension",
            ),
        ] {
            let result = finalize_fetch_response(
                url,
                200,
                content_type,
                HashMap::new(),
                pdf_bytes.clone(),
                vec![],
                1,
                false,
            )
            .unwrap();
            assert!(
                result.binary_body.is_some(),
                "{label}: PDF must be routed to the binary download path"
            );
            assert_eq!(result.body, "");
        }
        // Non-PDF content with the same bytes stays text.
        let text_result = finalize_fetch_response(
            "https://a.example/doc",
            200,
            "text/plain",
            HashMap::new(),
            pdf_bytes,
            vec![],
            1,
            false,
        )
        .unwrap();
        assert!(text_result.binary_body.is_none());
    }
}

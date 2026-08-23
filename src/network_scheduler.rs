//! Bounded asynchronous scheduling for browser network work.
//!
//! Protocol implementations are adapters. Scheduling, priority, cancellation,
//! timeouts and response budgets remain browser-owned Rust policy.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use std::{future::Future, pin::Pin};

use tokio::sync::{Notify, Semaphore};

use crate::network::FetchResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestPriority {
    Background = 0,
    Image = 1,
    Media = 2,
    ScriptStyle = 3,
    Navigation = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseMode {
    Document,
    Binary,
}

#[derive(Debug, Clone)]
pub struct ScheduledRequest {
    pub id: u64,
    pub url: String,
    pub cookie_header: String,
    pub max_retries: u32,
    pub priority: RequestPriority,
    pub response_mode: ResponseMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduledError {
    Cancelled,
    QueueFull,
    Timeout,
    InvalidRequest(String),
    Transport(String),
    ResponseBudgetExceeded,
    WorkerFailure,
}

#[derive(Debug, Clone)]
pub struct ScheduledResponse {
    pub request_id: u64,
    pub result: Result<FetchResult, ScheduledError>,
}

#[derive(Debug, Clone)]
pub struct SchedulerLimits {
    pub max_concurrency: usize,
    pub max_queued: usize,
    pub max_response_bytes: usize,
    pub request_timeout: Duration,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            max_concurrency: 8,
            max_queued: 2_048,
            max_response_bytes: 50 * 1024 * 1024,
            request_timeout: Duration::from_secs(45),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    notification: Arc<Notify>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notification.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.notification.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

pub trait NetworkTransport: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        request: &'a ScheduledRequest,
        cancellation: &'a CancellationToken,
        max_response_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<FetchResult, String>> + Send + 'a>>;
}

#[derive(Debug)]
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(30))
                .redirects(0)
                .user_agent(&crate::network::browser_ua())
                .build(),
        }
    }
}

impl NetworkTransport for UreqTransport {
    fn execute<'a>(
        &'a self,
        request: &'a ScheduledRequest,
        cancellation: &'a CancellationToken,
        _max_response_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<FetchResult, String>> + Send + 'a>> {
        let agent = self.agent.clone();
        let request = request.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err("request cancelled before transport start".to_string());
            }
            tokio::task::spawn_blocking(move || {
                crate::network::fetch_with_agent_and_retry(
                    &agent,
                    &request.url,
                    &request.cookie_header,
                    request.max_retries,
                )
            })
            .await
            .map_err(|error| format!("blocking compatibility transport failed: {error}"))?
        })
    }
}

/// Production asynchronous HTTP transport. Dropping its request/body future
/// closes the in-flight stream, so scheduler cancellation reaches the socket
/// instead of merely ignoring a completed blocking request.
#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .user_agent(crate::network::browser_ua())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(8)
            .http2_adaptive_window(true)
            .build()
            .map_err(|error| format!("cannot initialize async HTTP transport: {error}"))?;
        Ok(Self { client })
    }

    async fn execute_once(
        &self,
        request: &ScheduledRequest,
        cancellation: &CancellationToken,
        max_response_bytes: usize,
    ) -> Result<FetchResult, String> {
        const MAX_REDIRECTS: usize = 5;
        let started = std::time::Instant::now();
        let original = url::Url::parse(&request.url).map_err(|error| error.to_string())?;
        let mut current = original.clone();
        let mut set_cookie_headers = Vec::new();

        for hop in 0..=MAX_REDIRECTS {
            let mut builder = self.client.get(current.clone());
            if !request.cookie_header.is_empty() && same_origin(&original, &current) {
                builder = builder.header(reqwest::header::COOKIE, &request.cookie_header);
            }
            let mut response = tokio::select! {
                _ = cancellation.cancelled() => return Err("request cancelled".to_string()),
                response = builder.send() => response.map_err(|error| error.to_string())?,
            };

            for value in response.headers().get_all(reqwest::header::SET_COOKIE) {
                if let Ok(value) = value.to_str() {
                    if !value.trim().is_empty() {
                        set_cookie_headers.push(value.trim().to_string());
                    }
                }
            }

            if response.status().is_redirection() {
                let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
                    return Err(format!(
                        "redirect status {} did not include Location",
                        response.status()
                    ));
                };
                if hop == MAX_REDIRECTS {
                    return Err(format!("too many redirects for {}", request.url));
                }
                let location = location
                    .to_str()
                    .map_err(|_| "redirect Location is not valid text".to_string())?;
                current = current.join(location).map_err(|error| error.to_string())?;
                continue;
            }

            let status_code = response.status().as_u16();
            if status_code == 408 || status_code == 429 || status_code >= 500 {
                return Err(format!("status {status_code}"));
            }
            if status_code >= 400 {
                return Err(format!("status {status_code}"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > max_response_bytes as u64)
            {
                return Err("response body exceeds scheduler budget".to_string());
            }

            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("text/html")
                .to_string();
            let mut headers = std::collections::HashMap::new();
            for (name, value) in response.headers() {
                if name != reqwest::header::SET_COOKIE {
                    if let Ok(value) = value.to_str() {
                        headers.insert(name.as_str().to_ascii_lowercase(), value.to_string());
                    }
                }
            }
            let final_url = response.url().to_string();
            let mut bytes = Vec::new();
            loop {
                let chunk = tokio::select! {
                    _ = cancellation.cancelled() => return Err("request cancelled".to_string()),
                    chunk = response.chunk() => chunk.map_err(|error| error.to_string())?,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                if bytes.len().saturating_add(chunk.len()) > max_response_bytes {
                    return Err("response body exceeds scheduler budget".to_string());
                }
                bytes.extend_from_slice(&chunk);
            }
            // Shared finalization: content-type / PDF / charset policy is
            // identical to the blocking ureq path so the two transports can
            // never drift on how a body becomes a FetchResult.
            return crate::network::finalize_fetch_response(
                &final_url,
                status_code,
                &content_type,
                headers,
                bytes,
                set_cookie_headers,
                started.elapsed().as_millis() as u64,
                request.response_mode == ResponseMode::Binary,
            );
        }
        Err(format!("too many redirects for {}", request.url))
    }
}

impl NetworkTransport for ReqwestTransport {
    fn execute<'a>(
        &'a self,
        request: &'a ScheduledRequest,
        cancellation: &'a CancellationToken,
        max_response_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<FetchResult, String>> + Send + 'a>> {
        Box::pin(async move {
            let mut backoff_ms = 1_000_u64;
            let mut last_error = String::new();
            for attempt in 0..=request.max_retries {
                if attempt > 0 {
                    tokio::select! {
                        _ = cancellation.cancelled() => return Err("request cancelled".to_string()),
                        _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                    }
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
                match self
                    .execute_once(request, cancellation, max_response_bytes)
                    .await
                {
                    Ok(result) => return Ok(result),
                    Err(error) if cancellation.is_cancelled() => return Err(error),
                    Err(error) => {
                        let retryable = crate::network::is_retryable_error(&error);
                        last_error = error;
                        if !retryable {
                            break;
                        }
                    }
                }
            }
            Err(last_error)
        })
    }
}

fn same_origin(left: &url::Url, right: &url::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

pub struct NetworkScheduler<T: NetworkTransport> {
    transport: Arc<T>,
    limits: SchedulerLimits,
    permits: Arc<Semaphore>,
    queued: Arc<AtomicUsize>,
}

impl<T: NetworkTransport> Clone for NetworkScheduler<T> {
    fn clone(&self) -> Self {
        Self {
            transport: Arc::clone(&self.transport),
            limits: self.limits.clone(),
            permits: Arc::clone(&self.permits),
            queued: Arc::clone(&self.queued),
        }
    }
}

impl<T: NetworkTransport> NetworkScheduler<T> {
    pub fn new(transport: T, limits: SchedulerLimits) -> Result<Self, String> {
        if limits.max_concurrency == 0
            || limits.max_queued == 0
            || limits.max_response_bytes == 0
            || limits.request_timeout.is_zero()
        {
            return Err("Network scheduler limits must be non-zero".to_string());
        }
        Ok(Self {
            transport: Arc::new(transport),
            permits: Arc::new(Semaphore::new(limits.max_concurrency)),
            queued: Arc::new(AtomicUsize::new(0)),
            limits,
        })
    }

    pub fn queued_len(&self) -> usize {
        self.queued.load(Ordering::Acquire)
    }

    pub async fn fetch(
        &self,
        request: ScheduledRequest,
        cancellation: CancellationToken,
    ) -> ScheduledResponse {
        let request_id = request.id;
        let parsed = url::Url::parse(&request.url);
        if !matches!(
            parsed.as_ref().map(|url| url.scheme()),
            Ok("http" | "https")
        ) {
            return ScheduledResponse {
                request_id,
                result: Err(ScheduledError::InvalidRequest(request.url)),
            };
        }
        if cancellation.is_cancelled() {
            return ScheduledResponse {
                request_id,
                result: Err(ScheduledError::Cancelled),
            };
        }
        let queued_before = self.queued.fetch_add(1, Ordering::AcqRel);
        if queued_before >= self.limits.max_queued {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            return ScheduledResponse {
                request_id,
                result: Err(ScheduledError::QueueFull),
            };
        }

        let permit = match tokio::time::timeout(
            self.limits.request_timeout,
            Arc::clone(&self.permits).acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => {
                self.queued.fetch_sub(1, Ordering::AcqRel);
                return ScheduledResponse {
                    request_id,
                    result: Err(ScheduledError::WorkerFailure),
                };
            }
            Err(_) => {
                self.queued.fetch_sub(1, Ordering::AcqRel);
                return ScheduledResponse {
                    request_id,
                    result: Err(ScheduledError::Timeout),
                };
            }
        };
        self.queued.fetch_sub(1, Ordering::AcqRel);
        if cancellation.is_cancelled() {
            drop(permit);
            return ScheduledResponse {
                request_id,
                result: Err(ScheduledError::Cancelled),
            };
        }

        let work = self
            .transport
            .execute(&request, &cancellation, self.limits.max_response_bytes);
        let timed = tokio::time::timeout(self.limits.request_timeout, work);
        let result = match tokio::select! {
            _ = cancellation.cancelled() => Err(ScheduledError::Cancelled),
            result = timed => match result {
                Err(_) => Err(ScheduledError::Timeout),
                Ok(result) => Ok(result),
            },
        } {
            Err(error) => Err(error),
            Ok(Err(error)) if cancellation.is_cancelled() => {
                let _ = error;
                Err(ScheduledError::Cancelled)
            }
            Ok(Err(error)) => Err(ScheduledError::Transport(error)),
            Ok(Ok(result)) => {
                let bytes = result
                    .binary_body
                    .as_ref()
                    .map(Vec::len)
                    .unwrap_or_else(|| result.body.len());
                if bytes > self.limits.max_response_bytes {
                    Err(ScheduledError::ResponseBudgetExceeded)
                } else if cancellation.is_cancelled() {
                    Err(ScheduledError::Cancelled)
                } else {
                    Ok(result)
                }
            }
        };
        drop(permit);
        ScheduledResponse { request_id, result }
    }

    pub async fn execute_batch(
        &self,
        mut requests: Vec<(ScheduledRequest, CancellationToken)>,
    ) -> Vec<ScheduledResponse> {
        if requests.len() > self.limits.max_queued {
            return requests
                .into_iter()
                .map(|(request, _)| ScheduledResponse {
                    request_id: request.id,
                    result: Err(ScheduledError::QueueFull),
                })
                .collect();
        }
        requests.sort_by_key(|(request, _)| (std::cmp::Reverse(request.priority), request.id));
        let mut workers = Vec::with_capacity(requests.len());
        for (request, cancellation) in requests {
            let scheduler = self.clone();
            workers.push(tokio::spawn(async move {
                scheduler.fetch(request, cancellation).await
            }));
        }
        let mut responses = Vec::with_capacity(workers.len());
        for worker in workers {
            responses.push(worker.await.unwrap_or(ScheduledResponse {
                request_id: u64::MAX,
                result: Err(ScheduledError::WorkerFailure),
            }));
        }
        responses
    }
}

pub async fn fetch_navigation(
    url: String,
    cookie_header: String,
    max_retries: u32,
) -> Result<FetchResult, String> {
    fetch_shared(
        url,
        cookie_header,
        max_retries,
        RequestPriority::Navigation,
        ResponseMode::Document,
        CancellationToken::default(),
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ExternalResourceKind {
    Script,
    Style,
}

#[derive(Debug, Clone)]
struct ExternalResource {
    kind: ExternalResourceKind,
    url: String,
}

/// Fetch the top-level document and its bounded external script/style graph
/// through the same scheduler before document preparation. Failed optional
/// subresources do not replace an otherwise valid document, but are counted
/// in a response diagnostic header.
pub async fn fetch_document_bundle(
    url: String,
    cookie_header: String,
    max_retries: u32,
    cancellation: CancellationToken,
) -> Result<FetchResult, String> {
    const MAX_EXTERNAL_RESOURCES: usize = 64;
    let subresource_cookie_header = cookie_header.clone();
    let mut document = fetch_shared(
        url,
        cookie_header,
        max_retries,
        RequestPriority::Navigation,
        ResponseMode::Document,
        cancellation.clone(),
    )
    .await?;
    if cancellation.is_cancelled()
        || !document
            .content_type
            .to_ascii_lowercase()
            .starts_with("text/html")
        || document.body.is_empty()
    {
        return Ok(document);
    }

    let mut dom = crate::parser::parse_html(&document.body);
    let document_url = url::Url::parse(&document.url).map_err(|error| error.to_string())?;
    let base_url = dom
        .find_tag("base")
        .and_then(|element| element.get_attr("href"))
        .and_then(|href| document_url.join(href).ok())
        .unwrap_or(document_url);
    let mut resources = Vec::new();
    collect_external_resources(&dom, &base_url, &mut resources, MAX_EXTERNAL_RESOURCES);
    let mut tasks = Vec::with_capacity(resources.len());
    for resource in resources {
        let token = cancellation.clone();
        let resource_url = url::Url::parse(&resource.url).ok();
        let cookie_header = resource_url
            .as_ref()
            .filter(|resource_url| same_origin(&base_url, resource_url))
            .map(|_| subresource_cookie_header.clone())
            .unwrap_or_default();
        tasks.push(tokio::spawn(async move {
            let result = fetch_shared(
                resource.url.clone(),
                cookie_header,
                1,
                RequestPriority::ScriptStyle,
                ResponseMode::Document,
                token,
            )
            .await;
            (resource, result)
        }));
    }
    let mut bodies = std::collections::BTreeMap::<
        (ExternalResourceKind, String),
        std::collections::VecDeque<String>,
    >::new();
    let mut failures = 0_usize;
    for task in tasks {
        match task.await {
            Ok((resource, Ok(response))) => bodies
                .entry((resource.kind, resource.url))
                .or_default()
                .push_back(response.body),
            _ => failures = failures.saturating_add(1),
        }
    }
    if cancellation.is_cancelled() {
        return Err("Cancelled".to_string());
    }
    inject_external_resources(&mut dom, &base_url, &mut bodies);
    document.body = dom.to_html();
    document.headers.insert(
        "x-ghita-external-resource-failures".to_string(),
        failures.to_string(),
    );
    Ok(document)
}

fn collect_external_resources(
    element: &crate::parser::Element,
    base_url: &url::Url,
    output: &mut Vec<ExternalResource>,
    limit: usize,
) {
    if output.len() >= limit {
        return;
    }
    let candidate = if element.tag == "script" {
        element
            .get_attr("src")
            .map(|url| (ExternalResourceKind::Script, url))
    } else if element.tag == "link"
        && element.get_attr("rel").is_some_and(|rel| {
            rel.split_whitespace()
                .any(|token| token.eq_ignore_ascii_case("stylesheet"))
        })
    {
        element
            .get_attr("href")
            .map(|url| (ExternalResourceKind::Style, url))
    } else {
        None
    };
    if let Some((kind, relative)) = candidate {
        if let Ok(url) = base_url.join(relative) {
            if matches!(url.scheme(), "http" | "https") {
                output.push(ExternalResource {
                    kind,
                    url: url.to_string(),
                });
            }
        }
    }
    for child in &element.children {
        collect_external_resources(child, base_url, output, limit);
        if output.len() >= limit {
            break;
        }
    }
}

fn inject_external_resources(
    element: &mut crate::parser::Element,
    base_url: &url::Url,
    bodies: &mut std::collections::BTreeMap<
        (ExternalResourceKind, String),
        std::collections::VecDeque<String>,
    >,
) {
    let candidate = if element.tag == "script" {
        element
            .get_attr("src")
            .and_then(|relative| base_url.join(relative).ok())
            .map(|url| (ExternalResourceKind::Script, url.to_string()))
    } else if element.tag == "link"
        && element.get_attr("rel").is_some_and(|rel| {
            rel.split_whitespace()
                .any(|token| token.eq_ignore_ascii_case("stylesheet"))
        })
    {
        element
            .get_attr("href")
            .and_then(|relative| base_url.join(relative).ok())
            .map(|url| (ExternalResourceKind::Style, url.to_string()))
    } else {
        None
    };
    if let Some((kind, url)) = candidate {
        if let Some(body) = bodies
            .get_mut(&(kind, url))
            .and_then(|queue| queue.pop_front())
        {
            element.text = body;
            match kind {
                ExternalResourceKind::Script => {
                    element.attrs.remove("src");
                }
                ExternalResourceKind::Style => {
                    element.tag = "style".to_string();
                    element.attrs.remove("href");
                    element.attrs.remove("rel");
                    element.is_void = false;
                }
            }
        }
    }
    for child in &mut element.children {
        inject_external_resources(child, base_url, bodies);
    }
}

pub async fn fetch_shared(
    url: String,
    cookie_header: String,
    max_retries: u32,
    priority: RequestPriority,
    response_mode: ResponseMode,
    cancellation: CancellationToken,
) -> Result<FetchResult, String> {
    static SHARED_SCHEDULER: OnceLock<NetworkScheduler<ReqwestTransport>> = OnceLock::new();
    static NEXT_REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let scheduler = SHARED_SCHEDULER.get_or_init(|| {
        NetworkScheduler::new(
            ReqwestTransport::new().expect("async transport must initialize"),
            SchedulerLimits::default(),
        )
        .expect("default network scheduler limits must be valid")
    });
    let response = scheduler
        .fetch(
            ScheduledRequest {
                id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
                url,
                cookie_header,
                max_retries,
                priority,
                response_mode,
            },
            cancellation,
        )
        .await;
    response.result.map_err(|error| format!("{error:?}"))
}

/// Download a known-length binary resource using bounded HTTP byte ranges.
///
/// Media CDNs may deliberately keep an un-ranged response open. The browser
/// still owns cancellation, origin validation at the caller, byte budgets and
/// exact reassembly; every chunk must be a standards-compliant 206 response.
pub async fn fetch_binary_ranges(
    url: String,
    total_bytes: u64,
    max_bytes: usize,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, String> {
    const CHUNK_BYTES: u64 = 512 * 1024;
    if total_bytes == 0 || total_bytes > max_bytes as u64 || max_bytes > 64 * 1024 * 1024 {
        return Err("Ranged response exceeds its byte budget".to_string());
    }
    let parsed = url::Url::parse(&url).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https" {
        return Err("Ranged binary transport requires HTTPS".to_string());
    }
    let transport = ReqwestTransport::new()?;
    let mut bytes = Vec::with_capacity(total_bytes as usize);
    let mut start = 0_u64;
    while start < total_bytes {
        if cancellation.is_cancelled() {
            return Err("Ranged binary request cancelled".to_string());
        }
        let end = start.saturating_add(CHUNK_BYTES - 1).min(total_bytes - 1);
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err("Ranged binary request cancelled".to_string()),
            response = transport
                .client
                .get(parsed.clone())
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .send() => response.map_err(|error| format!("Ranged binary request failed: {error}"))?,
        };
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!(
                "Ranged binary request returned HTTP {}",
                response.status().as_u16()
            ));
        }
        let expected_content_range = format!("bytes {start}-{end}/{total_bytes}");
        let content_range = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if content_range != expected_content_range {
            return Err(format!(
                "Ranged binary response mismatch: expected {expected_content_range}, got {content_range}"
            ));
        }
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err("Ranged binary request cancelled".to_string()),
            chunk = response.bytes() => chunk.map_err(|error| format!("Ranged binary body failed: {error}"))?,
        };
        let expected_length = usize::try_from(end - start + 1)
            .map_err(|_| "Ranged binary length overflow".to_string())?;
        if chunk.len() != expected_length || bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err("Ranged binary body length mismatch".to_string());
        }
        bytes.extend_from_slice(&chunk);
        start = end.saturating_add(1);
    }
    if bytes.len() != total_bytes as usize {
        return Err("Ranged binary reassembly length mismatch".to_string());
    }
    Ok(bytes)
}

/// Resolve the exact length of a binary resource without downloading it.
/// The server must honor a one-byte range and disclose an exact total.
pub async fn probe_binary_length(
    url: String,
    max_bytes: usize,
    cancellation: CancellationToken,
) -> Result<u64, String> {
    if max_bytes == 0 || max_bytes > 64 * 1024 * 1024 {
        return Err("Binary length probe budget is invalid".to_string());
    }
    let parsed = url::Url::parse(&url).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https" {
        return Err("Binary length probe requires HTTPS".to_string());
    }
    let transport = ReqwestTransport::new()?;
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err("Binary length probe cancelled".to_string()),
        response = transport
            .client
            .get(parsed)
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send() => response.map_err(|error| format!("Binary length probe failed: {error}"))?,
    };
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(format!(
            "Binary length probe returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let content_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "Binary length probe returned no Content-Range".to_string())?;
    let prefix = "bytes 0-0/";
    let total = content_range
        .strip_prefix(prefix)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= max_bytes as u64)
        .ok_or_else(|| "Binary length probe returned an invalid total".to_string())?;
    let body = tokio::select! {
        _ = cancellation.cancelled() => return Err("Binary length probe cancelled".to_string()),
        body = response.bytes() => body.map_err(|error| format!("Binary length probe body failed: {error}"))?,
    };
    if body.len() != 1 {
        return Err("Binary length probe did not return exactly one byte".to_string());
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingTransport {
        starts: Mutex<Vec<u64>>,
    }

    impl NetworkTransport for RecordingTransport {
        fn execute<'a>(
            &'a self,
            request: &'a ScheduledRequest,
            cancellation: &'a CancellationToken,
            _max_response_bytes: usize,
        ) -> Pin<Box<dyn Future<Output = Result<FetchResult, String>> + Send + 'a>> {
            Box::pin(async move {
                if cancellation.is_cancelled() {
                    return Err("cancelled".to_string());
                }
                self.starts.lock().unwrap().push(request.id);
                Ok(FetchResult {
                    body: request.id.to_string(),
                    binary_body: None,
                    url: request.url.clone(),
                    status_code: 200,
                    content_type: "text/plain".to_string(),
                    headers: Default::default(),
                    fetch_time_ms: 1,
                    set_cookie_headers: Vec::new(),
                })
            })
        }
    }

    #[tokio::test]
    async fn priorities_cancellation_and_two_hundred_requests_are_bounded() {
        let scheduler = NetworkScheduler::new(
            RecordingTransport::default(),
            SchedulerLimits {
                max_concurrency: 1,
                max_queued: 256,
                ..SchedulerLimits::default()
            },
        )
        .unwrap();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        let mut requests = (0..200)
            .map(|id| {
                (
                    ScheduledRequest {
                        id,
                        url: format!("https://local.test/{id}"),
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
                id: 500,
                url: "https://local.test/navigation".to_string(),
                cookie_header: String::new(),
                max_retries: 0,
                priority: RequestPriority::Navigation,
                response_mode: ResponseMode::Document,
            },
            CancellationToken::default(),
        ));
        requests.push((
            ScheduledRequest {
                id: 501,
                url: "https://local.test/cancelled".to_string(),
                cookie_header: String::new(),
                max_retries: 0,
                priority: RequestPriority::Navigation,
                response_mode: ResponseMode::Document,
            },
            cancelled,
        ));
        let responses = scheduler.execute_batch(requests).await;
        assert_eq!(responses.len(), 202);
        assert_eq!(responses[0].request_id, 500);
        assert!(responses[0].result.is_ok());
        assert!(matches!(
            responses[1].result,
            Err(ScheduledError::Cancelled)
        ));
        assert_eq!(scheduler.queued_len(), 0);
    }
}

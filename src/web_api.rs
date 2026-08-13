//! Bounded browser-facing networking Web APIs.
//!
//! The older `network` module loads top-level documents. This module applies
//! document-origin policy to script-visible requests: asynchronous settlement,
//! CORS/preflight, redirect modes, credentials, abort signals and filtered
//! response headers. It intentionally supports HTTP(S) only and keeps all
//! queues, headers, bodies, redirects, timers and storage notifications bounded.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use url::Url;

const MAX_HEADER_ENTRIES: usize = 128;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 50 * 1024 * 1024;
const MAX_FETCH_TASKS: usize = 256;
const MAX_PROMISES: usize = 1_024;
const MAX_REDIRECTS: usize = 10;
const MAX_TIMERS: usize = 2_048;
const MAX_STORAGE_CONTEXTS: usize = 256;
const MAX_STORAGE_EVENTS: usize = 2_048;
const MAX_STORAGE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    entries: BTreeMap<String, Vec<String>>,
    bytes: usize,
}

impl Headers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_pairs<I, K, V>(pairs: I) -> Result<Self, FetchError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut headers = Self::new();
        for (name, value) in pairs {
            headers.append(name.as_ref(), value.as_ref())?;
        }
        Ok(headers)
    }

    pub fn append(&mut self, name: &str, value: &str) -> Result<(), FetchError> {
        let name = normalize_header_name(name)?;
        let value = normalize_header_value(value)?;
        let creates_entry = !self.entries.contains_key(&name);
        if creates_entry && self.entries.len() >= MAX_HEADER_ENTRIES {
            return Err(FetchError::QuotaExceeded("header entry budget exceeded"));
        }
        let cost = name.len().saturating_add(value.len());
        if self.bytes.saturating_add(cost) > MAX_HEADER_BYTES {
            return Err(FetchError::QuotaExceeded("header byte budget exceeded"));
        }
        self.entries.entry(name).or_default().push(value);
        self.bytes = self.bytes.saturating_add(cost);
        Ok(())
    }

    pub fn set(&mut self, name: &str, value: &str) -> Result<(), FetchError> {
        let name = normalize_header_name(name)?;
        let value = normalize_header_value(value)?;
        let old_cost = self
            .entries
            .get(&name)
            .map(|values| name.len() * values.len() + values.iter().map(String::len).sum::<usize>())
            .unwrap_or_default();
        if !self.entries.contains_key(&name) && self.entries.len() >= MAX_HEADER_ENTRIES {
            return Err(FetchError::QuotaExceeded("header entry budget exceeded"));
        }
        let projected = self
            .bytes
            .saturating_sub(old_cost)
            .saturating_add(name.len())
            .saturating_add(value.len());
        if projected > MAX_HEADER_BYTES {
            return Err(FetchError::QuotaExceeded("header byte budget exceeded"));
        }
        self.entries.insert(name, vec![value]);
        self.bytes = projected;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.entries
            .get(&name.to_ascii_lowercase())
            .map(|values| values.join(", "))
    }

    pub fn get_all(&self, name: &str) -> Vec<String> {
        self.entries
            .get(&name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    pub fn has(&self, name: &str) -> bool {
        self.entries.contains_key(&name.to_ascii_lowercase())
    }

    pub fn delete(&mut self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        let Some(values) = self.entries.remove(&name) else {
            return false;
        };
        self.bytes = self.bytes.saturating_sub(
            name.len() * values.len() + values.iter().map(String::len).sum::<usize>(),
        );
        true
    }

    pub fn pairs(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .map(|(name, values)| (name.clone(), values.join(", ")))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn normalize_header_name(name: &str) -> Result<String, FetchError> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > 256
        || !normalized.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        return Err(FetchError::InvalidHeader(name.to_string()));
    }
    Ok(normalized)
}

fn normalize_header_value(value: &str) -> Result<String, FetchError> {
    if value.contains(['\r', '\n']) {
        return Err(FetchError::InvalidHeader(
            "header values cannot contain line breaks".to_string(),
        ));
    }
    Ok(value.trim().chars().take(MAX_HEADER_BYTES).collect())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    pub fn parse(value: &str) -> Result<Self, FetchError> {
        let url = Url::parse(value).map_err(|error| FetchError::InvalidUrl(error.to_string()))?;
        Self::from_url(&url)
    }

    fn from_url(url: &Url) -> Result<Self, FetchError> {
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(FetchError::InvalidUrl(format!(
                "unsupported origin scheme: {scheme}"
            )));
        }
        let host = url
            .host_str()
            .ok_or_else(|| FetchError::InvalidUrl("origin has no host".to_string()))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| FetchError::InvalidUrl("origin has no port".to_string()))?;
        Ok(Self {
            scheme: scheme.to_string(),
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    pub fn serialize(&self) -> String {
        let default_port = (self.scheme == "http" && self.port == 80)
            || (self.scheme == "https" && self.port == 443);
        if default_port {
            format!("{}://{}", self.scheme, self.host)
        } else {
            format!("{}://{}:{}", self.scheme, self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebUrl {
    inner: Url,
}

impl WebUrl {
    pub fn parse(value: &str) -> Result<Self, FetchError> {
        let inner = Url::parse(value).map_err(|error| FetchError::InvalidUrl(error.to_string()))?;
        Ok(Self { inner })
    }

    pub fn parse_with_base(value: &str, base: &str) -> Result<Self, FetchError> {
        let base = Url::parse(base).map_err(|error| FetchError::InvalidUrl(error.to_string()))?;
        let inner = base
            .join(value)
            .map_err(|error| FetchError::InvalidUrl(error.to_string()))?;
        Ok(Self { inner })
    }

    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    pub fn origin(&self) -> Result<Origin, FetchError> {
        Origin::from_url(&self.inner)
    }

    pub fn protocol(&self) -> &str {
        self.inner.scheme()
    }

    pub fn host(&self) -> Option<&str> {
        self.inner.host_str()
    }

    pub fn pathname(&self) -> &str {
        self.inner.path()
    }

    pub fn search_params(&self) -> UrlSearchParams {
        UrlSearchParams::parse(self.inner.query().unwrap_or_default())
    }

    pub fn set_search_params(&mut self, params: &UrlSearchParams) {
        let query = params.to_query_string();
        self.inner
            .set_query((!query.is_empty()).then_some(query.as_str()));
    }
}

impl fmt::Display for WebUrl {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(self.inner.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrlSearchParams {
    pairs: Vec<(String, String)>,
}

impl UrlSearchParams {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse(query: &str) -> Self {
        let query = query.strip_prefix('?').unwrap_or(query);
        let pairs = url::form_urlencoded::parse(query.as_bytes())
            .take(4_096)
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        Self { pairs }
    }

    pub fn append(&mut self, name: &str, value: &str) -> Result<(), FetchError> {
        if self.pairs.len() >= 4_096 || name.len().saturating_add(value.len()) > MAX_HEADER_BYTES {
            return Err(FetchError::QuotaExceeded("URLSearchParams budget exceeded"));
        }
        self.pairs.push((name.to_string(), value.to_string()));
        Ok(())
    }

    pub fn set(&mut self, name: &str, value: &str) -> Result<(), FetchError> {
        self.delete(name);
        self.append(name, value)
    }

    pub fn delete(&mut self, name: &str) {
        self.pairs.retain(|(candidate, _)| candidate != name);
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.pairs
            .iter()
            .filter(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    pub fn sort(&mut self) {
        self.pairs.sort_by(|left, right| left.0.cmp(&right.0));
    }

    pub fn to_query_string(&self) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (name, value) in &self.pairs {
            serializer.append_pair(name, value);
        }
        serializer.finish()
    }
}

#[derive(Debug, Clone)]
pub struct AbortSignal {
    aborted: Arc<AtomicBool>,
}

impl AbortSignal {
    pub fn aborted(&self) -> bool {
        self.aborted.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone)]
pub struct AbortController {
    signal: AbortSignal,
}

impl Default for AbortController {
    fn default() -> Self {
        Self::new()
    }
}

impl AbortController {
    pub fn new() -> Self {
        Self {
            signal: AbortSignal {
                aborted: Arc::new(AtomicBool::new(false)),
            },
        }
    }

    pub fn signal(&self) -> AbortSignal {
        self.signal.clone()
    }

    pub fn abort(&self) {
        self.signal.aborted.store(true, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    SameOrigin,
    Cors,
    NoCors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialsMode {
    Omit,
    SameOrigin,
    Include,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectMode {
    Follow,
    Error,
    Manual,
}

#[derive(Debug, Clone)]
pub struct WebRequest {
    pub url: WebUrl,
    pub method: String,
    pub headers: Headers,
    pub body: Vec<u8>,
    pub mode: RequestMode,
    pub credentials: CredentialsMode,
    pub redirect: RedirectMode,
    pub signal: Option<AbortSignal>,
}

impl WebRequest {
    pub fn get(url: &str) -> Result<Self, FetchError> {
        Ok(Self {
            url: WebUrl::parse(url)?,
            method: "GET".to_string(),
            headers: Headers::new(),
            body: Vec::new(),
            mode: RequestMode::Cors,
            credentials: CredentialsMode::SameOrigin,
            redirect: RedirectMode::Follow,
            signal: None,
        })
    }

    pub fn set_method(&mut self, method: &str) -> Result<(), FetchError> {
        let method = method.trim().to_ascii_uppercase();
        validate_method(&method)?;
        self.method = method;
        Ok(())
    }

    pub fn set_body(&mut self, body: impl Into<Vec<u8>>) -> Result<(), FetchError> {
        let body = body.into();
        if matches!(self.method.as_str(), "GET" | "HEAD") && !body.is_empty() {
            return Err(FetchError::InvalidRequest(
                "GET and HEAD requests cannot have a body".to_string(),
            ));
        }
        if body.len() > MAX_BODY_BYTES {
            return Err(FetchError::QuotaExceeded("request body exceeds 50 MB"));
        }
        self.body = body;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    Basic,
    Cors,
    Opaque,
    OpaqueRedirect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebResponse {
    pub response_type: ResponseType,
    pub url: String,
    pub status: u16,
    pub status_text: String,
    pub headers: Headers,
    pub body: Vec<u8>,
    pub redirected: bool,
}

impl WebResponse {
    pub fn ok(&self) -> bool {
        (200..=299).contains(&self.status)
    }

    pub fn text(&self) -> Result<String, FetchError> {
        String::from_utf8(self.body.clone()).map_err(|error| FetchError::Decode(error.to_string()))
    }

    pub fn json(&self) -> Result<serde_json::Value, FetchError> {
        serde_json::from_slice(&self.body).map_err(|error| FetchError::Decode(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    InvalidUrl(String),
    InvalidHeader(String),
    InvalidMethod(String),
    InvalidRequest(String),
    SameOriginViolation,
    Cors(String),
    Redirect(String),
    Aborted,
    Network(String),
    Decode(String),
    QuotaExceeded(&'static str),
}

impl fmt::Display for FetchError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(reason) => write!(output, "Invalid URL: {reason}"),
            Self::InvalidHeader(reason) => write!(output, "Invalid header: {reason}"),
            Self::InvalidMethod(method) => write!(output, "Invalid method: {method}"),
            Self::InvalidRequest(reason) => write!(output, "Invalid request: {reason}"),
            Self::SameOriginViolation => output.write_str("Same-origin policy blocked the request"),
            Self::Cors(reason) => write!(output, "CORS blocked the request: {reason}"),
            Self::Redirect(reason) => write!(output, "Redirect blocked: {reason}"),
            Self::Aborted => output.write_str("AbortError"),
            Self::Network(reason) => write!(output, "Network error: {reason}"),
            Self::Decode(reason) => write!(output, "Decode error: {reason}"),
            Self::QuotaExceeded(reason) => write!(output, "Quota exceeded: {reason}"),
        }
    }
}

impl std::error::Error for FetchError {}

pub type FetchPromiseId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchPromiseState {
    Pending,
    Fulfilled(WebResponse),
    Rejected(FetchError),
}

#[derive(Debug)]
struct PendingFetch {
    id: FetchPromiseId,
    request: WebRequest,
}

struct HttpExchange {
    url: String,
    status: u16,
    status_text: String,
    headers: Headers,
    body: Vec<u8>,
    set_cookie: Vec<String>,
}

struct HttpRequestOptions<'a> {
    headers: &'a Headers,
    body: &'a [u8],
    cookie: Option<&'a str>,
    origin: Option<&'a str>,
    browser_generated_headers: bool,
}

/// Script-visible fetch queue. Calling `fetch` creates a pending promise;
/// network work runs only when the embedding event loop calls `run_one` or
/// `drain`, so settlement is never delivered inline with the call.
pub struct FetchRuntime {
    client_origin: Origin,
    agent: ureq::Agent,
    queue: VecDeque<PendingFetch>,
    promises: BTreeMap<FetchPromiseId, FetchPromiseState>,
    next_promise: FetchPromiseId,
    cookies: crate::storage::CookieStore,
}

impl fmt::Debug for FetchRuntime {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output
            .debug_struct("FetchRuntime")
            .field("client_origin", &self.client_origin)
            .field("queue_len", &self.queue.len())
            .field("promise_count", &self.promises.len())
            .finish_non_exhaustive()
    }
}

impl FetchRuntime {
    pub fn new(client_origin: &str) -> Result<Self, FetchError> {
        Ok(Self {
            client_origin: Origin::parse(client_origin)?,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(30))
                .redirects(0)
                .user_agent(&crate::network::browser_ua())
                .build(),
            queue: VecDeque::new(),
            promises: BTreeMap::new(),
            next_promise: 1,
            cookies: crate::storage::CookieStore::new(),
        })
    }

    pub fn fetch(&mut self, request: WebRequest) -> Result<FetchPromiseId, FetchError> {
        validate_request(&request)?;
        if self.queue.len() >= MAX_FETCH_TASKS || self.promises.len() >= MAX_PROMISES {
            return Err(FetchError::QuotaExceeded("fetch queue budget exceeded"));
        }
        let id = self.next_promise;
        self.next_promise = self
            .next_promise
            .checked_add(1)
            .ok_or(FetchError::QuotaExceeded(
                "fetch identifier space exhausted",
            ))?;
        self.promises.insert(id, FetchPromiseState::Pending);
        self.queue.push_back(PendingFetch { id, request });
        Ok(id)
    }

    pub fn promise(&self, id: FetchPromiseId) -> Option<&FetchPromiseState> {
        self.promises.get(&id)
    }

    pub fn take_promise(&mut self, id: FetchPromiseId) -> Option<FetchPromiseState> {
        self.promises.remove(&id)
    }

    pub fn pending_len(&self) -> usize {
        self.queue.len()
    }

    pub fn run_one(&mut self) -> bool {
        let Some(pending) = self.queue.pop_front() else {
            return false;
        };
        let result = self.execute(pending.request);
        let state = match result {
            Ok(response) => FetchPromiseState::Fulfilled(response),
            Err(error) => FetchPromiseState::Rejected(error),
        };
        self.promises.insert(pending.id, state);
        true
    }

    pub fn drain(&mut self, budget: usize) -> usize {
        let mut completed = 0;
        while completed < budget.min(MAX_FETCH_TASKS) && self.run_one() {
            completed += 1;
        }
        completed
    }

    pub fn cookie_store(&self) -> &crate::storage::CookieStore {
        &self.cookies
    }

    pub fn cookie_store_mut(&mut self) -> &mut crate::storage::CookieStore {
        &mut self.cookies
    }

    fn execute(&mut self, mut request: WebRequest) -> Result<WebResponse, FetchError> {
        if request.signal.as_ref().is_some_and(AbortSignal::aborted) {
            return Err(FetchError::Aborted);
        }
        let initial_origin = request.url.origin()?;
        let initial_cross_origin = initial_origin != self.client_origin;
        if initial_cross_origin && request.mode == RequestMode::SameOrigin {
            return Err(FetchError::SameOriginViolation);
        }
        if request.mode == RequestMode::NoCors && !is_simple_method(&request.method) {
            return Err(FetchError::Cors(
                "no-cors mode only permits safelisted methods".to_string(),
            ));
        }

        let mut current = request.url.inner.clone();
        let mut redirected = false;
        let mut redirect_tainted = false;
        for hop in 0..=MAX_REDIRECTS {
            if request.signal.as_ref().is_some_and(AbortSignal::aborted) {
                return Err(FetchError::Aborted);
            }
            let target_origin = Origin::from_url(&current)?;
            let cross_origin = target_origin != self.client_origin;
            if cross_origin && request.mode == RequestMode::SameOrigin {
                return Err(FetchError::SameOriginViolation);
            }

            if cross_origin && request.mode == RequestMode::Cors && needs_preflight(&request) {
                self.preflight(&current, &request)?;
            }

            let credentials_allowed = match request.credentials {
                CredentialsMode::Omit => false,
                CredentialsMode::SameOrigin => !cross_origin,
                CredentialsMode::Include => true,
            };
            let cookie = credentials_allowed.then(|| self.cookie_header(current.as_str()));
            let origin = cross_origin.then(|| self.client_origin.serialize());
            let exchange = self.send_http(
                &request.method,
                &current,
                HttpRequestOptions {
                    headers: &request.headers,
                    body: &request.body,
                    cookie: cookie.as_deref().filter(|value| !value.is_empty()),
                    origin: origin.as_deref(),
                    browser_generated_headers: false,
                },
            )?;

            if request.signal.as_ref().is_some_and(AbortSignal::aborted) {
                return Err(FetchError::Aborted);
            }
            if cross_origin && request.mode == RequestMode::Cors {
                validate_cors_response(
                    &exchange.headers,
                    &self.client_origin,
                    request.credentials == CredentialsMode::Include,
                )?;
            }
            if credentials_allowed {
                self.store_response_cookies(&exchange.url, &exchange.set_cookie);
            }

            if is_redirect_status(exchange.status) {
                let Some(location) = exchange.headers.get("location") else {
                    return self.finish_response(
                        exchange,
                        request.mode,
                        cross_origin,
                        redirected,
                        redirect_tainted,
                    );
                };
                match request.redirect {
                    RedirectMode::Error => {
                        return Err(FetchError::Redirect(
                            "request redirect mode is 'error'".to_string(),
                        ))
                    }
                    RedirectMode::Manual => {
                        return Ok(WebResponse {
                            response_type: ResponseType::OpaqueRedirect,
                            url: String::new(),
                            status: 0,
                            status_text: String::new(),
                            headers: Headers::new(),
                            body: Vec::new(),
                            redirected: false,
                        })
                    }
                    RedirectMode::Follow => {}
                }
                if hop == MAX_REDIRECTS {
                    return Err(FetchError::Redirect(
                        "redirect count exceeded 10".to_string(),
                    ));
                }
                let next = current
                    .join(&location)
                    .map_err(|error| FetchError::Redirect(error.to_string()))?;
                let next_origin = Origin::from_url(&next)?;
                if next_origin != target_origin {
                    request.headers.delete("authorization");
                    request.headers.delete("proxy-authorization");
                    redirect_tainted = true;
                }
                if exchange.status == 303
                    || ((exchange.status == 301 || exchange.status == 302)
                        && request.method == "POST")
                {
                    request.method = "GET".to_string();
                    request.body.clear();
                    request.headers.delete("content-type");
                    request.headers.delete("content-length");
                }
                current = next;
                redirected = true;
                continue;
            }

            return self.finish_response(
                exchange,
                request.mode,
                cross_origin,
                redirected,
                redirect_tainted,
            );
        }
        Err(FetchError::Redirect(
            "redirect count exceeded 10".to_string(),
        ))
    }

    fn finish_response(
        &self,
        exchange: HttpExchange,
        mode: RequestMode,
        cross_origin: bool,
        redirected: bool,
        redirect_tainted: bool,
    ) -> Result<WebResponse, FetchError> {
        if mode == RequestMode::NoCors && cross_origin {
            return Ok(WebResponse {
                response_type: ResponseType::Opaque,
                url: String::new(),
                status: 0,
                status_text: String::new(),
                headers: Headers::new(),
                body: Vec::new(),
                redirected,
            });
        }
        let response_type = if cross_origin || redirect_tainted {
            ResponseType::Cors
        } else {
            ResponseType::Basic
        };
        let headers = filter_response_headers(exchange.headers, response_type)?;
        Ok(WebResponse {
            response_type,
            url: exchange.url,
            status: exchange.status,
            status_text: exchange.status_text,
            headers,
            body: exchange.body,
            redirected,
        })
    }

    fn preflight(&self, url: &Url, request: &WebRequest) -> Result<(), FetchError> {
        let mut headers = Headers::new();
        headers.set("origin", &self.client_origin.serialize())?;
        headers.set("access-control-request-method", &request.method)?;
        let non_simple = non_safelisted_header_names(&request.headers);
        if !non_simple.is_empty() {
            headers.set(
                "access-control-request-headers",
                &non_simple.into_iter().collect::<Vec<_>>().join(", "),
            )?;
        }
        let exchange = self.send_http(
            "OPTIONS",
            url,
            HttpRequestOptions {
                headers: &headers,
                body: &[],
                cookie: None,
                origin: None,
                browser_generated_headers: true,
            },
        )?;
        if !(200..=299).contains(&exchange.status) {
            return Err(FetchError::Cors(format!(
                "preflight returned status {}",
                exchange.status
            )));
        }
        validate_cors_response(
            &exchange.headers,
            &self.client_origin,
            request.credentials == CredentialsMode::Include,
        )?;
        let methods = comma_tokens(exchange.headers.get("access-control-allow-methods"));
        if !is_simple_method(&request.method)
            && !methods.contains(&request.method.to_ascii_lowercase())
        {
            return Err(FetchError::Cors(format!(
                "method {} was not allowed by preflight",
                request.method
            )));
        }
        let allowed_headers = comma_tokens(exchange.headers.get("access-control-allow-headers"));
        for header in non_safelisted_header_names(&request.headers) {
            if !allowed_headers.contains(&header) && !allowed_headers.contains("*") {
                return Err(FetchError::Cors(format!(
                    "header {header} was not allowed by preflight"
                )));
            }
        }
        Ok(())
    }

    fn send_http(
        &self,
        method: &str,
        url: &Url,
        options: HttpRequestOptions<'_>,
    ) -> Result<HttpExchange, FetchError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(FetchError::InvalidUrl(format!(
                "unsupported fetch scheme: {}",
                url.scheme()
            )));
        }
        let mut outbound = self.agent.request(method, url.as_str());
        for (name, value) in options.headers.pairs() {
            if options.browser_generated_headers || !is_forbidden_request_header(&name) {
                outbound = outbound.set(&name, &value);
            }
        }
        if let Some(cookie) = options.cookie {
            outbound = outbound.set("cookie", cookie);
        }
        if let Some(origin) = options.origin {
            outbound = outbound.set("origin", origin);
        }
        let result = if options.body.is_empty() {
            outbound.call()
        } else {
            outbound.send_bytes(options.body)
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(error) => return Err(FetchError::Network(error.to_string())),
        };
        let status = response.status();
        let status_text = response.status_text().to_string();
        let response_url = response.get_url().to_string();
        let mut response_headers = Headers::new();
        for name in response.headers_names() {
            for value in response.all(&name) {
                response_headers.append(&name, value)?;
            }
        }
        let set_cookie = response
            .all("set-cookie")
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_BODY_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| FetchError::Network(error.to_string()))?;
        if bytes.len() > MAX_BODY_BYTES {
            return Err(FetchError::QuotaExceeded("response body exceeds 50 MB"));
        }
        Ok(HttpExchange {
            url: response_url,
            status,
            status_text,
            headers: response_headers,
            body: bytes,
            set_cookie,
        })
    }

    fn cookie_header(&self, url: &str) -> String {
        let domain = Url::parse(url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        self.cookies
            .get_cookies(&domain)
            .into_iter()
            .filter(|cookie| cookie.matches_url(url))
            .map(|cookie| cookie.to_header_value())
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn store_response_cookies(&mut self, url: &str, values: &[String]) {
        let domain = Url::parse(url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        for value in values {
            let cookie = crate::storage::Cookie::from_set_cookie_header(value, &domain);
            if !cookie.name.is_empty() && cookie.matches_url(url) {
                self.cookies.add_cookie(cookie);
            }
        }
    }
}

fn validate_request(request: &WebRequest) -> Result<(), FetchError> {
    let origin = request.url.origin()?;
    let _ = origin;
    validate_method(&request.method)?;
    if request.body.len() > MAX_BODY_BYTES {
        return Err(FetchError::QuotaExceeded("request body exceeds 50 MB"));
    }
    if matches!(request.method.as_str(), "GET" | "HEAD") && !request.body.is_empty() {
        return Err(FetchError::InvalidRequest(
            "GET and HEAD requests cannot have a body".to_string(),
        ));
    }
    Ok(())
}

fn validate_method(method: &str) -> Result<(), FetchError> {
    if method.is_empty()
        || method.len() > 32
        || method != method.to_ascii_uppercase()
        || !method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || matches!(method, "CONNECT" | "TRACE" | "TRACK")
    {
        return Err(FetchError::InvalidMethod(method.to_string()));
    }
    Ok(())
}

fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn is_simple_method(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "POST")
}

fn is_safelisted_request_header(name: &str, value: &str) -> bool {
    match name {
        "accept" | "accept-language" | "content-language" => value.len() <= 128,
        "content-type" => {
            let mime = value.split(';').next().unwrap_or_default().trim();
            matches!(
                mime.to_ascii_lowercase().as_str(),
                "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
            )
        }
        _ => false,
    }
}

fn non_safelisted_header_names(headers: &Headers) -> BTreeSet<String> {
    headers
        .pairs()
        .into_iter()
        .filter(|(name, value)| {
            !is_forbidden_request_header(name) && !is_safelisted_request_header(name, value)
        })
        .map(|(name, _)| name)
        .collect()
}

fn needs_preflight(request: &WebRequest) -> bool {
    !is_simple_method(&request.method) || !non_safelisted_header_names(&request.headers).is_empty()
}

fn is_forbidden_request_header(name: &str) -> bool {
    matches!(
        name,
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "referer"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "via"
    ) || name.starts_with("proxy-")
        || name.starts_with("sec-")
}

fn validate_cors_response(
    headers: &Headers,
    origin: &Origin,
    credentials_include: bool,
) -> Result<(), FetchError> {
    let allowed_origin = headers
        .get("access-control-allow-origin")
        .ok_or_else(|| FetchError::Cors("missing Access-Control-Allow-Origin".to_string()))?;
    if allowed_origin == "*" && credentials_include {
        return Err(FetchError::Cors(
            "wildcard origin cannot be used with credentials".to_string(),
        ));
    }
    if allowed_origin != "*" && allowed_origin != origin.serialize() {
        return Err(FetchError::Cors(format!(
            "origin {} was not allowed",
            origin.serialize()
        )));
    }
    if credentials_include
        && !headers
            .get("access-control-allow-credentials")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        return Err(FetchError::Cors(
            "credentialed response did not opt in".to_string(),
        ));
    }
    Ok(())
}

fn comma_tokens(value: Option<String>) -> BTreeSet<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn filter_response_headers(
    headers: Headers,
    response_type: ResponseType,
) -> Result<Headers, FetchError> {
    let exposed = comma_tokens(headers.get("access-control-expose-headers"));
    let mut filtered = Headers::new();
    for (name, value) in headers.pairs() {
        if matches!(name.as_str(), "set-cookie" | "set-cookie2") {
            continue;
        }
        let cors_safelisted = matches!(
            name.as_str(),
            "cache-control"
                | "content-language"
                | "content-length"
                | "content-type"
                | "expires"
                | "last-modified"
                | "pragma"
        );
        if response_type == ResponseType::Basic
            || cors_safelisted
            || exposed.contains(&name)
            || exposed.contains("*")
        {
            filtered.append(&name, &value)?;
        }
    }
    Ok(filtered)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XhrReadyState {
    Unsent = 0,
    Opened = 1,
    HeadersReceived = 2,
    Loading = 3,
    Done = 4,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XhrEvent {
    ReadyStateChange(XhrReadyState),
    Load,
    Error(FetchError),
    Abort,
}

#[derive(Debug)]
pub struct XmlHttpRequest {
    ready_state: XhrReadyState,
    request: Option<WebRequest>,
    promise: Option<FetchPromiseId>,
    response: Option<WebResponse>,
    events: Vec<XhrEvent>,
    controller: AbortController,
    with_credentials: bool,
}

impl Default for XmlHttpRequest {
    fn default() -> Self {
        Self::new()
    }
}

impl XmlHttpRequest {
    pub fn new() -> Self {
        Self {
            ready_state: XhrReadyState::Unsent,
            request: None,
            promise: None,
            response: None,
            events: Vec::new(),
            controller: AbortController::new(),
            with_credentials: false,
        }
    }

    pub fn open(&mut self, method: &str, url: &str) -> Result<(), FetchError> {
        let mut request = WebRequest::get(url)?;
        request.set_method(method)?;
        request.mode = RequestMode::Cors;
        self.controller = AbortController::new();
        self.request = Some(request);
        self.promise = None;
        self.response = None;
        self.ready_state = XhrReadyState::Opened;
        self.events
            .push(XhrEvent::ReadyStateChange(XhrReadyState::Opened));
        Ok(())
    }

    pub fn set_request_header(&mut self, name: &str, value: &str) -> Result<(), FetchError> {
        if self.ready_state != XhrReadyState::Opened || self.promise.is_some() {
            return Err(FetchError::InvalidRequest(
                "XHR headers require OPENED state before send".to_string(),
            ));
        }
        self.request
            .as_mut()
            .expect("OPENED state has request")
            .headers
            .append(name, value)
    }

    pub fn set_with_credentials(&mut self, include: bool) {
        self.with_credentials = include;
    }

    pub fn send(
        &mut self,
        runtime: &mut FetchRuntime,
        body: Option<Vec<u8>>,
    ) -> Result<FetchPromiseId, FetchError> {
        if self.ready_state != XhrReadyState::Opened || self.promise.is_some() {
            return Err(FetchError::InvalidRequest(
                "XHR send requires a fresh OPENED state".to_string(),
            ));
        }
        let request = self.request.as_mut().expect("OPENED state has request");
        request.credentials = if self.with_credentials {
            CredentialsMode::Include
        } else {
            CredentialsMode::SameOrigin
        };
        request.signal = Some(self.controller.signal());
        if let Some(body) = body {
            request.set_body(body)?;
        }
        let id = runtime.fetch(request.clone())?;
        self.promise = Some(id);
        Ok(id)
    }

    pub fn poll(&mut self, runtime: &mut FetchRuntime) -> bool {
        let Some(id) = self.promise else {
            return false;
        };
        let Some(state) = runtime.promise(id).cloned() else {
            return false;
        };
        match state {
            FetchPromiseState::Pending => false,
            FetchPromiseState::Fulfilled(response) => {
                self.promise = None;
                for state in [
                    XhrReadyState::HeadersReceived,
                    XhrReadyState::Loading,
                    XhrReadyState::Done,
                ] {
                    self.ready_state = state;
                    self.events.push(XhrEvent::ReadyStateChange(state));
                }
                self.response = Some(response);
                self.events.push(XhrEvent::Load);
                true
            }
            FetchPromiseState::Rejected(FetchError::Aborted) => {
                self.promise = None;
                self.ready_state = XhrReadyState::Done;
                self.events
                    .push(XhrEvent::ReadyStateChange(XhrReadyState::Done));
                self.events.push(XhrEvent::Abort);
                true
            }
            FetchPromiseState::Rejected(error) => {
                self.promise = None;
                self.ready_state = XhrReadyState::Done;
                self.events
                    .push(XhrEvent::ReadyStateChange(XhrReadyState::Done));
                self.events.push(XhrEvent::Error(error));
                true
            }
        }
    }

    pub fn abort(&mut self) {
        self.controller.abort();
    }

    pub fn ready_state(&self) -> XhrReadyState {
        self.ready_state
    }

    pub fn status(&self) -> u16 {
        self.response
            .as_ref()
            .map(|response| response.status)
            .unwrap_or(0)
    }

    pub fn response_text(&self) -> Result<String, FetchError> {
        self.response
            .as_ref()
            .ok_or_else(|| FetchError::InvalidRequest("XHR response is not ready".to_string()))?
            .text()
    }

    pub fn events(&self) -> &[XhrEvent] {
        &self.events
    }
}

pub type TimerId = u64;

#[derive(Debug, Clone)]
struct Timer {
    callback: u64,
    due_ms: u64,
    interval_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub struct WebTimerQueue {
    now_ms: u64,
    next_timer: TimerId,
    timers: BTreeMap<TimerId, Timer>,
}

impl WebTimerQueue {
    pub fn new() -> Self {
        Self {
            next_timer: 1,
            ..Self::default()
        }
    }

    pub fn set_timeout(&mut self, callback: u64, delay_ms: u64) -> Result<TimerId, FetchError> {
        self.insert(callback, delay_ms, None)
    }

    pub fn set_interval(&mut self, callback: u64, interval_ms: u64) -> Result<TimerId, FetchError> {
        self.insert(callback, interval_ms.max(1), Some(interval_ms.max(1)))
    }

    fn insert(
        &mut self,
        callback: u64,
        delay_ms: u64,
        interval_ms: Option<u64>,
    ) -> Result<TimerId, FetchError> {
        if self.timers.len() >= MAX_TIMERS {
            return Err(FetchError::QuotaExceeded("timer budget exceeded"));
        }
        let id = self.next_timer;
        self.next_timer = self
            .next_timer
            .checked_add(1)
            .ok_or(FetchError::QuotaExceeded(
                "timer identifier space exhausted",
            ))?;
        self.timers.insert(
            id,
            Timer {
                callback,
                due_ms: self.now_ms.saturating_add(delay_ms),
                interval_ms,
            },
        );
        Ok(id)
    }

    pub fn clear(&mut self, id: TimerId) -> bool {
        self.timers.remove(&id).is_some()
    }

    pub fn advance(&mut self, elapsed_ms: u64, budget: usize) -> Vec<u64> {
        self.now_ms = self.now_ms.saturating_add(elapsed_ms);
        let due = self
            .timers
            .iter()
            .filter(|(_, timer)| timer.due_ms <= self.now_ms)
            .map(|(id, timer)| (*id, timer.due_ms))
            .collect::<Vec<_>>();
        let mut due = due;
        due.sort_by_key(|(id, at)| (*at, *id));
        let mut callbacks = Vec::new();
        for (id, _) in due.into_iter().take(budget.min(MAX_TIMERS)) {
            let Some(mut timer) = self.timers.remove(&id) else {
                continue;
            };
            callbacks.push(timer.callback);
            if let Some(interval) = timer.interval_ms {
                timer.due_ms = self.now_ms.saturating_add(interval);
                self.timers.insert(id, timer);
            }
        }
        callbacks
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageEvent {
    pub key: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub url: String,
    pub storage_origin: String,
}

#[derive(Debug, Clone)]
struct StorageContext {
    origin: Origin,
    events: VecDeque<StorageEvent>,
}

#[derive(Debug, Default)]
pub struct StorageEventBus {
    contexts: BTreeMap<u64, StorageContext>,
    values: BTreeMap<Origin, BTreeMap<String, String>>,
}

impl StorageEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_context(&mut self, id: u64, origin: &str) -> Result<(), FetchError> {
        if !self.contexts.contains_key(&id) && self.contexts.len() >= MAX_STORAGE_CONTEXTS {
            return Err(FetchError::QuotaExceeded("storage context budget exceeded"));
        }
        self.contexts.insert(
            id,
            StorageContext {
                origin: Origin::parse(origin)?,
                events: VecDeque::new(),
            },
        );
        Ok(())
    }

    pub fn set_item(
        &mut self,
        source: u64,
        key: &str,
        value: &str,
        url: &str,
    ) -> Result<(), FetchError> {
        if key.len().saturating_add(value.len()) > MAX_STORAGE_BYTES {
            return Err(FetchError::QuotaExceeded("storage item budget exceeded"));
        }
        let origin = self
            .contexts
            .get(&source)
            .ok_or_else(|| FetchError::InvalidRequest("unknown storage context".to_string()))?
            .origin
            .clone();
        let values = self.values.entry(origin.clone()).or_default();
        let previous_bytes = values
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>();
        let old = values.insert(key.to_string(), value.to_string());
        let projected = previous_bytes
            .saturating_sub(old.as_ref().map(String::len).unwrap_or_default())
            .saturating_add(value.len())
            .saturating_add(if old.is_none() { key.len() } else { 0 });
        if projected > MAX_STORAGE_BYTES {
            if let Some(old) = old {
                values.insert(key.to_string(), old);
            } else {
                values.remove(key);
            }
            return Err(FetchError::QuotaExceeded("storage origin budget exceeded"));
        }
        if old.as_deref() != Some(value) {
            self.broadcast(
                source,
                &origin,
                StorageEvent {
                    key: Some(key.to_string()),
                    old_value: old,
                    new_value: Some(value.to_string()),
                    url: url.to_string(),
                    storage_origin: origin.serialize(),
                },
            );
        }
        Ok(())
    }

    pub fn remove_item(&mut self, source: u64, key: &str, url: &str) -> Result<(), FetchError> {
        let origin = self
            .contexts
            .get(&source)
            .ok_or_else(|| FetchError::InvalidRequest("unknown storage context".to_string()))?
            .origin
            .clone();
        let old = self.values.entry(origin.clone()).or_default().remove(key);
        if let Some(old) = old {
            self.broadcast(
                source,
                &origin,
                StorageEvent {
                    key: Some(key.to_string()),
                    old_value: Some(old),
                    new_value: None,
                    url: url.to_string(),
                    storage_origin: origin.serialize(),
                },
            );
        }
        Ok(())
    }

    pub fn take_events(&mut self, context: u64) -> Vec<StorageEvent> {
        self.contexts
            .get_mut(&context)
            .map(|context| context.events.drain(..).collect())
            .unwrap_or_default()
    }

    fn broadcast(&mut self, source: u64, origin: &Origin, event: StorageEvent) {
        for (id, context) in &mut self.contexts {
            if *id != source
                && &context.origin == origin
                && context.events.len() < MAX_STORAGE_EVENTS
            {
                context.events.push_back(event.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_and_url_search_params_are_normalized_and_bounded() {
        let mut headers = Headers::new();
        headers.append("X-Test", " first ").unwrap();
        headers.append("x-test", "second").unwrap();
        assert_eq!(headers.get("X-Test").as_deref(), Some("first, second"));
        assert!(headers.set("bad\nname", "value").is_err());

        let mut params = UrlSearchParams::parse("q=rust+browser&q=engine");
        assert_eq!(params.get_all("q"), vec!["rust browser", "engine"]);
        params.set("page", "2").unwrap();
        let mut url = WebUrl::parse("https://example.test/search").unwrap();
        url.set_search_params(&params);
        assert!(url.as_str().contains("page=2"));
    }

    #[test]
    fn abort_timer_and_storage_delivery_are_deterministic() {
        let controller = AbortController::new();
        let signal = controller.signal();
        controller.abort();
        assert!(signal.aborted());

        let mut timers = WebTimerQueue::new();
        timers.set_timeout(7, 5).unwrap();
        timers.set_interval(9, 10).unwrap();
        assert_eq!(timers.advance(5, 8), vec![7]);
        assert_eq!(timers.advance(5, 8), vec![9]);
        assert_eq!(timers.advance(10, 8), vec![9]);

        let mut bus = StorageEventBus::new();
        bus.register_context(1, "https://app.test/").unwrap();
        bus.register_context(2, "https://app.test/other").unwrap();
        bus.register_context(3, "https://other.test/").unwrap();
        bus.set_item(1, "theme", "dark", "https://app.test/")
            .unwrap();
        assert!(bus.take_events(1).is_empty());
        assert_eq!(bus.take_events(2)[0].new_value.as_deref(), Some("dark"));
        assert!(bus.take_events(3).is_empty());
    }
}

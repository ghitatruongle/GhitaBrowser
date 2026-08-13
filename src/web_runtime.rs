//! Bounded host bridge for a deliberately small, clean-room web runtime.
//!
//! Pure ECMAScript continues to run through `JsvEngine`. This module handles
//! a safe first layer of DOM mutations, storage requests and same-origin fetch
//! discovery used by common progressively-enhanced documents.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;

use crate::css_parser::CssRule;
use crate::html_media::{HtmlMediaElement, MediaEvent};
use crate::javascript::{
    JsvHost, JsvValue, HOST_CACHE_STORAGE, HOST_DOCUMENT, HOST_EVENT, HOST_FORM_DATA, HOST_HISTORY,
    HOST_INDEXED_DB, HOST_LOCAL_STORAGE, HOST_LOCATION, HOST_NAVIGATOR, HOST_SERVICE_WORKER,
    HOST_WINDOW,
};
use crate::live_dom::{DefaultAction, DispatchReport, DomEvent, LiveDocument, NodeId};
use crate::media_backend::{merged_capabilities, FallbackRegistry, WindowsMediaFoundationBackend};
use crate::mse::MediaSource;
use crate::parser::Element;
use crate::runtime_core::{HeapHandle, HostObjectKind, RuntimeRealm, RuntimeValue};

const MAX_SCRIPTS: usize = 64;
const MAX_SCRIPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_HOST_OPERATIONS: usize = 2_048;
const MAX_PENDING_TASKS: usize = 1_024;
const MAX_TASK_SOURCE_BYTES: usize = 64 * 1024;
const MAX_TIMER_DELAY_MS: u64 = 60_000;
const MAX_ACTIVE_TIMERS: usize = 256;
const MAX_TIMERS_PER_PUMP: usize = 64;
const MAX_STORAGE_KEYS: usize = 1_024;
const MAX_STORAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_HISTORY_ENTRIES: usize = 100;
const MAX_EVENT_RECORDS: usize = 256;
const MAX_FORM_ENTRIES: usize = 256;
const MAX_JS_LISTENERS_PER_NODE: usize = 128;

/// Listener key for the `window` object. `HOST_WINDOW` (=2) can collide with
/// a real DOM NodeId, so window-level listeners use a reserved sentinel.
const LISTENER_WINDOW: u64 = u64::MAX;

fn runtime_origin(base_url: &str) -> String {
    url::Url::parse(base_url)
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|_| "null".to_string())
}

fn stable_origin_hash(origin: &str) -> u64 {
    // FNV-1a is used only as a stable filesystem key, never as a security
    // primitive. The full origin is still validated inside persisted files.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in origin.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Explicit bounded host capabilities (Phase 21). Canvas 2D and SVG shapes
/// are enabled by default; WebAssembly is not implemented and stays disabled
/// (fail closed) until an audited bounded interpreter exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HostCapability {
    Canvas2D,
    SvgShapes,
    WebAssembly,
}

/// Mutable 2D context state for one canvas element.
#[derive(Debug, Clone)]
struct CanvasState {
    canvas: NodeId,
    fill: String,
    stroke: String,
    font: String,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            canvas: 0,
            fill: "#000000".to_string(),
            stroke: "#000000".to_string(),
            font: "10px sans-serif".to_string(),
        }
    }
}

/// Origin-scoped in-memory localStorage view owned by the page runtime. The
/// persistent disk backing arrives with Phase 22; this type already enforces
/// the key, byte and string budgets that the persistent layer will reuse.
#[derive(Debug, Clone, Default)]
pub struct OriginStorage {
    entries: BTreeMap<String, String>,
    total_bytes: usize,
}

impl OriginStorage {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key.is_empty() || key.len() > 256 {
            return Err("Invalid key".to_string());
        }
        let previous = self.entries.get(key).map(String::len).unwrap_or(0);
        let projected = self
            .total_bytes
            .saturating_sub(previous)
            .saturating_add(value.len());
        if !self.entries.contains_key(key) && self.entries.len() >= MAX_STORAGE_KEYS {
            return Err("QuotaExceededError: localStorage key budget exceeded".to_string());
        }
        if projected > MAX_STORAGE_BYTES {
            return Err("QuotaExceededError: localStorage byte budget exceeded".to_string());
        }
        self.total_bytes = projected;
        self.entries.insert(key.to_string(), value.to_string());
        Ok(())
    }

    pub fn remove(&mut self, key: &str) {
        if let Some(value) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(value.len());
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    pub fn key_at(&self, index: usize) -> Option<&str> {
        self.entries.keys().nth(index).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebTaskKind {
    Microtask,
    Timer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTask {
    pub kind: WebTaskKind,
    pub source: String,
    pub due_ms: u64,
    order: u64,
}

/// Deterministic, bounded task queue used by the web-runtime host bridge.
/// Microtasks always drain before timers that are ready at the same instant.
#[derive(Debug, Default)]
pub struct BoundedEventLoop {
    now_ms: u64,
    next_order: u64,
    pending: Vec<WebTask>,
    truncated: bool,
}

impl BoundedEventLoop {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn queue_microtask(&mut self, source: impl Into<String>) -> Result<(), &'static str> {
        self.enqueue(WebTaskKind::Microtask, source.into(), self.now_ms)
    }

    pub fn set_timeout(
        &mut self,
        source: impl Into<String>,
        delay_ms: u64,
    ) -> Result<(), &'static str> {
        let due_ms = self.now_ms.saturating_add(delay_ms.min(MAX_TIMER_DELAY_MS));
        self.enqueue(WebTaskKind::Timer, source.into(), due_ms)
    }

    pub fn advance_time(&mut self, elapsed_ms: u64) {
        self.now_ms = self.now_ms.saturating_add(elapsed_ms);
    }

    pub fn drain_ready(&mut self, budget: usize) -> Vec<WebTask> {
        let mut ready = Vec::new();
        while ready.len() < budget {
            let next = self
                .pending
                .iter()
                .enumerate()
                .filter(|(_, task)| task.due_ms <= self.now_ms)
                .min_by_key(|(_, task)| {
                    let priority = match task.kind {
                        WebTaskKind::Microtask => 0_u8,
                        WebTaskKind::Timer => 1_u8,
                    };
                    (priority, task.due_ms, task.order)
                })
                .map(|(index, _)| index);
            let Some(index) = next else {
                break;
            };
            ready.push(self.pending.remove(index));
        }
        ready
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn was_truncated(&self) -> bool {
        self.truncated
    }

    fn enqueue(
        &mut self,
        kind: WebTaskKind,
        source: String,
        due_ms: u64,
    ) -> Result<(), &'static str> {
        if source.len() > MAX_TASK_SOURCE_BYTES {
            self.truncated = true;
            return Err("Task source exceeds 64 KB");
        }
        if self.pending.len() >= MAX_PENDING_TASKS {
            self.truncated = true;
            return Err("Event-loop task budget exceeded");
        }
        let order = self.next_order;
        self.next_order = self.next_order.saturating_add(1);
        self.pending.push(WebTask {
            kind,
            source,
            due_ms,
            order,
        });
        Ok(())
    }
}

/// A JavaScript listener registered through the host bridge. The callback
/// value is captured inside the persistent engine and invoked on later
/// turns with a fresh execution budget.
#[derive(Debug, Clone)]
struct JsListener {
    id: u64,
    event_type: String,
    capture: bool,
    once: bool,
    callback: JsvValue,
}

#[derive(Debug, Clone)]
struct TimerEntry {
    callback: JsvValue,
    interval: bool,
    delay_ms: u64,
    due_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryState {
    pub url: String,
    pub title: String,
    pub state: Option<String>,
}

/// Bounded session-history view exposed to `window.history`. The browser UI
/// owns the persistent tab history; this record keeps the page-visible
/// behavior (push/replace/back/forward/popstate) deterministic and bounded.
#[derive(Debug, Clone, Default)]
pub struct PageHistory {
    entries: Vec<HistoryState>,
    index: usize,
    truncated: bool,
}

impl PageHistory {
    pub fn new(current_url: &str, title: &str) -> Self {
        Self {
            entries: vec![HistoryState {
                url: current_url.to_string(),
                title: title.to_string(),
                state: None,
            }],
            index: 0,
            truncated: false,
        }
    }

    pub fn push(&mut self, state: HistoryState) {
        if self.entries.len() > self.index + 1 {
            self.entries.truncate(self.index + 1);
        }
        if self.entries.len() >= MAX_HISTORY_ENTRIES {
            self.entries.remove(0);
            self.truncated = true;
        }
        self.entries.push(state);
        self.index = self.entries.len().saturating_sub(1);
    }

    pub fn replace(&mut self, state: HistoryState) {
        if self.entries.is_empty() {
            self.entries.push(state);
            self.index = 0;
            return;
        }
        self.entries[self.index] = state;
    }

    pub fn back(&mut self) -> Option<HistoryState> {
        self.go(-1)
    }

    pub fn forward(&mut self) -> Option<HistoryState> {
        self.go(1)
    }

    pub fn go(&mut self, delta: i32) -> Option<HistoryState> {
        let next = self.index as i64 + i64::from(delta);
        if next < 0 || next >= self.entries.len() as i64 {
            return None;
        }
        self.index = next as usize;
        Some(self.entries[self.index].clone())
    }

    pub fn length(&self) -> usize {
        self.entries.len()
    }

    pub fn current(&self) -> Option<&HistoryState> {
        self.entries.get(self.index)
    }

    pub fn was_truncated(&self) -> bool {
        self.truncated
    }
}

#[derive(Debug, Clone)]
struct HostEventRecord {
    event_type: String,
    bubbles: bool,
    cancelable: bool,
    detail: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct PendingDispatch {
    node: u64,
    event_type: String,
    detail: Option<String>,
}

/// All state that outlives a single script turn, owned by `PageRuntime`.
/// `RuntimeHost` borrows this for the duration of one execution so event
/// listeners, timers, storage and history stay consistent across tasks.
#[derive(Debug, Default)]
struct PageRuntimeState {
    listeners: BTreeMap<u64, Vec<JsListener>>,
    next_listener_id: u64,
    timers: BTreeMap<u64, TimerEntry>,
    next_timer_id: u64,
    now_ms: u64,
    storage: OriginStorage,
    history: PageHistory,
    current_url: String,
    active_event: Option<crate::live_dom::DomEvent>,
    events: BTreeMap<u64, HostEventRecord>,
    next_event_id: u64,
    form_data: BTreeMap<u64, Vec<(String, String)>>,
    next_form_id: u64,
    pending_dispatches: Vec<PendingDispatch>,
    /// 2D canvas contexts (context id → state), used by the Canvas2D host
    /// capability.
    canvas_contexts: BTreeMap<u64, CanvasState>,
    next_context_id: u64,
    /// Enabled host capabilities for this page.
    capabilities: std::collections::BTreeSet<HostCapability>,
    /// Host handle ↔ node identity maps persist across script turns so a
    /// handle captured by an earlier script stays valid for later callbacks.
    elements: BTreeMap<u64, ElementLocator>,
    handles: BTreeMap<NodeId, u64>,
    heap_roots: Vec<HeapHandle>,
    media_elements: BTreeMap<u64, HtmlMediaElement>,
    media_sources: BTreeMap<u64, MediaSource>,
    source_buffers: BTreeMap<u64, (u64, u32)>,
    media_attachments: BTreeMap<u64, u64>,
    next_element_id: u64,
    /// Phase 22: origin-partitioned persistent storage and real-time engines.
    pub indexeddb: crate::indexeddb::IndexedDBEngine,
    pub cache_storage: crate::cache_api::CacheStorage,
    pub service_workers: crate::service_worker::ServiceWorkerContainer,
    pub quota_manager: crate::storage_quota::StorageQuotaManager,
    idb_databases: BTreeMap<u64, String>,
    idb_stores: BTreeMap<u64, (String, String)>,
    cache_handles: BTreeMap<u64, String>,
    service_worker_registrations: BTreeMap<u64, String>,
    web_sockets: BTreeMap<u64, crate::realtime::WebSocketClient>,
    event_sources: BTreeMap<u64, crate::realtime::EventSourceClient>,
    broadcast_channels: BTreeMap<u64, crate::messaging::BroadcastChannel>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RuntimeReport {
    pub scripts_seen: usize,
    pub scripts_executed: usize,
    pub dom_mutations: usize,
    pub event_listeners: usize,
    pub scheduled_tasks: usize,
    pub media_elements: usize,
    pub media_sources: usize,
    pub source_buffers: usize,
    pub media_events: Vec<String>,
    pub realm_heap_bytes: usize,
    pub fetch_requests: Vec<String>,
    pub storage_writes: Vec<(String, String)>,
    pub console: Vec<String>,
    pub errors: Vec<String>,
    pub truncated: bool,
    /// Phase 21: script-driven history mutations (`pushState`/`replaceState`)
    /// in the form `kind url` so browser UI can mirror them into tab history.
    pub history_mutations: Vec<String>,
    /// Phase 21: cancelable form submissions that reached the browser-owned
    /// navigation boundary (`method action body-size`).
    pub submitted_forms: Vec<String>,
    /// Phase 21: number of timer callbacks executed through the page runtime.
    pub timers_fired: usize,
    /// Phase 21: number of JavaScript event listeners invoked by name.
    pub events_dispatched: Vec<String>,
    /// Phase 21: form validation failures that blocked a submission
    /// (`required field 'name' is empty` style).
    pub validation_errors: Vec<String>,
    /// Phase 22: successful platform API operations, capped for diagnostics.
    pub platform_operations: Vec<String>,
    /// Phase 25: successful, origin-noised Canvas 2D pixel readbacks.
    pub canvas_readbacks: usize,
}

#[derive(Debug, Clone)]
enum ElementLocator {
    Node(NodeId),
}

/// The document-scoped capability implementation passed to the interpreter.
/// It intentionally exposes only mutations that are currently connected to the
/// renderer and reports discovery-only fetch/storage operations for later web
/// API phases. No string scanning is involved: every operation arrives through
/// the JavaScript AST as a member get, property assignment or function call.
struct RuntimeHost<'a> {
    dom: &'a mut LiveDocument,
    report: &'a mut RuntimeReport,
    realm: &'a mut RuntimeRealm,
    state: &'a mut PageRuntimeState,
    operations: usize,
}

impl<'a> RuntimeHost<'a> {
    fn new(
        dom: &'a mut LiveDocument,
        report: &'a mut RuntimeReport,
        realm: &'a mut RuntimeRealm,
        state: &'a mut PageRuntimeState,
    ) -> Self {
        Self {
            dom,
            report,
            realm,
            state,
            operations: 0,
        }
    }

    fn charge(&mut self) -> Result<(), String> {
        self.operations = self.operations.saturating_add(1);
        if self.operations > MAX_HOST_OPERATIONS {
            self.report.truncated = true;
            return Err("Script host-operation budget exceeded".to_string());
        }
        Ok(())
    }

    fn argument_string(value: &JsvValue, label: &str) -> Result<String, String> {
        let output = value.to_display_string();
        if output.len() > 1024 * 1024 {
            return Err(format!("{} exceeds 1 MB", label));
        }
        Ok(output)
    }

    fn allocate_platform_handle(&mut self) -> Result<u64, String> {
        let handle = self.state.next_element_id;
        self.state.next_element_id = self
            .state
            .next_element_id
            .checked_add(1)
            .ok_or_else(|| "Platform object handle overflow".to_string())?;
        Ok(handle)
    }

    fn record_platform_operation(&mut self, operation: impl Into<String>) {
        if self.report.platform_operations.len() < 256 {
            self.report.platform_operations.push(operation.into());
        }
    }

    fn idb_key(value: &JsvValue) -> Result<crate::indexeddb::IDBKey, String> {
        match value {
            JsvValue::Number(number) if number.is_finite() => {
                Ok(crate::indexeddb::IDBKey::Number(*number))
            }
            JsvValue::String(string) if string.len() <= 64 * 1024 => {
                Ok(crate::indexeddb::IDBKey::String(string.clone()))
            }
            _ => Err("DataError: IndexedDB key must be a finite number or string".to_string()),
        }
    }

    fn jsv_to_json(value: &JsvValue) -> Result<serde_json::Value, String> {
        fn convert(
            value: &JsvValue,
            depth: usize,
            visited: &mut std::collections::BTreeSet<usize>,
        ) -> Result<serde_json::Value, String> {
            if depth > 64 {
                return Err("DataCloneError: maximum clone depth exceeded".to_string());
            }
            match value {
                JsvValue::Null | JsvValue::Undefined => Ok(serde_json::Value::Null),
                JsvValue::Boolean(value) => Ok(serde_json::Value::Bool(*value)),
                JsvValue::Number(value) => serde_json::Number::from_f64(*value)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| "DataCloneError: non-finite number".to_string()),
                JsvValue::String(value) => Ok(serde_json::Value::String(value.clone())),
                JsvValue::Array(array) => {
                    let identity = Rc::as_ptr(array) as usize;
                    if !visited.insert(identity) {
                        return Err("DataCloneError: cyclic value".to_string());
                    }
                    let result = array
                        .borrow()
                        .iter()
                        .map(|item| convert(item, depth + 1, visited))
                        .collect::<Result<Vec<_>, _>>()
                        .map(serde_json::Value::Array);
                    visited.remove(&identity);
                    result
                }
                JsvValue::Object(object) => {
                    let identity = Rc::as_ptr(object) as usize;
                    if !visited.insert(identity) {
                        return Err("DataCloneError: cyclic value".to_string());
                    }
                    let result = object
                        .borrow()
                        .properties
                        .iter()
                        .map(|(key, value)| {
                            convert(value, depth + 1, visited).map(|value| (key.clone(), value))
                        })
                        .collect::<Result<serde_json::Map<_, _>, _>>()
                        .map(serde_json::Value::Object);
                    visited.remove(&identity);
                    result
                }
                _ => Err("DataCloneError: value cannot be cloned".to_string()),
            }
        }
        convert(value, 0, &mut std::collections::BTreeSet::new())
    }

    fn json_to_jsv(value: &serde_json::Value) -> JsvValue {
        match value {
            serde_json::Value::Null => JsvValue::Null,
            serde_json::Value::Bool(value) => JsvValue::Boolean(*value),
            serde_json::Value::Number(value) => {
                JsvValue::Number(value.as_f64().unwrap_or_default())
            }
            serde_json::Value::String(value) => JsvValue::String(value.clone()),
            serde_json::Value::Array(values) => JsvValue::Array(Rc::new(RefCell::new(
                values.iter().map(Self::json_to_jsv).collect(),
            ))),
            serde_json::Value::Object(values) => {
                JsvValue::Object(Rc::new(RefCell::new(crate::javascript::JsvObject {
                    properties: values
                        .iter()
                        .map(|(key, value)| (key.clone(), Self::json_to_jsv(value)))
                        .collect(),
                    prototype: None,
                })))
            }
        }
    }

    fn update_platform_quota(&mut self) {
        let origin = self.state.indexeddb.origin.clone();
        let indexeddb_bytes = self
            .state
            .indexeddb
            .databases
            .values()
            .flat_map(|database| database.object_stores.values())
            .flat_map(|store| store.records.iter())
            .map(|record| record.value.len())
            .sum();
        let cache_bytes = self
            .state
            .cache_storage
            .caches
            .values()
            .flat_map(|cache| cache.entries.iter())
            .map(|entry| entry.response_body.len())
            .sum();
        self.state.quota_manager.update_usage(
            &origin,
            crate::storage_quota::StorageCategory::IndexedDB,
            indexeddb_bytes,
        );
        self.state.quota_manager.update_usage(
            &origin,
            crate::storage_quota::StorageCategory::CacheAPI,
            cache_bytes,
        );
    }

    fn element_for(&mut self, node: NodeId) -> Result<JsvValue, String> {
        self.dom
            .node(node)
            .ok_or_else(|| "InvalidStateError: node is no longer in the document".to_string())?;
        if let Some(handle) = self.state.handles.get(&node) {
            return Ok(JsvValue::HostObject(*handle));
        }
        let element_id = self.state.next_element_id;
        self.state.next_element_id = self
            .state
            .next_element_id
            .checked_add(1)
            .ok_or_else(|| "Host element identifier space exhausted".to_string())?;
        let media_element = self
            .dom
            .node(node)
            .and_then(|node| match &node.kind {
                crate::live_dom::LiveNodeKind::Element { tag, .. } => {
                    Some(matches!(tag.as_str(), "audio" | "video"))
                }
                crate::live_dom::LiveNodeKind::Text(_) => None,
            })
            .unwrap_or(false);
        let heap_object = self.realm.heap.allocate(if media_element {
            HostObjectKind::MediaElement
        } else {
            HostObjectKind::Element
        })?;
        self.realm.heap.set_property(
            heap_object,
            "kind",
            RuntimeValue::String("Element".to_string()),
        )?;
        self.state.heap_roots.push(heap_object);
        self.state
            .elements
            .insert(element_id, ElementLocator::Node(node));
        self.state.handles.insert(node, element_id);
        if media_element {
            self.state
                .media_elements
                .insert(element_id, HtmlMediaElement::new());
            self.report.media_elements = self.report.media_elements.saturating_add(1);
        }
        Ok(JsvValue::HostObject(element_id))
    }

    fn allocate_media_source(&mut self) -> Result<JsvValue, String> {
        let id = self.next_host_id()?;
        let handle = self.realm.heap.allocate(HostObjectKind::MediaSource)?;
        self.state.heap_roots.push(handle);
        let mut source = MediaSource::new();
        source.open()?;
        self.state.media_sources.insert(id, source);
        self.report.media_sources = self.report.media_sources.saturating_add(1);
        Ok(JsvValue::HostObject(id))
    }

    fn allocate_source_buffer(&mut self, source: u64, buffer: u32) -> Result<JsvValue, String> {
        let id = self.next_host_id()?;
        let handle = self.realm.heap.allocate(HostObjectKind::SourceBuffer)?;
        self.state.heap_roots.push(handle);
        self.state.source_buffers.insert(id, (source, buffer));
        self.report.source_buffers = self.report.source_buffers.saturating_add(1);
        Ok(JsvValue::HostObject(id))
    }

    fn next_host_id(&mut self) -> Result<u64, String> {
        let id = self.state.next_element_id;
        self.state.next_element_id = self
            .state
            .next_element_id
            .checked_add(1)
            .ok_or_else(|| "Host object identifier space exhausted".to_string())?;
        Ok(id)
    }

    fn argument_bytes(value: &JsvValue) -> Result<Vec<u8>, String> {
        const MAX_APPEND_BYTES: usize = 16 * 1024 * 1024;
        let JsvValue::Array(values) = value else {
            return Err("TypeError: appendBuffer requires a byte array".to_string());
        };
        let values = values.borrow();
        if values.len() > MAX_APPEND_BYTES {
            return Err("SourceBuffer append exceeds 16 MB".to_string());
        }
        values
            .iter()
            .map(|value| {
                value
                    .as_number()
                    .filter(|value| value.is_finite() && value.fract() == 0.0)
                    .and_then(|value| u8::try_from(value as i64).ok())
                    .ok_or_else(|| "SourceBuffer byte array contains an invalid value".to_string())
            })
            .collect()
    }

    fn collect_media_events(&mut self, object: u64) {
        let Some(media) = self.state.media_elements.get_mut(&object) else {
            return;
        };
        for event in media.drain_events() {
            if self.report.media_events.len() >= 512 {
                self.report.media_events.remove(0);
            }
            self.report
                .media_events
                .push(format!("{object}:{}", media_event_name(event)));
        }
    }

    fn locator(&self, object: u64) -> Result<NodeId, String> {
        match self
            .state
            .elements
            .get(&object)
            .cloned()
            .ok_or_else(|| "SecurityError: unknown host object".to_string())?
        {
            ElementLocator::Node(node) => Ok(node),
        }
    }

    fn element_text(&self, object: u64) -> Result<String, String> {
        self.dom.text_content(self.locator(object)?)
    }

    /// Install the event snapshot the page script sees as the `Event` object
    /// during one callback invocation.
    fn begin_event(&mut self, node: u64, event: &crate::live_dom::DomEvent) {
        let mut snapshot = event.clone();
        snapshot.current_target = Some(node);
        self.state.active_event = Some(snapshot);
    }

    /// Merge listener-side mutations (preventDefault/stopPropagation) back
    /// into the in-flight event after a callback returns.
    fn finish_event(&mut self, event: &mut crate::live_dom::DomEvent) {
        if let Some(active) = self.state.active_event.take() {
            event.default_prevented = active.default_prevented;
            event.propagation_stopped = active.propagation_stopped;
            event.immediate_propagation_stopped = active.immediate_propagation_stopped;
        }
    }

    fn register_listener(
        &mut self,
        object: u64,
        event_type: &str,
        capture: bool,
        once: bool,
        callback: JsvValue,
    ) -> Result<(), String> {
        // Listeners are keyed by stable DOM node identity so the dispatch
        // pass can find them from the event path without host handles.
        // Window listeners use a sentinel that can never collide with a NodeId.
        let key = if object == HOST_WINDOW {
            LISTENER_WINDOW
        } else if object == HOST_DOCUMENT {
            self.dom.root()
        } else {
            self.locator(object)?
        };
        let entries = self.state.listeners.entry(key).or_default();
        if entries.len() >= MAX_JS_LISTENERS_PER_NODE {
            return Err("QuotaExceededError: JavaScript listener budget exceeded".to_string());
        }
        let id = self.state.next_listener_id;
        self.state.next_listener_id = self
            .state
            .next_listener_id
            .checked_add(1)
            .ok_or_else(|| "Listener identifier space exhausted".to_string())?;
        entries.push(JsListener {
            id,
            event_type: event_type.to_string(),
            capture,
            once,
            callback,
        });
        self.report.event_listeners = self.report.event_listeners.saturating_add(1);
        Ok(())
    }

    fn remove_listener(&mut self, object: u64, event_type: &str, callback: &JsvValue) -> bool {
        let key = if object == HOST_WINDOW {
            LISTENER_WINDOW
        } else if object == HOST_DOCUMENT {
            self.dom.root()
        } else {
            match self.locator(object) {
                Ok(node) => node,
                Err(_) => return false,
            }
        };
        let Some(entries) = self.state.listeners.get_mut(&key) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|listener| {
            listener.event_type != event_type || !listener.callback.function_identity_eq(callback)
        });
        entries.len() != before
    }

    fn set_timer(
        &mut self,
        callback: JsvValue,
        delay_ms: u64,
        interval: bool,
    ) -> Result<u64, String> {
        if self.state.timers.len() >= MAX_ACTIVE_TIMERS {
            return Err("QuotaExceededError: timer budget exceeded".to_string());
        }
        let delay = delay_ms.min(MAX_TIMER_DELAY_MS);
        let id = self.state.next_timer_id;
        self.state.next_timer_id = self
            .state
            .next_timer_id
            .checked_add(1)
            .ok_or_else(|| "Timer identifier space exhausted".to_string())?;
        let due_ms = self.state.now_ms.saturating_add(delay);
        self.state.timers.insert(
            id,
            TimerEntry {
                callback,
                interval,
                delay_ms: delay,
                due_ms,
            },
        );
        Ok(id)
    }

    fn clear_timer(&mut self, id: u64) {
        self.state.timers.remove(&id);
    }

    fn allocate_event(
        &mut self,
        event_type: String,
        bubbles: bool,
        cancelable: bool,
        detail: Option<String>,
    ) -> Result<u64, String> {
        if self.state.events.len() >= MAX_EVENT_RECORDS {
            return Err("QuotaExceededError: event object budget exceeded".to_string());
        }
        let id = self.state.next_event_id;
        self.state.next_event_id = self
            .state
            .next_event_id
            .checked_add(1)
            .ok_or_else(|| "Event identifier space exhausted".to_string())?;
        self.state.events.insert(
            id,
            HostEventRecord {
                event_type,
                bubbles,
                cancelable,
                detail,
            },
        );
        Ok(id)
    }

    fn allocate_form_data(&mut self, form: NodeId) -> Result<u64, String> {
        if self.state.form_data.len() >= MAX_EVENT_RECORDS {
            return Err("QuotaExceededError: FormData budget exceeded".to_string());
        }
        let mut entries = Vec::new();
        for node in self.dom.document_order() {
            if self.is_descendant_of(node, form) || node == form {
                if let Some(name) = self.dom.get_attribute(node, "name") {
                    let tag = self.element_tag(node).unwrap_or_default();
                    if matches!(tag.as_str(), "input" | "select" | "textarea") {
                        let value = self.control_value(node);
                        if let Some(value) = value {
                            entries.push((name.to_string(), value));
                            if entries.len() >= MAX_FORM_ENTRIES {
                                break;
                            }
                        }
                    }
                }
            }
        }
        let id = self.state.next_form_id;
        self.state.next_form_id = self
            .state
            .next_form_id
            .checked_add(1)
            .ok_or_else(|| "FormData identifier space exhausted".to_string())?;
        self.state.form_data.insert(id, entries);
        Ok(id)
    }

    fn control_value(&self, node: NodeId) -> Option<String> {
        match self.element_tag(node).as_deref() {
            Some("textarea") => self.dom.text_content(node).ok(),
            Some("select") => self.selected_option_value(node),
            Some("input") => match self.dom.get_attribute(node, "type").unwrap_or("text") {
                "checkbox" | "radio" => {
                    self.dom.get_attribute(node, "checked").is_some().then(|| {
                        self.dom
                            .get_attribute(node, "value")
                            .unwrap_or("on")
                            .to_string()
                    })
                }
                "submit" | "button" | "reset" => None,
                _ => Some(
                    self.dom
                        .get_attribute(node, "value")
                        .unwrap_or("")
                        .to_string(),
                ),
            },
            _ => None,
        }
    }

    fn selected_option_value(&self, node: NodeId) -> Option<String> {
        let mut first: Option<String> = None;
        let selected = self
            .dom
            .get_attribute(node, "value")
            .map(str::to_string)
            .or_else(|| {
                for child in self.dom.document_order() {
                    if self.element_tag(child).as_deref() == Some("option")
                        && self.is_descendant_of(child, node)
                    {
                        let value = self
                            .dom
                            .get_attribute(child, "value")
                            .map(str::to_string)
                            .or_else(|| self.dom.text_content(child).ok());
                        if first.is_none() {
                            first = value.clone();
                        }
                        if self.dom.get_attribute(child, "selected").is_some() {
                            return value;
                        }
                    }
                }
                first
            });
        selected
    }

    /// Assign `element.value` on a `<select>`: clear any prior `selected`
    /// attributes and mark the matching option, falling back to appending a
    /// new option when no existing option matches (bounded by the node
    /// budget). Unmatched assignments with no budget left fail closed.
    fn select_value(&mut self, node: NodeId, value: &str) -> Result<(), String> {
        let mut matched = false;
        for child in self.dom.document_order() {
            if self.element_tag(child).as_deref() == Some("option")
                && self.is_descendant_of(child, node)
            {
                let option_value = self
                    .dom
                    .get_attribute(child, "value")
                    .map(str::to_string)
                    .or_else(|| self.dom.text_content(child).ok())
                    .unwrap_or_default();
                if option_value == value {
                    self.dom.set_attribute(child, "selected", "")?;
                    matched = true;
                } else {
                    self.dom.remove_attribute(child, "selected")?;
                }
            }
        }
        if !matched {
            let option = self.dom.create_element("option")?;
            self.dom.set_attribute(option, "value", value)?;
            self.dom.append_child(node, option)?;
            self.dom.set_attribute(option, "selected", "")?;
        }
        Ok(())
    }

    fn is_descendant_of(&self, candidate: NodeId, ancestor: NodeId) -> bool {
        let mut current = self.dom.node(candidate).and_then(|node| node.parent);
        while let Some(id) = current {
            if id == ancestor {
                return true;
            }
            current = self.dom.node(id).and_then(|node| node.parent);
        }
        false
    }

    fn element_tag(&self, node: NodeId) -> Option<String> {
        match &self.dom.node(node)?.kind {
            crate::live_dom::LiveNodeKind::Element { tag, .. } => Some(tag.clone()),
            crate::live_dom::LiveNodeKind::Text(_) => None,
        }
    }

    fn option_bool(value: Option<&JsvValue>, property: &str) -> bool {
        value
            .and_then(|value| match value {
                JsvValue::Object(object) => object_property_value(object, property),
                _ => None,
            })
            .map(|value| value.is_truthy())
            .unwrap_or(false)
    }

    fn option_string(value: Option<&JsvValue>, property: &str) -> Option<String> {
        value
            .and_then(|value| match value {
                JsvValue::Object(object) => object_property_value(object, property),
                _ => None,
            })
            .map(|value| {
                let display = value.to_display_string();
                display.chars().take(64 * 1024).collect()
            })
    }

    fn queue_pending_dispatch(&mut self, node: u64, event_type: &str, detail: Option<String>) {
        if self.state.pending_dispatches.len() >= MAX_PENDING_TASKS {
            self.report.truncated = true;
            return;
        }
        self.state.pending_dispatches.push(PendingDispatch {
            node,
            event_type: event_type.to_string(),
            detail,
        });
    }

    /// Resolve a candidate URL against the current document URL. Only
    /// same-origin candidates resolve; cross-origin navigation is recorded
    /// but never performed by the page runtime.
    fn resolved_navigation(&self, candidate: &str) -> Option<String> {
        let base = url::Url::parse(&self.state.current_url).ok()?;
        if !matches!(base.scheme(), "http" | "https") {
            return None;
        }
        let resolved = base.join(candidate).ok()?;
        (resolved.scheme() == base.scheme()
            && resolved.host_str() == base.host_str()
            && resolved.port_or_known_default() == base.port_or_known_default())
        .then(|| String::from(resolved))
    }

    fn record_history_mutation(&mut self, kind: &str, url: &str) {
        if self.report.history_mutations.len() < 128 {
            self.report
                .history_mutations
                .push(format!("{} {}", kind, url));
        }
    }

    fn location_part(&self, property: &str) -> Result<String, String> {
        let url = url::Url::parse(&self.state.current_url)
            .map_err(|_| "SecurityError: location is unavailable".to_string())?;
        let host = url.host_str().unwrap_or("").to_string();
        let port = url
            .port()
            .map(|port| format!(":{}", port))
            .unwrap_or_default();
        match property {
            "href" => Ok(self.state.current_url.clone()),
            "protocol" => Ok(format!("{}:", url.scheme())),
            "host" => Ok(format!("{}{}", host, port)),
            "hostname" => Ok(host),
            "port" => Ok(url.port().map(|port| port.to_string()).unwrap_or_default()),
            "origin" => Ok(format!("{}://{}{}", url.scheme(), host, port)),
            "pathname" => Ok(url.path().to_string()),
            "search" => Ok(url
                .query()
                .map(|query| format!("?{}", query))
                .unwrap_or_default()),
            "hash" => Ok(url
                .fragment()
                .map(|fragment| format!("#{}", fragment))
                .unwrap_or_default()),
            _ => Err(format!(
                "TypeError: location.{} is not implemented",
                property
            )),
        }
    }

    fn event_property(&mut self, property: &str) -> Result<JsvValue, String> {
        let event = self
            .state
            .active_event
            .as_ref()
            .ok_or_else(|| "InvalidStateError: no event is being dispatched".to_string())?;
        match property {
            "type" => Ok(JsvValue::String(event.event_type.clone())),
            "detail" => Ok(event
                .detail
                .clone()
                .map(JsvValue::String)
                .unwrap_or(JsvValue::Undefined)),
            "target" => {
                let current = event.current_target.unwrap_or(event.target);
                self.element_for(self.dom.effective_target(event.target, current))
            }
            "currentTarget" => match event.current_target {
                Some(node) => self.element_for(node),
                None => Ok(JsvValue::Null),
            },
            "eventPhase" => Ok(JsvValue::Number(match event.phase {
                crate::live_dom::EventPhase::None => 0.0,
                crate::live_dom::EventPhase::Capturing => 1.0,
                crate::live_dom::EventPhase::AtTarget => 2.0,
                crate::live_dom::EventPhase::Bubbling => 3.0,
            })),
            _ => Err(format!("TypeError: event.{} is not implemented", property)),
        }
    }

    fn event_boolean_property(&self, property: &str) -> Result<bool, String> {
        let event = self
            .state
            .active_event
            .as_ref()
            .ok_or_else(|| "InvalidStateError: no event is being dispatched".to_string())?;
        match property {
            "bubbles" => Ok(event.bubbles),
            "cancelable" => Ok(event.cancelable),
            "defaultPrevented" => Ok(event.default_prevented),
            _ => Err(format!("TypeError: event.{} is not implemented", property)),
        }
    }

    /// Apply a completed history traversal: update the visible URL, record
    /// the mutation and dispatch `popstate` through the combined dispatch
    /// queue so window listeners observe the new state.
    fn after_history_move(&mut self, entry: Option<HistoryState>) -> Result<(), String> {
        if let Some(entry) = entry {
            let previous = self.state.current_url.clone();
            if entry.url != previous {
                self.state.current_url = entry.url.clone();
                self.record_history_mutation("popstate", &entry.url);
            }
            let detail = entry
                .state
                .unwrap_or_default()
                .chars()
                .take(64 * 1024)
                .collect::<String>();
            let mut event = crate::live_dom::DomEvent::new("popstate", self.dom.root());
            event.detail = Some(detail);
            self.queue_pending_dispatch(LISTENER_WINDOW, "popstate", event.detail.clone());
        }
        Ok(())
    }
}

fn object_property_value(object: &crate::javascript::JsvObjectRef, name: &str) -> Option<JsvValue> {
    object.borrow().properties.get(name).cloned()
}

impl JsvHost for RuntimeHost<'_> {
    fn get_property(&mut self, object: u64, property: &str) -> Result<JsvValue, String> {
        self.charge()?;
        if let Some(media) = self.state.media_elements.get(&object) {
            let controls = media.controls_state();
            return match property {
                "play" | "pause" | "load" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "paused" => Ok(JsvValue::Boolean(controls.paused)),
                "muted" => Ok(JsvValue::Boolean(controls.muted)),
                "seeking" => Ok(JsvValue::Boolean(controls.seeking)),
                "ended" => Ok(JsvValue::Boolean(controls.ended)),
                "currentTime" => Ok(JsvValue::Number(controls.current_time_seconds)),
                "duration" => Ok(JsvValue::Number(
                    controls.duration_seconds.unwrap_or(f64::NAN),
                )),
                "volume" => Ok(JsvValue::Number(controls.volume)),
                "playbackRate" => Ok(JsvValue::Number(controls.playback_rate)),
                "readyState" => Ok(JsvValue::Number(media.ready_state() as u8 as f64)),
                "networkState" => Ok(JsvValue::Number(media.network_state() as u8 as f64)),
                "srcObject" => Ok(self
                    .state
                    .media_attachments
                    .get(&object)
                    .copied()
                    .map(JsvValue::HostObject)
                    .unwrap_or(JsvValue::Null)),
                "src" | "currentSrc" => {
                    let node = self.locator(object)?;
                    Ok(JsvValue::String(
                        self.dom
                            .get_attribute(node, "src")
                            .unwrap_or("")
                            .to_string(),
                    ))
                }
                "error" => Ok(JsvValue::Null),
                "buffered" => {
                    let ranges = self
                        .state
                        .media_attachments
                        .get(&object)
                        .and_then(|source| self.state.media_sources.get(source))
                        .map(crate::mse::MediaSource::buffered_ranges)
                        .unwrap_or_default();
                    let values = ranges
                        .iter()
                        .map(|range| {
                            JsvValue::Array(Rc::new(RefCell::new(vec![
                                JsvValue::Number(range.start_us as f64 / 1_000_000.0),
                                JsvValue::Number(range.end_us as f64 / 1_000_000.0),
                            ])))
                        })
                        .collect();
                    Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
                }
                "playsInline" | "autoplay" | "loop" => {
                    let node = self.locator(object)?;
                    Ok(JsvValue::Boolean(
                        self.dom.get_attribute(node, property).is_some(),
                    ))
                }
                "canPlayType" => Ok(JsvValue::HostFunction(object, property.to_string())),
                _ => Err(format!(
                    "TypeError: HTMLMediaElement.{} is not implemented",
                    property
                )),
            };
        }
        if let Some(source) = self.state.media_sources.get(&object) {
            return match property {
                "addSourceBuffer" | "endOfStream" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "readyState" => Ok(JsvValue::String(
                    format!("{:?}", source.ready_state()).to_ascii_lowercase(),
                )),
                "duration" => Ok(JsvValue::Number(
                    source
                        .duration_us()
                        .map(|duration| duration as f64 / 1_000_000.0)
                        .unwrap_or(f64::NAN),
                )),
                _ => Err(format!(
                    "TypeError: MediaSource.{} is not implemented",
                    property
                )),
            };
        }
        if let Some((source, buffer)) = self.state.source_buffers.get(&object).copied() {
            let source_buffer = self
                .state
                .media_sources
                .get(&source)
                .and_then(|source| source.source_buffer(buffer))
                .ok_or_else(|| "InvalidStateError: SourceBuffer is detached".to_string())?;
            return match property {
                "appendBuffer" | "abort" | "remove" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "updating" => Ok(JsvValue::Boolean(source_buffer.updating())),
                "buffered" => {
                    let ranges = source_buffer.buffered();
                    let values = ranges
                        .iter()
                        .map(|range| {
                            JsvValue::Array(Rc::new(RefCell::new(vec![
                                JsvValue::Number(range.start_us as f64 / 1_000_000.0),
                                JsvValue::Number(range.end_us as f64 / 1_000_000.0),
                            ])))
                        })
                        .collect();
                    Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
                }
                "timestampOffset" => {
                    let offset = source_buffer.timestamp_offset_us();
                    Ok(JsvValue::Number(offset as f64 / 1_000_000.0))
                }
                _ => Err(format!(
                    "TypeError: SourceBuffer.{} is not implemented",
                    property
                )),
            };
        }
        if let Some(database_name) = self.state.idb_databases.get(&object) {
            let database = self
                .state
                .indexeddb
                .databases
                .get(database_name)
                .ok_or_else(|| "InvalidStateError: IndexedDB database is closed".to_string())?;
            return match property {
                "name" => Ok(JsvValue::String(database.name.clone())),
                "version" => Ok(JsvValue::Number(database.version as f64)),
                "objectStoreNames" => Ok(JsvValue::Array(Rc::new(RefCell::new(
                    database
                        .object_stores
                        .keys()
                        .cloned()
                        .map(JsvValue::String)
                        .collect(),
                )))),
                "createObjectStore" | "deleteObjectStore" | "objectStore" | "close" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                _ => Err(format!(
                    "TypeError: IDBDatabase.{property} is not implemented"
                )),
            };
        }
        if let Some((database_name, store_name)) = self.state.idb_stores.get(&object) {
            let store = self
                .state
                .indexeddb
                .databases
                .get(database_name)
                .and_then(|database| database.object_stores.get(store_name))
                .ok_or_else(|| {
                    "InvalidStateError: IndexedDB object store is detached".to_string()
                })?;
            return match property {
                "name" => Ok(JsvValue::String(store.name.clone())),
                "keyPath" => Ok(store
                    .key_path
                    .clone()
                    .map(JsvValue::String)
                    .unwrap_or(JsvValue::Null)),
                "autoIncrement" => Ok(JsvValue::Boolean(store.auto_increment)),
                "put" | "add" | "get" | "delete" | "clear" | "count" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                _ => Err(format!(
                    "TypeError: IDBObjectStore.{property} is not implemented"
                )),
            };
        }
        if let Some(cache_name) = self.state.cache_handles.get(&object) {
            if !self.state.cache_storage.has(cache_name) {
                return Err("InvalidStateError: Cache is detached".to_string());
            }
            return match property {
                "put" | "match" | "delete" | "keys" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                _ => Err(format!("TypeError: Cache.{property} is not implemented")),
            };
        }
        if let Some(scope) = self.state.service_worker_registrations.get(&object) {
            let registration = self
                .state
                .service_workers
                .registrations
                .get(scope)
                .ok_or_else(|| "InvalidStateError: service worker is unregistered".to_string())?;
            return match property {
                "scope" => Ok(JsvValue::String(registration.scope.clone())),
                "active" => Ok(registration
                    .active_worker
                    .clone()
                    .map(JsvValue::String)
                    .unwrap_or(JsvValue::Null)),
                "unregister" => Ok(JsvValue::HostFunction(object, property.to_string())),
                _ => Err(format!(
                    "TypeError: ServiceWorkerRegistration.{property} is not implemented"
                )),
            };
        }
        if let Some(socket) = self.state.web_sockets.get_mut(&object) {
            socket.pump_transport();
            return match property {
                "url" => Ok(JsvValue::String(socket.url.clone())),
                "protocol" => Ok(JsvValue::String(socket.protocol.clone())),
                "readyState" => Ok(JsvValue::Number(socket.ready_state as u16 as f64)),
                "bufferedAmount" => Ok(JsvValue::Number(socket.buffered_amount as f64)),
                "send" | "close" | "poll" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                _ => Err(format!(
                    "TypeError: WebSocket.{property} is not implemented"
                )),
            };
        }
        if let Some(source) = self.state.event_sources.get_mut(&object) {
            source.pump_transport();
            return match property {
                "url" => Ok(JsvValue::String(source.url.clone())),
                "readyState" => Ok(JsvValue::Number(source.ready_state as u16 as f64)),
                "close" | "poll" => Ok(JsvValue::HostFunction(object, property.to_string())),
                _ => Err(format!(
                    "TypeError: EventSource.{property} is not implemented"
                )),
            };
        }
        if self.state.broadcast_channels.contains_key(&object) {
            return match property {
                "name" => Ok(JsvValue::String(
                    self.state
                        .broadcast_channels
                        .get(&object)
                        .expect("channel existence checked")
                        .name
                        .clone(),
                )),
                "postMessage" | "close" | "poll" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                _ => Err(format!(
                    "TypeError: BroadcastChannel.{property} is not implemented"
                )),
            };
        }
        match object {
            HOST_DOCUMENT => match property {
                "getElementById" | "querySelector" | "createElement" | "createTextNode" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "activeElement" => self
                    .dom
                    .focused()
                    .map_or(Ok(JsvValue::Null), |node| self.element_for(node)),
                "URL" => Ok(JsvValue::String(self.state.current_url.clone())),
                _ => Err(format!(
                    "TypeError: document.{} is not implemented",
                    property
                )),
            },
            HOST_WINDOW => match property {
                "document" => Ok(JsvValue::HostObject(HOST_DOCUMENT)),
                "localStorage" => Ok(JsvValue::HostObject(HOST_LOCAL_STORAGE)),
                "indexedDB" => Ok(JsvValue::HostObject(HOST_INDEXED_DB)),
                "caches" => Ok(JsvValue::HostObject(HOST_CACHE_STORAGE)),
                "navigator" => Ok(JsvValue::HostObject(HOST_NAVIGATOR)),
                "history" => Ok(JsvValue::HostObject(HOST_HISTORY)),
                "location" => Ok(JsvValue::HostObject(HOST_LOCATION)),
                "fetch" => Ok(JsvValue::HostFunction(object, property.to_string())),
                "MediaSource" => Ok(JsvValue::HostFunction(object, property.to_string())),
                "Event" | "CustomEvent" | "FormData" | "WebSocket" | "EventSource"
                | "BroadcastChannel" | "structuredClone" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "setTimeout" | "setInterval" | "clearTimeout" | "clearInterval" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "addEventListener" | "removeEventListener" | "dispatchEvent" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "innerWidth" | "innerHeight" => Ok(JsvValue::Number(0.0)),
                "name" => Ok(JsvValue::String(String::new())),
                _ => Err(format!("TypeError: window.{} is not implemented", property)),
            },
            HOST_LOCAL_STORAGE => match property {
                "getItem" | "setItem" | "removeItem" | "key" | "clear" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "length" => Ok(JsvValue::Number(self.state.storage.len() as f64)),
                _ => Err(format!(
                    "TypeError: localStorage.{} is not implemented",
                    property
                )),
            },
            HOST_INDEXED_DB => match property {
                "open" | "deleteDatabase" | "databases" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                _ => Err(format!(
                    "TypeError: indexedDB.{property} is not implemented"
                )),
            },
            HOST_CACHE_STORAGE => match property {
                "open" | "has" | "delete" | "keys" | "match" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                _ => Err(format!("TypeError: caches.{property} is not implemented")),
            },
            HOST_NAVIGATOR => match property {
                "serviceWorker" => Ok(JsvValue::HostObject(HOST_SERVICE_WORKER)),
                _ => Err(format!(
                    "TypeError: navigator.{property} is not implemented"
                )),
            },
            HOST_SERVICE_WORKER => match property {
                "register" | "getRegistration" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "controller" => Ok(self
                    .state
                    .service_workers
                    .get_registration(&self.state.current_url)
                    .and_then(|registration| registration.active_worker.clone())
                    .map(JsvValue::String)
                    .unwrap_or(JsvValue::Null)),
                _ => Err(format!(
                    "TypeError: navigator.serviceWorker.{property} is not implemented"
                )),
            },
            HOST_HISTORY => match property {
                "pushState" | "replaceState" | "back" | "forward" | "go" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                "length" => Ok(JsvValue::Number(self.state.history.length() as f64)),
                "state" => Ok(self
                    .state
                    .history
                    .current()
                    .and_then(|entry| entry.state.clone())
                    .map(JsvValue::String)
                    .unwrap_or(JsvValue::Null)),
                _ => Err(format!(
                    "TypeError: history.{} is not implemented",
                    property
                )),
            },
            HOST_LOCATION => match property {
                "href" | "pathname" | "search" | "hash" | "host" | "origin" | "protocol"
                | "hostname" | "port" => self.location_part(property).map(JsvValue::String),
                "assign" | "replace" | "reload" => {
                    Ok(JsvValue::HostFunction(object, property.to_string()))
                }
                _ => Err(format!(
                    "TypeError: location.{} is not implemented",
                    property
                )),
            },
            HOST_EVENT => match property {
                "type" | "detail" | "target" | "currentTarget" | "eventPhase" => {
                    self.event_property(property)
                }
                "bubbles" | "cancelable" | "defaultPrevented" => {
                    self.event_boolean_property(property).map(JsvValue::Boolean)
                }
                "preventDefault"
                | "stopPropagation"
                | "stopImmediatePropagation"
                | "composedPath" => Ok(JsvValue::HostFunction(object, property.to_string())),
                _ => Err(format!("TypeError: event.{} is not implemented", property)),
            },
            _ if self.state.canvas_contexts.contains_key(&object) => {
                let state = self
                    .state
                    .canvas_contexts
                    .get(&object)
                    .expect("canvas context existence checked");
                match property {
                    "fillStyle" => Ok(JsvValue::String(state.fill.clone())),
                    "strokeStyle" => Ok(JsvValue::String(state.stroke.clone())),
                    "font" => Ok(JsvValue::String(state.font.clone())),
                    "canvas" => self.element_for(state.canvas),
                    "fillRect" | "strokeRect" | "clearRect" | "fillText" | "getImageData" => {
                        Ok(JsvValue::HostFunction(object, property.to_string()))
                    }
                    _ => Err(format!(
                        "TypeError: CanvasRenderingContext2D.{} is not implemented",
                        property
                    )),
                }
            }
            HOST_FORM_DATA => {
                let entries = self
                    .state
                    .form_data
                    .get(&object)
                    .cloned()
                    .ok_or_else(|| "InvalidStateError: FormData is detached".to_string())?;
                match property {
                    "get" | "append" | "delete" | "has" => {
                        Ok(JsvValue::HostFunction(object, property.to_string()))
                    }
                    "entries" => {
                        let values = entries
                            .iter()
                            .map(|(name, value)| {
                                JsvValue::Array(Rc::new(RefCell::new(vec![
                                    JsvValue::String(name.clone()),
                                    JsvValue::String(value.clone()),
                                ])))
                            })
                            .collect();
                        Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
                    }
                    _ => Err(format!(
                        "TypeError: FormData.{} is not implemented",
                        property
                    )),
                }
            }
            element => match property {
                "textContent" | "innerText" => Ok(JsvValue::String(self.element_text(element)?)),
                "id" | "className" => Ok(JsvValue::String(
                    self.dom
                        .get_attribute(
                            self.locator(element)?,
                            match property {
                                "id" => "id",
                                _ => "class",
                            },
                        )
                        .unwrap_or("")
                        .to_string(),
                )),
                "value" => {
                    let node = self.locator(element)?;
                    match self.element_tag(node).as_deref() {
                        Some("select") => Ok(self
                            .selected_option_value(node)
                            .map(JsvValue::String)
                            .unwrap_or(JsvValue::String(String::new()))),
                        Some("textarea") => Ok(JsvValue::String(
                            self.dom.text_content(node).unwrap_or_default(),
                        )),
                        _ => Ok(JsvValue::String(
                            self.dom
                                .get_attribute(node, "value")
                                .unwrap_or("")
                                .to_string(),
                        )),
                    }
                }
                "checked" => Ok(JsvValue::Boolean(
                    self.dom
                        .get_attribute(self.locator(element)?, "checked")
                        .is_some(),
                )),
                "tagName" => Ok(JsvValue::String(
                    self.dom
                        .node(self.locator(element)?)
                        .and_then(|node| match &node.kind {
                            crate::live_dom::LiveNodeKind::Element { tag, .. } => {
                                Some(tag.to_ascii_uppercase())
                            }
                            crate::live_dom::LiveNodeKind::Text(_) => None,
                        })
                        .ok_or_else(|| "InvalidStateError: node is not an element".to_string())?,
                )),
                "type" => Ok(JsvValue::String(
                    self.dom
                        .get_attribute(self.locator(element)?, "type")
                        .unwrap_or(match self.element_tag(self.locator(element)?).as_deref() {
                            Some("input") => "text",
                            Some("button") => "submit",
                            _ => "",
                        })
                        .to_string(),
                )),
                "name" | "disabled" | "hidden" | "placeholder" | "form" | "action" | "method"
                | "href" | "src" | "title" | "tabIndex" => Ok(JsvValue::String(
                    self.dom
                        .get_attribute(
                            self.locator(element)?,
                            match property {
                                "tabIndex" => "tabindex",
                                "className" => "class",
                                _ => property,
                            },
                        )
                        .unwrap_or("")
                        .to_string(),
                )),
                "shadowRoot" => {
                    let node = self.locator(element)?;
                    match self.dom.shadow_root(node) {
                        Some(root) => self.element_for(root),
                        None => Ok(JsvValue::Null),
                    }
                }
                "innerHTML" => Ok(JsvValue::String(self.element_text(element)?)),
                "getContext" => {
                    let node = self.locator(element)?;
                    if self.element_tag(node).as_deref() == Some("canvas")
                        && self.state.capabilities.contains(&HostCapability::Canvas2D)
                    {
                        Ok(JsvValue::HostFunction(element, property.to_string()))
                    } else {
                        Ok(JsvValue::Null)
                    }
                }
                "setAttribute"
                | "getAttribute"
                | "removeAttribute"
                | "appendChild"
                | "removeChild"
                | "querySelector"
                | "focus"
                | "click"
                | "addEventListener"
                | "removeEventListener"
                | "dispatchEvent"
                | "submit"
                | "attachShadow" => Ok(JsvValue::HostFunction(element, property.to_string())),
                _ => Err(format!(
                    "TypeError: element.{} is not implemented",
                    property
                )),
            },
        }
    }

    fn set_property(
        &mut self,
        object: u64,
        property: &str,
        value: JsvValue,
    ) -> Result<JsvValue, String> {
        self.charge()?;
        if self.state.media_elements.contains_key(&object) {
            let result = match property {
                "srcObject" => {
                    let JsvValue::HostObject(source) = value else {
                        return Err("TypeError: srcObject requires a MediaSource".to_string());
                    };
                    if !self.state.media_sources.contains_key(&source) {
                        return Err("TypeError: srcObject requires a MediaSource".to_string());
                    }
                    self.state.media_attachments.insert(object, source);
                    JsvValue::HostObject(source)
                }
                "currentTime" => {
                    let seconds = value
                        .as_number()
                        .ok_or_else(|| "TypeError: currentTime requires a number".to_string())?;
                    self.state
                        .media_elements
                        .get_mut(&object)
                        .expect("media object existence checked")
                        .seek(seconds)?;
                    value
                }
                "volume" => {
                    let volume = value
                        .as_number()
                        .ok_or_else(|| "TypeError: volume requires a number".to_string())?;
                    self.state
                        .media_elements
                        .get_mut(&object)
                        .expect("media object existence checked")
                        .set_volume(volume)?;
                    value
                }
                "muted" => {
                    let muted = value.is_truthy();
                    self.state
                        .media_elements
                        .get_mut(&object)
                        .expect("media object existence checked")
                        .set_muted(muted);
                    JsvValue::Boolean(muted)
                }
                "playbackRate" => {
                    let rate = value
                        .as_number()
                        .ok_or_else(|| "TypeError: playbackRate requires a number".to_string())?;
                    self.state
                        .media_elements
                        .get_mut(&object)
                        .expect("media object existence checked")
                        .set_playback_rate(rate)?;
                    value
                }
                "src" => {
                    let node = self.locator(object)?;
                    let text = Self::argument_string(&value, "Media src")?;
                    if self.dom.get_attribute(node, "src").map(str::to_string) != Some(text.clone())
                    {
                        self.dom.set_attribute(node, "src", &text)?;
                        // Assigning a new src resets the media element: the
                        // MSE attachment is dropped and direct URLs are
                        // recorded for the network layer (unsupported and
                        // cipher-only sources fail closed).
                        self.state.media_attachments.remove(&object);
                        self.state
                            .media_elements
                            .get_mut(&object)
                            .expect("media object existence checked")
                            .pause();
                    }
                    value
                }
                "playsInline" | "autoplay" | "loop" => {
                    let node = self.locator(object)?;
                    if value.is_truthy() {
                        self.dom.set_attribute(node, property, "")?;
                    } else {
                        self.dom.remove_attribute(node, property)?;
                    }
                    value
                }
                _ => {
                    return Err(format!(
                        "TypeError: HTMLMediaElement.{} is not writable",
                        property
                    ))
                }
            };
            self.collect_media_events(object);
            return Ok(result);
        }
        if let Some((source, buffer)) = self.state.source_buffers.get(&object).copied() {
            if property != "timestampOffset" {
                return Err(format!(
                    "TypeError: SourceBuffer.{} is not writable",
                    property
                ));
            }
            let seconds = value
                .as_number()
                .filter(|value| value.is_finite())
                .ok_or_else(|| "TypeError: timestampOffset requires a number".to_string())?;
            self.state
                .media_sources
                .get_mut(&source)
                .and_then(|source| source.source_buffer_mut(buffer))
                .ok_or_else(|| "InvalidStateError: SourceBuffer is detached".to_string())?
                .set_timestamp_offset((seconds * 1_000_000.0) as i64)?;
            return Ok(value);
        }
        if self.state.media_sources.contains_key(&object) {
            if property != "duration" {
                return Err(format!(
                    "TypeError: MediaSource.{} is not writable",
                    property
                ));
            }
            let seconds = value
                .as_number()
                .filter(|value| value.is_finite() && *value > 0.0)
                .ok_or_else(|| "TypeError: MediaSource duration is invalid".to_string())?;
            self.state
                .media_sources
                .get_mut(&object)
                .expect("media source existence checked")
                .set_duration((seconds * 1_000_000.0) as i64)?;
            return Ok(value);
        }
        if let Some(state) = self.state.canvas_contexts.get_mut(&object) {
            let text = Self::argument_string(&value, "Canvas style")?;
            match property {
                "fillStyle" => state.fill = text,
                "strokeStyle" => state.stroke = text,
                "font" => state.font = text,
                _ => {
                    return Err(format!(
                        "TypeError: CanvasRenderingContext2D.{} is not writable",
                        property
                    ))
                }
            }
            return Ok(value);
        }
        if matches!(object, HOST_LOCATION) {
            let node_url = self.state.current_url.clone();
            let mut url = url::Url::parse(&node_url)
                .map_err(|_| "SecurityError: location is unavailable".to_string())?;
            let value = Self::argument_string(&value, "Location value")?;
            match property {
                "href" => {
                    if let Some(resolved) = self.resolved_navigation(&value) {
                        self.state.current_url = resolved.clone();
                        self.record_history_mutation("navigate", &resolved);
                    } else {
                        self.record_history_mutation("navigate-blocked", &value);
                    }
                }
                "hash" => {
                    let hash = if value.starts_with('#') {
                        value.clone()
                    } else {
                        format!("#{}", value)
                    };
                    url.set_fragment(Some(hash.trim_start_matches('#')));
                    let updated = String::from(url);
                    if updated != node_url {
                        self.state.current_url = updated.clone();
                        self.record_history_mutation("hashchange", &updated);
                        self.queue_pending_dispatch(
                            LISTENER_WINDOW,
                            "hashchange",
                            Some(hash.clone()),
                        );
                    }
                }
                "search" => {
                    url.set_query(Some(value.trim_start_matches('?')));
                    let updated = String::from(url);
                    if updated != node_url {
                        self.state.current_url = updated.clone();
                        self.record_history_mutation("searchchange", &updated);
                    }
                }
                _ => return Err(format!("TypeError: location.{} is not writable", property)),
            }
            return Ok(JsvValue::Undefined);
        }
        if matches!(
            object,
            HOST_DOCUMENT
                | HOST_WINDOW
                | HOST_LOCAL_STORAGE
                | HOST_HISTORY
                | HOST_EVENT
                | HOST_INDEXED_DB
                | HOST_CACHE_STORAGE
                | HOST_NAVIGATOR
                | HOST_SERVICE_WORKER
        ) || self.state.idb_databases.contains_key(&object)
            || self.state.idb_stores.contains_key(&object)
            || self.state.cache_handles.contains_key(&object)
            || self
                .state
                .service_worker_registrations
                .contains_key(&object)
            || self.state.web_sockets.contains_key(&object)
            || self.state.event_sources.contains_key(&object)
            || self.state.broadcast_channels.contains_key(&object)
        {
            return Err(format!("SecurityError: {} is read-only", property));
        }
        if self.state.form_data.contains_key(&object) {
            return Err("SecurityError: FormData is read-only".to_string());
        }
        let node = self.locator(object)?;
        match property {
            "textContent" | "innerText" => {
                let text = Self::argument_string(&value, "Element text")?;
                self.dom.set_text_content(node, &text)?;
            }
            "id" => {
                self.dom
                    .set_attribute(node, "id", &Self::argument_string(&value, "Element id")?)?
            }
            "className" => self.dom.set_attribute(
                node,
                "class",
                &Self::argument_string(&value, "Element class")?,
            )?,
            "value" => {
                let text = Self::argument_string(&value, "Element value")?;
                match self.element_tag(node).as_deref() {
                    Some("textarea") => self.dom.set_text_content(node, &text)?,
                    Some("select") => self.select_value(node, &text)?,
                    _ => self.dom.set_attribute(node, "value", &text)?,
                }
            }
            "checked" => {
                if value.is_truthy() {
                    self.dom.set_attribute(node, "checked", "")?;
                } else {
                    self.dom.remove_attribute(node, "checked")?;
                }
            }
            "innerHTML" => {
                let html = Self::argument_string(&value, "innerHTML")?;
                self.dom.set_inner_html(node, &html)?;
            }
            "disabled" | "hidden" => {
                if value.is_truthy() {
                    self.dom.set_attribute(node, property, "")?;
                } else {
                    self.dom.remove_attribute(node, property)?;
                }
            }
            _ => return Err(format!("TypeError: element.{} is not writable", property)),
        }
        self.report.dom_mutations = self.report.dom_mutations.saturating_add(1);
        Ok(value)
    }

    fn call(
        &mut self,
        object: u64,
        method: &str,
        arguments: Vec<JsvValue>,
    ) -> Result<JsvValue, String> {
        self.charge()?;
        // Phase 22 platform capabilities are dispatched before legacy media
        // and DOM objects, keeping opaque handle namespaces isolated.
        if object == HOST_WINDOW && method == "structuredClone" {
            let value = arguments
                .first()
                .ok_or_else(|| "TypeError: structuredClone requires a value".to_string())?;
            let json = Self::jsv_to_json(value)?;
            let cloned = crate::messaging::structured_clone(&json, 0)?;
            self.record_platform_operation("structuredClone".to_string());
            return Ok(Self::json_to_jsv(&cloned));
        }
        if object == HOST_WINDOW && method == "BroadcastChannel" {
            let name = arguments
                .first()
                .ok_or_else(|| "TypeError: BroadcastChannel requires a name".to_string())?;
            let name = Self::argument_string(name, "BroadcastChannel name")?;
            if name.is_empty() || name.len() > 256 {
                return Err("TypeError: invalid BroadcastChannel name".to_string());
            }
            let handle = self.allocate_platform_handle()?;
            let origin = runtime_origin(&self.state.current_url);
            self.state.broadcast_channels.insert(
                handle,
                crate::messaging::BroadcastChannel::new(origin, name.clone()),
            );
            self.record_platform_operation(format!("BroadcastChannel.open {name}"));
            return Ok(JsvValue::HostObject(handle));
        }
        if object == HOST_WINDOW && method == "WebSocket" {
            let target = arguments
                .first()
                .ok_or_else(|| "TypeError: WebSocket requires a URL".to_string())?;
            let target = Self::argument_string(target, "WebSocket URL")?;
            let parsed = url::Url::parse(&target)
                .map_err(|_| "SyntaxError: invalid WebSocket URL".to_string())?;
            let page = url::Url::parse(&self.state.current_url)
                .map_err(|_| "SecurityError: page origin is unavailable".to_string())?;
            if !matches!(parsed.scheme(), "ws" | "wss")
                || parsed.host_str() != page.host_str()
                || parsed.port_or_known_default() != page.port_or_known_default()
                || (page.scheme() == "https" && parsed.scheme() != "wss")
            {
                return Err(
                    "SecurityError: cross-origin or insecure WebSocket rejected".to_string()
                );
            }
            let protocol = arguments.get(1).map(|value| value.to_display_string());
            let socket = crate::realtime::WebSocketClient::new(target, protocol.as_deref())?;
            let handle = self.allocate_platform_handle()?;
            self.state.web_sockets.insert(handle, socket);
            self.record_platform_operation(format!("WebSocket.open {}", parsed));
            return Ok(JsvValue::HostObject(handle));
        }
        if object == HOST_WINDOW && method == "EventSource" {
            let target = arguments
                .first()
                .ok_or_else(|| "TypeError: EventSource requires a URL".to_string())?;
            let target = Self::argument_string(target, "EventSource URL")?;
            let target = resolve_same_origin(&self.state.current_url, &target)
                .ok_or_else(|| "SecurityError: cross-origin EventSource rejected".to_string())?;
            let source = crate::realtime::EventSourceClient::new(&target)?;
            let handle = self.allocate_platform_handle()?;
            self.state.event_sources.insert(handle, source);
            self.record_platform_operation(format!("EventSource.open {target}"));
            return Ok(JsvValue::HostObject(handle));
        }
        if let Some(mut channel) = self.state.broadcast_channels.remove(&object) {
            let result = match method {
                "postMessage" => {
                    let value = arguments.first().ok_or_else(|| {
                        "TypeError: BroadcastChannel.postMessage requires a value".to_string()
                    })?;
                    let message = serde_json::to_string(&Self::jsv_to_json(value)?)
                        .map_err(|error| error.to_string())?;
                    channel.post_message(message)?;
                    self.record_platform_operation(format!(
                        "BroadcastChannel.postMessage {}",
                        channel.name
                    ));
                    Ok(JsvValue::Undefined)
                }
                "poll" => Ok(channel
                    .poll_message()
                    .and_then(|message| serde_json::from_str(&message).ok())
                    .as_ref()
                    .map(Self::json_to_jsv)
                    .unwrap_or(JsvValue::Null)),
                "close" => {
                    channel.close();
                    self.record_platform_operation(format!(
                        "BroadcastChannel.close {}",
                        channel.name
                    ));
                    Ok(JsvValue::Undefined)
                }
                _ => Err(format!(
                    "SecurityError: BroadcastChannel method '{method}' is unavailable"
                )),
            };
            if !channel.closed {
                self.state.broadcast_channels.insert(object, channel);
            }
            return result;
        }
        if let Some(socket) = self.state.web_sockets.get_mut(&object) {
            return match method {
                "send" => {
                    let message = arguments
                        .first()
                        .ok_or_else(|| "TypeError: WebSocket.send requires data".to_string())?;
                    let message = Self::argument_string(message, "WebSocket frame")?;
                    let socket_url = socket.url.clone();
                    socket.send(crate::realtime::WebSocketMessage::Text(message))?;
                    self.record_platform_operation(format!("WebSocket.send {socket_url}"));
                    Ok(JsvValue::Undefined)
                }
                "poll" => Ok(socket
                    .poll_incoming()
                    .map(|message| match message {
                        crate::realtime::WebSocketMessage::Text(text) => JsvValue::String(text),
                        crate::realtime::WebSocketMessage::Binary(bytes) => {
                            JsvValue::Array(Rc::new(RefCell::new(
                                bytes
                                    .into_iter()
                                    .map(|byte| JsvValue::Number(f64::from(byte)))
                                    .collect(),
                            )))
                        }
                    })
                    .unwrap_or(JsvValue::Null)),
                "close" => {
                    let code = arguments
                        .first()
                        .and_then(JsvValue::as_number)
                        .map(|v| v as u16);
                    let reason = arguments.get(1).map(JsvValue::to_display_string);
                    socket.close(code, reason)?;
                    Ok(JsvValue::Undefined)
                }
                _ => Err(format!(
                    "SecurityError: WebSocket method '{method}' is unavailable"
                )),
            };
        }
        if let Some(source) = self.state.event_sources.get_mut(&object) {
            return match method {
                "poll" => Ok(source
                    .poll_event()
                    .map(|event| {
                        let mut properties = std::collections::HashMap::new();
                        properties.insert("type".to_string(), JsvValue::String(event.event_type));
                        properties.insert("data".to_string(), JsvValue::String(event.data));
                        properties.insert(
                            "lastEventId".to_string(),
                            JsvValue::String(event.last_event_id),
                        );
                        JsvValue::Object(Rc::new(RefCell::new(crate::javascript::JsvObject {
                            properties,
                            prototype: None,
                        })))
                    })
                    .unwrap_or(JsvValue::Null)),
                "close" => {
                    source.close();
                    Ok(JsvValue::Undefined)
                }
                _ => Err(format!(
                    "SecurityError: EventSource method '{method}' is unavailable"
                )),
            };
        }
        if object == HOST_INDEXED_DB {
            return match method {
                "open" => {
                    let name = arguments
                        .first()
                        .ok_or_else(|| "TypeError: indexedDB.open requires a name".to_string())?;
                    let name = Self::argument_string(name, "IndexedDB name")?;
                    if name.is_empty() || name.len() > 256 {
                        return Err("TypeError: invalid IndexedDB name".to_string());
                    }
                    let version = arguments
                        .get(1)
                        .and_then(JsvValue::as_number)
                        .filter(|value| value.is_finite() && *value >= 1.0)
                        .map(|value| value as u64);
                    let (transaction, _) = self.state.indexeddb.open_db(&name, version)?;
                    self.state.indexeddb.commit_transaction(transaction)?;
                    let handle = self.allocate_platform_handle()?;
                    self.state.idb_databases.insert(handle, name.clone());
                    self.record_platform_operation(format!("indexedDB.open {name}"));
                    Ok(JsvValue::HostObject(handle))
                }
                "deleteDatabase" => {
                    let name = arguments.first().ok_or_else(|| {
                        "TypeError: indexedDB.deleteDatabase requires a name".to_string()
                    })?;
                    let name = Self::argument_string(name, "IndexedDB name")?;
                    let deleted = self.state.indexeddb.delete_database(&name);
                    if deleted {
                        self.state.idb_databases.retain(|_, value| value != &name);
                        self.state
                            .idb_stores
                            .retain(|_, (database, _)| database != &name);
                    }
                    self.update_platform_quota();
                    Ok(JsvValue::Boolean(deleted))
                }
                "databases" => Ok(JsvValue::Array(Rc::new(RefCell::new(
                    self.state
                        .indexeddb
                        .databases
                        .keys()
                        .cloned()
                        .map(JsvValue::String)
                        .collect(),
                )))),
                _ => Err(format!(
                    "SecurityError: indexedDB method '{method}' is unavailable"
                )),
            };
        }
        if let Some(database_name) = self.state.idb_databases.get(&object).cloned() {
            return match method {
                "createObjectStore" => {
                    let name = arguments.first().ok_or_else(|| {
                        "TypeError: createObjectStore requires a name".to_string()
                    })?;
                    let name = Self::argument_string(name, "Object store name")?;
                    let key_path = arguments.get(1).and_then(|value| match value {
                        JsvValue::String(value) if !value.is_empty() => Some(value.clone()),
                        _ => None,
                    });
                    let auto_increment = arguments.get(2).is_some_and(JsvValue::is_truthy);
                    self.state
                        .indexeddb
                        .databases
                        .get_mut(&database_name)
                        .ok_or_else(|| "InvalidStateError: database is closed".to_string())?
                        .create_object_store(&name, key_path, auto_increment)?;
                    self.state.indexeddb.persist()?;
                    let handle = self.allocate_platform_handle()?;
                    self.state
                        .idb_stores
                        .insert(handle, (database_name.clone(), name.clone()));
                    self.record_platform_operation(format!(
                        "indexedDB.createObjectStore {database_name}/{name}"
                    ));
                    Ok(JsvValue::HostObject(handle))
                }
                "objectStore" => {
                    let name = arguments
                        .first()
                        .ok_or_else(|| "TypeError: objectStore requires a name".to_string())?;
                    let name = Self::argument_string(name, "Object store name")?;
                    let exists = self
                        .state
                        .indexeddb
                        .databases
                        .get(&database_name)
                        .is_some_and(|database| database.object_stores.contains_key(&name));
                    if !exists {
                        return Err("NotFoundError: object store does not exist".to_string());
                    }
                    let handle = self.allocate_platform_handle()?;
                    self.state.idb_stores.insert(handle, (database_name, name));
                    Ok(JsvValue::HostObject(handle))
                }
                "deleteObjectStore" => {
                    let name = arguments.first().ok_or_else(|| {
                        "TypeError: deleteObjectStore requires a name".to_string()
                    })?;
                    let name = Self::argument_string(name, "Object store name")?;
                    let removed = self
                        .state
                        .indexeddb
                        .databases
                        .get_mut(&database_name)
                        .is_some_and(|database| database.delete_object_store(&name));
                    if removed {
                        self.state.indexeddb.persist()?;
                        self.state.idb_stores.retain(|_, (database, store)| {
                            database != &database_name || store != &name
                        });
                    }
                    self.update_platform_quota();
                    Ok(JsvValue::Boolean(removed))
                }
                "close" => {
                    self.state.idb_databases.remove(&object);
                    self.state
                        .idb_stores
                        .retain(|_, (database, _)| database != &database_name);
                    Ok(JsvValue::Undefined)
                }
                _ => Err(format!(
                    "SecurityError: IDBDatabase method '{method}' is unavailable"
                )),
            };
        }
        if let Some((database_name, store_name)) = self.state.idb_stores.get(&object).cloned() {
            return match method {
                "put" | "add" => {
                    let value = arguments.first().ok_or_else(|| {
                        format!("TypeError: IDBObjectStore.{method} requires a value")
                    })?;
                    let value = serde_json::to_string(&Self::jsv_to_json(value)?)
                        .map_err(|error| error.to_string())?;
                    self.state
                        .quota_manager
                        .check_quota(&self.state.indexeddb.origin, value.len())?;
                    let key = arguments.get(1).map(Self::idb_key).transpose()?;
                    let store = self
                        .state
                        .indexeddb
                        .databases
                        .get_mut(&database_name)
                        .and_then(|database| database.object_stores.get_mut(&store_name))
                        .ok_or_else(|| "InvalidStateError: object store is detached".to_string())?;
                    let key = if method == "put" {
                        store.put(key, value)?
                    } else {
                        store.add(key, value)?
                    };
                    self.state.indexeddb.persist()?;
                    self.update_platform_quota();
                    self.record_platform_operation(format!(
                        "indexedDB.{method} {database_name}/{store_name}"
                    ));
                    Ok(match key {
                        crate::indexeddb::IDBKey::Number(value) => JsvValue::Number(value),
                        crate::indexeddb::IDBKey::String(value) => JsvValue::String(value),
                        _ => JsvValue::String(format!("{key:?}")),
                    })
                }
                "get" => {
                    let key = arguments
                        .first()
                        .ok_or_else(|| "TypeError: get requires a key".to_string())
                        .and_then(Self::idb_key)?;
                    let value = self
                        .state
                        .indexeddb
                        .databases
                        .get(&database_name)
                        .and_then(|database| database.object_stores.get(&store_name))
                        .and_then(|store| store.get(&key))
                        .and_then(|record| serde_json::from_str(&record.value).ok())
                        .as_ref()
                        .map(Self::json_to_jsv)
                        .unwrap_or(JsvValue::Undefined);
                    Ok(value)
                }
                "delete" => {
                    let key = arguments
                        .first()
                        .ok_or_else(|| "TypeError: delete requires a key".to_string())
                        .and_then(Self::idb_key)?;
                    let removed = self
                        .state
                        .indexeddb
                        .databases
                        .get_mut(&database_name)
                        .and_then(|database| database.object_stores.get_mut(&store_name))
                        .is_some_and(|store| store.delete(&key));
                    if removed {
                        self.state.indexeddb.persist()?;
                    }
                    self.update_platform_quota();
                    Ok(JsvValue::Boolean(removed))
                }
                "clear" => {
                    self.state
                        .indexeddb
                        .databases
                        .get_mut(&database_name)
                        .and_then(|database| database.object_stores.get_mut(&store_name))
                        .ok_or_else(|| "InvalidStateError: object store is detached".to_string())?
                        .clear();
                    self.state.indexeddb.persist()?;
                    self.update_platform_quota();
                    Ok(JsvValue::Undefined)
                }
                "count" => Ok(JsvValue::Number(
                    self.state
                        .indexeddb
                        .databases
                        .get(&database_name)
                        .and_then(|database| database.object_stores.get(&store_name))
                        .map(|store| store.count() as f64)
                        .unwrap_or_default(),
                )),
                _ => Err(format!(
                    "SecurityError: IDBObjectStore method '{method}' is unavailable"
                )),
            };
        }
        if object == HOST_CACHE_STORAGE {
            return match method {
                "open" => {
                    let name = arguments
                        .first()
                        .ok_or_else(|| "TypeError: caches.open requires a name".to_string())?;
                    let name = Self::argument_string(name, "Cache name")?;
                    if name.is_empty() || name.len() > 256 {
                        return Err("TypeError: invalid cache name".to_string());
                    }
                    self.state.cache_storage.open(&name)?;
                    let handle = self.allocate_platform_handle()?;
                    self.state.cache_handles.insert(handle, name.clone());
                    self.record_platform_operation(format!("caches.open {name}"));
                    Ok(JsvValue::HostObject(handle))
                }
                "has" => {
                    let name = arguments
                        .first()
                        .ok_or_else(|| "TypeError: caches.has requires a name".to_string())?;
                    Ok(JsvValue::Boolean(
                        self.state
                            .cache_storage
                            .has(&Self::argument_string(name, "Cache name")?),
                    ))
                }
                "delete" => {
                    let name = arguments
                        .first()
                        .ok_or_else(|| "TypeError: caches.delete requires a name".to_string())?;
                    let name = Self::argument_string(name, "Cache name")?;
                    let removed = self.state.cache_storage.delete(&name);
                    if removed {
                        self.state.cache_handles.retain(|_, cache| cache != &name);
                    }
                    self.update_platform_quota();
                    Ok(JsvValue::Boolean(removed))
                }
                "keys" => Ok(JsvValue::Array(Rc::new(RefCell::new(
                    self.state
                        .cache_storage
                        .keys()
                        .into_iter()
                        .map(JsvValue::String)
                        .collect(),
                )))),
                "match" => {
                    let target = arguments
                        .first()
                        .ok_or_else(|| "TypeError: caches.match requires a URL".to_string())?;
                    let target = resolve_same_origin(
                        &self.state.current_url,
                        &Self::argument_string(target, "Cache request URL")?,
                    )
                    .ok_or_else(|| "SecurityError: cross-origin cache request".to_string())?;
                    Ok(self
                        .state
                        .cache_storage
                        .match_all(&target)
                        .map(|entry| {
                            JsvValue::String(String::from_utf8_lossy(&entry.response_body).into())
                        })
                        .unwrap_or(JsvValue::Undefined))
                }
                _ => Err(format!(
                    "SecurityError: CacheStorage method '{method}' is unavailable"
                )),
            };
        }
        if let Some(cache_name) = self.state.cache_handles.get(&object).cloned() {
            return match method {
                "put" => {
                    if arguments.len() < 2 {
                        return Err("TypeError: Cache.put requires a URL and body".to_string());
                    }
                    let target = resolve_same_origin(
                        &self.state.current_url,
                        &Self::argument_string(&arguments[0], "Cache request URL")?,
                    )
                    .ok_or_else(|| "SecurityError: cross-origin cache request".to_string())?;
                    let body =
                        Self::argument_string(&arguments[1], "Cache response body")?.into_bytes();
                    self.state
                        .quota_manager
                        .check_quota(&self.state.cache_storage.origin, body.len())?;
                    let status = arguments
                        .get(2)
                        .and_then(JsvValue::as_number)
                        .filter(|value| value.is_finite() && (100.0..=599.0).contains(value))
                        .map(|value| value as u16)
                        .unwrap_or(200);
                    self.state.cache_storage.open(&cache_name)?.put(
                        &target,
                        "GET",
                        status,
                        std::collections::HashMap::new(),
                        body,
                    )?;
                    self.state.cache_storage.persist()?;
                    self.update_platform_quota();
                    self.record_platform_operation(format!("Cache.put {target}"));
                    Ok(JsvValue::Undefined)
                }
                "match" => {
                    let target = arguments
                        .first()
                        .ok_or_else(|| "TypeError: Cache.match requires a URL".to_string())?;
                    let target = resolve_same_origin(
                        &self.state.current_url,
                        &Self::argument_string(target, "Cache request URL")?,
                    )
                    .ok_or_else(|| "SecurityError: cross-origin cache request".to_string())?;
                    Ok(self
                        .state
                        .cache_storage
                        .caches
                        .get(&cache_name)
                        .and_then(|cache| cache.match_req(&target))
                        .map(|entry| {
                            JsvValue::String(String::from_utf8_lossy(&entry.response_body).into())
                        })
                        .unwrap_or(JsvValue::Undefined))
                }
                "delete" => {
                    let target = arguments
                        .first()
                        .ok_or_else(|| "TypeError: Cache.delete requires a URL".to_string())?;
                    let target = resolve_same_origin(
                        &self.state.current_url,
                        &Self::argument_string(target, "Cache request URL")?,
                    )
                    .ok_or_else(|| "SecurityError: cross-origin cache request".to_string())?;
                    let removed = self
                        .state
                        .cache_storage
                        .caches
                        .get_mut(&cache_name)
                        .is_some_and(|cache| cache.delete(&target));
                    if removed {
                        self.state.cache_storage.persist()?;
                    }
                    self.update_platform_quota();
                    Ok(JsvValue::Boolean(removed))
                }
                "keys" => Ok(JsvValue::Array(Rc::new(RefCell::new(
                    self.state
                        .cache_storage
                        .caches
                        .get(&cache_name)
                        .map(crate::cache_api::Cache::keys)
                        .unwrap_or_default()
                        .into_iter()
                        .map(JsvValue::String)
                        .collect(),
                )))),
                _ => Err(format!(
                    "SecurityError: Cache method '{method}' is unavailable"
                )),
            };
        }
        if object == HOST_SERVICE_WORKER {
            return match method {
                "register" => {
                    let script = arguments.first().ok_or_else(|| {
                        "TypeError: serviceWorker.register requires a script URL".to_string()
                    })?;
                    let script = Self::argument_string(script, "Service worker script URL")?;
                    let options = arguments.get(1).and_then(|value| match value {
                        JsvValue::String(scope) => {
                            Some(crate::service_worker::ServiceWorkerRegistrationOptions {
                                scope: scope.clone(),
                            })
                        }
                        JsvValue::Object(object) => object
                            .borrow()
                            .properties
                            .get("scope")
                            .map(JsvValue::to_display_string)
                            .map(
                                |scope| crate::service_worker::ServiceWorkerRegistrationOptions {
                                    scope,
                                },
                            ),
                        _ => None,
                    });
                    let scope = self
                        .state
                        .service_workers
                        .register(&script, options)?
                        .scope
                        .clone();
                    let handle = self.allocate_platform_handle()?;
                    self.state
                        .service_worker_registrations
                        .insert(handle, scope.clone());
                    self.record_platform_operation(format!("serviceWorker.register {scope}"));
                    Ok(JsvValue::HostObject(handle))
                }
                "getRegistration" => {
                    let client_url = arguments
                        .first()
                        .map(JsvValue::to_display_string)
                        .unwrap_or_else(|| self.state.current_url.clone());
                    match self
                        .state
                        .service_workers
                        .get_registration(&client_url)
                        .map(|registration| registration.scope.clone())
                    {
                        Some(scope) => {
                            let handle = self.allocate_platform_handle()?;
                            self.state
                                .service_worker_registrations
                                .insert(handle, scope);
                            Ok(JsvValue::HostObject(handle))
                        }
                        None => Ok(JsvValue::Undefined),
                    }
                }
                _ => Err(format!(
                    "SecurityError: ServiceWorkerContainer method '{method}' is unavailable"
                )),
            };
        }
        if let Some(scope) = self
            .state
            .service_worker_registrations
            .get(&object)
            .cloned()
        {
            return match method {
                "unregister" => {
                    let removed = self.state.service_workers.unregister(&scope);
                    if removed {
                        self.state.service_worker_registrations.remove(&object);
                    }
                    Ok(JsvValue::Boolean(removed))
                }
                _ => Err(format!(
                    "SecurityError: ServiceWorkerRegistration method '{method}' is unavailable"
                )),
            };
        }
        if object == HOST_WINDOW && method == "MediaSource.isTypeSupported" {
            let content_type = arguments
                .first()
                .map(|value| value.to_display_string())
                .unwrap_or_default();
            let supported =
                crate::media_core::parse_media_type(&content_type).is_ok_and(|media_type| {
                    if media_type.codecs.is_empty() {
                        return false;
                    }
                    let capabilities = merged_capabilities(
                        &WindowsMediaFoundationBackend,
                        &FallbackRegistry::default(),
                    );
                    media_type
                        .codecs
                        .iter()
                        .all(|codec| capabilities.supports(codec))
                });
            return Ok(JsvValue::Boolean(supported));
        }
        if object == HOST_WINDOW && method == "MediaSource" {
            return self.allocate_media_source();
        }
        if self.state.media_elements.contains_key(&object) {
            match method {
                "canPlayType" => {
                    let content_type = arguments
                        .first()
                        .map(|value| value.to_display_string())
                        .unwrap_or_default();
                    let supported = crate::media_core::parse_media_type(&content_type).is_ok_and(
                        |media_type| {
                            if media_type.codecs.is_empty() {
                                return false;
                            }
                            let capabilities = merged_capabilities(
                                &WindowsMediaFoundationBackend,
                                &FallbackRegistry::default(),
                            );
                            media_type
                                .codecs
                                .iter()
                                .all(|codec| capabilities.supports(codec))
                        },
                    );
                    return Ok(JsvValue::String(
                        if supported { "probably" } else { "" }.to_string(),
                    ));
                }
                "play" | "load" => {
                    if let Some(source_id) = self.state.media_attachments.get(&object).copied() {
                        let source = self
                            .state
                            .media_sources
                            .get(&source_id)
                            .cloned()
                            .ok_or_else(|| {
                                "InvalidStateError: MediaSource is detached".to_string()
                            })?;
                        let needs_attach = self
                            .state
                            .media_elements
                            .get(&object)
                            .is_some_and(|media| media.media_source().is_none());
                        if needs_attach {
                            self.state
                                .media_elements
                                .get_mut(&object)
                                .expect("media object existence checked")
                                .attach_media_source(source)?;
                        }
                    }
                    if method == "play" {
                        self.state
                            .media_elements
                            .get_mut(&object)
                            .expect("media object existence checked")
                            .play()?;
                    } else {
                        self.state
                            .media_elements
                            .get_mut(&object)
                            .expect("media object existence checked")
                            .synchronize_source_state();
                    }
                    self.collect_media_events(object);
                    return Ok(JsvValue::Undefined);
                }
                "pause" => {
                    self.state
                        .media_elements
                        .get_mut(&object)
                        .expect("media object existence checked")
                        .pause();
                    self.collect_media_events(object);
                    return Ok(JsvValue::Undefined);
                }
                _ => {
                    return Err(format!(
                        "SecurityError: HTMLMediaElement method '{}' is unavailable",
                        method
                    ))
                }
            }
        }
        if self.state.media_sources.contains_key(&object) {
            return match method {
                "addSourceBuffer" => {
                    let content_type = arguments.first().ok_or_else(|| {
                        "TypeError: addSourceBuffer requires a content type".to_string()
                    })?;
                    let content_type = Self::argument_string(content_type, "Media type")?;
                    let capabilities = merged_capabilities(
                        &WindowsMediaFoundationBackend,
                        &FallbackRegistry::default(),
                    );
                    let buffer = self
                        .state
                        .media_sources
                        .get_mut(&object)
                        .expect("media source existence checked")
                        .add_source_buffer(&content_type, &capabilities)?;
                    self.allocate_source_buffer(object, buffer)
                }
                "endOfStream" => {
                    self.state
                        .media_sources
                        .get_mut(&object)
                        .expect("media source existence checked")
                        .end_of_stream()?;
                    Ok(JsvValue::Undefined)
                }
                _ => Err(format!(
                    "SecurityError: MediaSource method '{}' is unavailable",
                    method
                )),
            };
        }
        if let Some((source, buffer)) = self.state.source_buffers.get(&object).copied() {
            return match method {
                "remove" => {
                    let start = arguments
                        .first()
                        .and_then(JsvValue::as_number)
                        .filter(|value| value.is_finite() && *value >= 0.0)
                        .ok_or_else(|| "TypeError: remove requires a start time".to_string())?;
                    let end = arguments
                        .get(1)
                        .and_then(JsvValue::as_number)
                        .filter(|value| value.is_finite() && *value >= 0.0)
                        .ok_or_else(|| "TypeError: remove requires an end time".to_string())?;
                    if end <= start {
                        return Err("TypeError: remove end must be greater than start".to_string());
                    }
                    let removed = self
                        .state
                        .media_sources
                        .get_mut(&source)
                        .and_then(|source| source.source_buffer_mut(buffer))
                        .ok_or_else(|| "InvalidStateError: SourceBuffer is detached".to_string())?
                        .remove(crate::mse::TimeRange {
                            start_us: (start * 1_000_000.0) as i64,
                            end_us: (end * 1_000_000.0) as i64,
                        })?;
                    Ok(JsvValue::Number(removed as f64))
                }
                "appendBuffer" => {
                    let bytes =
                        Self::argument_bytes(arguments.first().ok_or_else(|| {
                            "TypeError: appendBuffer requires bytes".to_string()
                        })?)?;
                    self.state
                        .media_sources
                        .get_mut(&source)
                        .ok_or_else(|| "InvalidStateError: MediaSource is detached".to_string())?
                        .append_buffer(buffer, &bytes)?;
                    Ok(JsvValue::Undefined)
                }
                "abort" => {
                    self.state
                        .media_sources
                        .get_mut(&source)
                        .and_then(|source| source.source_buffer_mut(buffer))
                        .ok_or_else(|| "InvalidStateError: SourceBuffer is detached".to_string())?
                        .abort();
                    Ok(JsvValue::Undefined)
                }
                _ => Err(format!(
                    "SecurityError: SourceBuffer method '{}' is unavailable",
                    method
                )),
            };
        }
        match (object, method) {
            (HOST_DOCUMENT, "getElementById") => {
                let id = arguments
                    .first()
                    .ok_or_else(|| "TypeError: getElementById requires an id".to_string())?;
                let id = Self::argument_string(id, "Element id")?;
                self.dom
                    .get_element_by_id(&id)
                    .map_or(Ok(JsvValue::Null), |node| self.element_for(node))
            }
            (HOST_DOCUMENT, "querySelector") => {
                let selector = arguments
                    .first()
                    .ok_or_else(|| "TypeError: querySelector requires a selector".to_string())?;
                let selector = Self::argument_string(selector, "Selector")?;
                self.dom
                    .query_selector(&selector)
                    .map_or(Ok(JsvValue::Null), |node| self.element_for(node))
            }
            (HOST_DOCUMENT, "createElement") => {
                let tag = arguments
                    .first()
                    .ok_or_else(|| "TypeError: createElement requires a tag".to_string())?;
                let node = self
                    .dom
                    .create_element(&Self::argument_string(tag, "Element tag")?)?;
                self.element_for(node)
            }
            (HOST_DOCUMENT, "createTextNode") => {
                let text = arguments
                    .first()
                    .ok_or_else(|| "TypeError: createTextNode requires text".to_string())?;
                let node = self
                    .dom
                    .create_text_node(&Self::argument_string(text, "Text")?)?;
                self.element_for(node)
            }
            (HOST_LOCAL_STORAGE, "getItem") => {
                let key = arguments
                    .first()
                    .ok_or_else(|| "TypeError: getItem requires a key".to_string())?;
                Ok(self
                    .state
                    .storage
                    .get(&Self::argument_string(key, "Storage key")?)
                    .map(|value| JsvValue::String(value.to_string()))
                    .unwrap_or(JsvValue::Null))
            }
            (HOST_LOCAL_STORAGE, "setItem") => {
                if arguments.len() != 2 {
                    return Err(
                        "TypeError: localStorage.setItem requires two arguments".to_string()
                    );
                }
                let key = Self::argument_string(&arguments[0], "Storage key")?;
                let value = Self::argument_string(&arguments[1], "Storage value")?;
                self.state.storage.set(&key, &value)?;
                if self.report.storage_writes.len() < 128 {
                    self.report.storage_writes.push((key, value));
                }
                Ok(JsvValue::Undefined)
            }
            (HOST_LOCAL_STORAGE, "removeItem") => {
                let key = arguments
                    .first()
                    .ok_or_else(|| "TypeError: removeItem requires a key".to_string())?;
                self.state
                    .storage
                    .remove(&Self::argument_string(key, "Storage key")?);
                Ok(JsvValue::Undefined)
            }
            (HOST_LOCAL_STORAGE, "key") => {
                let index = arguments
                    .first()
                    .and_then(JsvValue::as_number)
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .map(|value| value as usize)
                    .unwrap_or(usize::MAX);
                Ok(self
                    .state
                    .storage
                    .key_at(index)
                    .map(|key| JsvValue::String(key.to_string()))
                    .unwrap_or(JsvValue::Null))
            }
            (HOST_LOCAL_STORAGE, "clear") => {
                self.state.storage.clear();
                Ok(JsvValue::Undefined)
            }
            (HOST_WINDOW, "setTimeout") | (HOST_WINDOW, "setInterval") => {
                let callback = arguments
                    .first()
                    .cloned()
                    .ok_or_else(|| "TypeError: timer requires a callback".to_string())?;
                if !crate::javascript::is_callable_public(&callback) {
                    return Err("TypeError: timer callback is not callable".to_string());
                }
                let delay = arguments
                    .get(1)
                    .and_then(JsvValue::as_number)
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .map(|value| value as u64)
                    .unwrap_or(0);
                let id = self.set_timer(callback, delay, method == "setInterval")?;
                Ok(JsvValue::Number(id as f64))
            }
            (HOST_WINDOW, "clearTimeout") | (HOST_WINDOW, "clearInterval") => {
                if let Some(id) = arguments
                    .first()
                    .and_then(JsvValue::as_number)
                    .filter(|value| value.is_finite() && *value >= 0.0)
                {
                    self.clear_timer(id as u64);
                }
                Ok(JsvValue::Undefined)
            }
            (HOST_WINDOW, "Event") | (HOST_WINDOW, "CustomEvent") => {
                let event_type = arguments
                    .first()
                    .ok_or_else(|| "TypeError: Event requires an event type".to_string())?;
                let event_type = Self::argument_string(event_type, "Event type")?;
                let options = arguments.get(1);
                let bubbles = Self::option_bool(options, "bubbles");
                let cancelable = Self::option_bool(options, "cancelable");
                let detail = if method == "CustomEvent" {
                    Self::option_string(options, "detail")
                } else {
                    None
                };
                let id = self.allocate_event(event_type, bubbles, cancelable, detail)?;
                Ok(JsvValue::HostObject(id))
            }
            (HOST_WINDOW, "FormData") => {
                let form = match arguments.first() {
                    Some(JsvValue::HostObject(object)) => {
                        let node = self.locator(*object)?;
                        if self.element_tag(node).as_deref() != Some("form") {
                            return Err("TypeError: FormData requires a form element".to_string());
                        }
                        node
                    }
                    _ => return Err("TypeError: FormData requires a form element".to_string()),
                };
                let id = self.allocate_form_data(form)?;
                Ok(JsvValue::HostObject(id))
            }
            (HOST_FORM_DATA, "get") => {
                let name = arguments
                    .first()
                    .ok_or_else(|| "TypeError: FormData.get requires a name".to_string())?;
                let name = Self::argument_string(name, "FormData name")?;
                Ok(self
                    .state
                    .form_data
                    .get(&object)
                    .and_then(|entries| {
                        entries
                            .iter()
                            .find(|(candidate, _)| *candidate == name)
                            .map(|(_, value)| JsvValue::String(value.clone()))
                    })
                    .unwrap_or(JsvValue::Null))
            }
            (HOST_FORM_DATA, "has") => {
                let name = arguments
                    .first()
                    .ok_or_else(|| "TypeError: FormData.has requires a name".to_string())?;
                let name = Self::argument_string(name, "FormData name")?;
                Ok(JsvValue::Boolean(
                    self.state.form_data.get(&object).is_some_and(|entries| {
                        entries.iter().any(|(candidate, _)| *candidate == name)
                    }),
                ))
            }
            (HOST_FORM_DATA, "append") => {
                if arguments.len() < 2 {
                    return Err("TypeError: FormData.append requires a name and value".to_string());
                }
                let name = Self::argument_string(&arguments[0], "FormData name")?;
                let value = Self::argument_string(&arguments[1], "FormData value")?;
                let entries = self
                    .state
                    .form_data
                    .get_mut(&object)
                    .ok_or_else(|| "InvalidStateError: FormData is detached".to_string())?;
                if entries.len() >= MAX_FORM_ENTRIES {
                    return Err("QuotaExceededError: FormData entry budget exceeded".to_string());
                }
                entries.push((name, value));
                Ok(JsvValue::Undefined)
            }
            (HOST_FORM_DATA, "delete") => {
                let name = arguments
                    .first()
                    .ok_or_else(|| "TypeError: FormData.delete requires a name".to_string())?;
                let name = Self::argument_string(name, "FormData name")?;
                if let Some(entries) = self.state.form_data.get_mut(&object) {
                    entries.retain(|(candidate, _)| *candidate != name);
                }
                Ok(JsvValue::Undefined)
            }
            (HOST_HISTORY, "pushState") | (HOST_HISTORY, "replaceState") => {
                let state = arguments
                    .first()
                    .map(|value| value.to_display_string())
                    .map(|value| value.chars().take(64 * 1024).collect::<String>());
                let title = arguments
                    .get(1)
                    .map(|value| {
                        value
                            .to_display_string()
                            .chars()
                            .take(256)
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                let url = match arguments.get(2) {
                    Some(JsvValue::Undefined) | Some(JsvValue::Null) | None => {
                        self.state.current_url.clone()
                    }
                    Some(value) => {
                        let candidate = Self::argument_string(value, "History URL")?;
                        self.resolved_navigation(&candidate)
                            .ok_or_else(|| "SecurityError: cross-origin history URL".to_string())?
                    }
                };
                let entry = HistoryState {
                    url: url.clone(),
                    title,
                    state: state.filter(|value| !value.is_empty()),
                };
                if method == "pushState" {
                    self.state.history.push(entry);
                    self.record_history_mutation("pushState", &url);
                } else {
                    self.state.history.replace(entry);
                    self.record_history_mutation("replaceState", &url);
                }
                self.state.current_url = url;
                Ok(JsvValue::Undefined)
            }
            (HOST_HISTORY, "back") | (HOST_HISTORY, "forward") => {
                let entry = if method == "back" {
                    self.state.history.back()
                } else {
                    self.state.history.forward()
                };
                self.after_history_move(entry)?;
                Ok(JsvValue::Undefined)
            }
            (HOST_HISTORY, "go") => {
                let delta = arguments
                    .first()
                    .and_then(JsvValue::as_number)
                    .filter(|value| value.is_finite())
                    .map(|value| value as i32)
                    .unwrap_or(0);
                let moved = self.state.history.go(delta);
                self.after_history_move(moved)?;
                Ok(JsvValue::Undefined)
            }
            (HOST_LOCATION, "assign") | (HOST_LOCATION, "replace") => {
                let target = arguments
                    .first()
                    .ok_or_else(|| "TypeError: location.assign requires a URL".to_string())?;
                let target = Self::argument_string(target, "Location URL")?;
                match self.resolved_navigation(&target) {
                    Some(resolved) => {
                        self.state.current_url = resolved.clone();
                        self.record_history_mutation("navigate", &resolved);
                    }
                    None => self.record_history_mutation("navigate-blocked", &target),
                }
                Ok(JsvValue::Undefined)
            }
            (HOST_LOCATION, "reload") => {
                self.record_history_mutation("reload", &self.state.current_url.clone());
                Ok(JsvValue::Undefined)
            }
            (HOST_EVENT, "preventDefault") => {
                if let Some(event) = self.state.active_event.as_mut() {
                    if event.cancelable {
                        event.default_prevented = true;
                    }
                }
                Ok(JsvValue::Undefined)
            }
            (HOST_EVENT, "stopPropagation") => {
                if let Some(event) = self.state.active_event.as_mut() {
                    event.propagation_stopped = true;
                }
                Ok(JsvValue::Undefined)
            }
            (HOST_EVENT, "composedPath") => {
                let Some(event) = self.state.active_event.as_ref() else {
                    return Err("InvalidStateError: no event is being dispatched".to_string());
                };
                let path = self.dom.composed_path(event.target)?;
                let mut values = Vec::with_capacity(path.len());
                for node in path {
                    if node == self.dom.root() {
                        continue;
                    }
                    values.push(self.element_for(node)?);
                }
                Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
            }
            (HOST_EVENT, "stopImmediatePropagation") => {
                if let Some(event) = self.state.active_event.as_mut() {
                    event.immediate_propagation_stopped = true;
                    event.propagation_stopped = true;
                }
                Ok(JsvValue::Undefined)
            }
            (HOST_WINDOW, "fetch") => {
                let target = arguments
                    .first()
                    .ok_or_else(|| "TypeError: fetch requires a URL".to_string())?;
                if let Some(url) = resolve_same_origin(
                    &self.state.current_url.clone(),
                    &Self::argument_string(target, "Fetch URL")?,
                ) {
                    if !self.report.fetch_requests.contains(&url) {
                        self.report.fetch_requests.push(url);
                    }
                }
                Ok(JsvValue::Undefined)
            }
            (element, "setAttribute") => {
                if arguments.len() != 2 {
                    return Err("TypeError: setAttribute requires two arguments".to_string());
                }
                let name = Self::argument_string(&arguments[0], "Attribute name")?;
                let value = Self::argument_string(&arguments[1], "Attribute value")?;
                self.dom
                    .set_attribute(self.locator(element)?, &name, &value)?;
                self.report.dom_mutations = self.report.dom_mutations.saturating_add(1);
                Ok(JsvValue::Undefined)
            }
            (element, "getAttribute") => {
                let name = arguments
                    .first()
                    .ok_or_else(|| "TypeError: getAttribute requires a name".to_string())?;
                Ok(self
                    .dom
                    .get_attribute(
                        self.locator(element)?,
                        &Self::argument_string(name, "Attribute name")?,
                    )
                    .map_or(JsvValue::Null, |value| JsvValue::String(value.to_string())))
            }
            (element, "removeAttribute") => {
                let name = arguments
                    .first()
                    .ok_or_else(|| "TypeError: removeAttribute requires a name".to_string())?;
                self.dom.remove_attribute(
                    self.locator(element)?,
                    &Self::argument_string(name, "Attribute name")?,
                )?;
                self.report.dom_mutations = self.report.dom_mutations.saturating_add(1);
                Ok(JsvValue::Undefined)
            }
            (element, "appendChild") => {
                let child = arguments
                    .first()
                    .ok_or_else(|| "TypeError: appendChild requires a child".to_string())?;
                let JsvValue::HostObject(child) = child else {
                    return Err("TypeError: appendChild requires a DOM node".to_string());
                };
                self.dom
                    .append_child(self.locator(element)?, self.locator(*child)?)?;
                self.report.dom_mutations = self.report.dom_mutations.saturating_add(1);
                Ok(JsvValue::HostObject(*child))
            }
            (element, "removeChild") => {
                let child = arguments
                    .first()
                    .ok_or_else(|| "TypeError: removeChild requires a child".to_string())?;
                let JsvValue::HostObject(child) = child else {
                    return Err("TypeError: removeChild requires a DOM node".to_string());
                };
                self.dom
                    .remove_child(self.locator(element)?, self.locator(*child)?)?;
                self.report.dom_mutations = self.report.dom_mutations.saturating_add(1);
                Ok(JsvValue::HostObject(*child))
            }
            (_element, "querySelector") => {
                let selector = arguments
                    .first()
                    .ok_or_else(|| "TypeError: querySelector requires a selector".to_string())?;
                self.dom
                    .query_selector(&Self::argument_string(selector, "Selector")?)
                    .map_or(Ok(JsvValue::Null), |node| self.element_for(node))
            }
            (element, "focus") => {
                let node = self.locator(element)?;
                self.dom.focus(node)?;
                self.queue_pending_dispatch(node, "focus", None);
                Ok(JsvValue::Undefined)
            }
            (element, "click") => {
                let node = self.locator(element)?;
                let native = self.dom.click(node)?;
                self.report.dom_mutations = self
                    .report
                    .dom_mutations
                    .saturating_add(native.default_actions.len());
                self.queue_pending_dispatch(node, "click", None);
                if native
                    .default_actions
                    .iter()
                    .any(|action| matches!(action, DefaultAction::ToggleChecked(..)))
                {
                    self.queue_pending_dispatch(node, "change", None);
                }
                Ok(JsvValue::Undefined)
            }
            (object, "addEventListener") => {
                if object != HOST_WINDOW && object != HOST_DOCUMENT {
                    let _ = self.locator(object)?;
                }
                if arguments.len() < 2
                    || !matches!(
                        arguments[1],
                        JsvValue::Function(_, _, _, _) | JsvValue::AsyncFunction(_, _, _, _)
                    )
                {
                    return Err(
                        "TypeError: addEventListener requires an event type and function"
                            .to_string(),
                    );
                }
                let event_type = Self::argument_string(&arguments[0], "Event type")?;
                let options = arguments.get(2);
                let capture = Self::option_bool(options, "capture");
                let once = Self::option_bool(options, "once");
                self.register_listener(object, &event_type, capture, once, arguments[1].clone())?;
                Ok(JsvValue::Undefined)
            }
            (object, "removeEventListener") => {
                if arguments.len() < 2 {
                    return Err(
                        "TypeError: removeEventListener requires an event type and function"
                            .to_string(),
                    );
                }
                let event_type = Self::argument_string(&arguments[0], "Event type")?;
                self.remove_listener(object, &event_type, &arguments[1]);
                Ok(JsvValue::Undefined)
            }
            (object, "dispatchEvent") => {
                let JsvValue::HostObject(event_id) = arguments
                    .first()
                    .ok_or_else(|| "TypeError: dispatchEvent requires an Event".to_string())?
                else {
                    return Err("TypeError: dispatchEvent requires an Event object".to_string());
                };
                let record = self
                    .state
                    .events
                    .get(event_id)
                    .cloned()
                    .ok_or_else(|| "InvalidStateError: Event is detached".to_string())?;
                let target = if matches!(object, HOST_WINDOW | HOST_DOCUMENT) {
                    self.dom.root()
                } else {
                    self.locator(object)?
                };
                let mut event = crate::live_dom::DomEvent::new(&record.event_type, target);
                event.bubbles = record.bubbles;
                event.cancelable = record.cancelable;
                event.detail = record.detail.clone();
                let native = self.dom.dispatch_event(target, &mut event)?;
                let prevented = native.default_prevented || event.default_prevented;
                self.queue_pending_dispatch(target, &record.event_type, record.detail.clone());
                Ok(JsvValue::Boolean(!prevented))
            }
            (element, "getContext") => {
                let kind = arguments
                    .first()
                    .map(|value| value.to_display_string())
                    .unwrap_or_default();
                let node = self.locator(element)?;
                if kind != "2d" {
                    return Ok(JsvValue::Null);
                }
                if !self.state.capabilities.contains(&HostCapability::Canvas2D) {
                    return Ok(JsvValue::Null);
                }
                if self.state.canvas_contexts.len() >= MAX_EVENT_RECORDS {
                    return Err("QuotaExceededError: canvas context budget exceeded".to_string());
                }
                let id = self.state.next_context_id;
                self.state.next_context_id = self
                    .state
                    .next_context_id
                    .checked_add(1)
                    .ok_or_else(|| "Canvas context identifier space exhausted".to_string())?;
                self.state.canvas_contexts.insert(
                    id,
                    CanvasState {
                        canvas: node,
                        ..CanvasState::default()
                    },
                );
                Ok(JsvValue::HostObject(id))
            }
            _ if self.state.canvas_contexts.contains_key(&object) => {
                let canvas = self
                    .state
                    .canvas_contexts
                    .get(&object)
                    .map(|state| state.canvas)
                    .ok_or_else(|| "InvalidStateError: canvas context is detached".to_string())?;
                match method {
                    "fillRect" | "strokeRect" => {
                        let numbers = arguments
                            .iter()
                            .take(4)
                            .map(|value| {
                                value
                                    .as_number()
                                    .filter(|value| value.is_finite())
                                    .map(|value| value.clamp(-1_000_000.0, 1_000_000.0) as f32)
                                    .ok_or_else(|| {
                                        "TypeError: canvas rect requires numbers".to_string()
                                    })
                            })
                            .collect::<Result<Vec<f32>, String>>()?;
                        let x = numbers.first().copied().unwrap_or(0.0);
                        let y = numbers.get(1).copied().unwrap_or(0.0);
                        let w = numbers.get(2).copied().unwrap_or(0.0);
                        let h = numbers.get(3).copied().unwrap_or(0.0);
                        let fill = if method == "fillRect" {
                            self.state
                                .canvas_contexts
                                .get(&object)
                                .and_then(|state| parse_css_color_host(&state.fill))
                        } else {
                            self.state
                                .canvas_contexts
                                .get(&object)
                                .and_then(|state| parse_css_color_host(&state.stroke))
                        };
                        self.dom.add_canvas_shape(
                            canvas,
                            crate::paint::VectorShape {
                                kind: crate::paint::VectorShapeKind::Rect,
                                x,
                                y,
                                w,
                                h,
                                fill,
                                stroke: None,
                                stroke_width: 1.0,
                            },
                        )?;
                        Ok(JsvValue::Undefined)
                    }
                    "clearRect" => {
                        self.dom.clear_canvas_shapes(canvas)?;
                        Ok(JsvValue::Undefined)
                    }
                    "fillText" => {
                        // Text rasterization on canvas is outside the bounded
                        // vector-shape profile; accepted and ignored.
                        Ok(JsvValue::Undefined)
                    }
                    "getImageData" => {
                        let number = |index: usize, label: &str| -> Result<f64, String> {
                            arguments
                                .get(index)
                                .and_then(JsvValue::as_number)
                                .filter(|value| value.is_finite())
                                .ok_or_else(|| {
                                    format!("TypeError: getImageData {label} must be a number")
                                })
                        };
                        let read_x = number(0, "x")?.floor() as i32;
                        let read_y = number(1, "y")?.floor() as i32;
                        let width = number(2, "width")?.floor() as i32;
                        let height = number(3, "height")?.floor() as i32;
                        if width <= 0 || height <= 0 {
                            return Err("IndexSizeError: getImageData dimensions must be positive"
                                .to_string());
                        }
                        let pixels =
                            (width as usize)
                                .checked_mul(height as usize)
                                .ok_or_else(|| {
                                    "QuotaExceededError: canvas readback is too large".to_string()
                                })?;
                        if pixels > 4096 {
                            return Err("QuotaExceededError: canvas readback exceeds 4096 pixels"
                                .to_string());
                        }

                        let mut rgba = vec![0_u8; pixels * 4];
                        for shape in self.dom.canvas_shapes(canvas) {
                            if shape.kind != crate::paint::VectorShapeKind::Rect {
                                continue;
                            }
                            let Some(fill) = shape.fill else {
                                continue;
                            };
                            let left = shape.x.floor() as i32;
                            let top = shape.y.floor() as i32;
                            let right = (shape.x + shape.w).ceil() as i32;
                            let bottom = (shape.y + shape.h).ceil() as i32;
                            let r = (fill.r.clamp(0.0, 1.0) * 255.0).round() as u8;
                            let g = (fill.g.clamp(0.0, 1.0) * 255.0).round() as u8;
                            let b = (fill.b.clamp(0.0, 1.0) * 255.0).round() as u8;
                            let a = (fill.a.clamp(0.0, 1.0) * 255.0).round() as u8;
                            for row in 0..height {
                                let canvas_y = read_y.saturating_add(row);
                                if canvas_y < top || canvas_y >= bottom {
                                    continue;
                                }
                                for column in 0..width {
                                    let canvas_x = read_x.saturating_add(column);
                                    if canvas_x < left || canvas_x >= right {
                                        continue;
                                    }
                                    let offset =
                                        ((row as usize * width as usize) + column as usize) * 4;
                                    rgba[offset..offset + 4].copy_from_slice(&[r, g, b, a]);
                                }
                            }
                        }
                        let origin = runtime_origin(&self.state.current_url);
                        let mut protector =
                            crate::tracking_protection::CanvasFingerprintProtector::for_origin(
                                true, &origin,
                            );
                        protector.scramble_pixel_buffer(&mut rgba);
                        self.report.canvas_readbacks =
                            self.report.canvas_readbacks.saturating_add(1);
                        self.record_platform_operation("canvas.getImageData");

                        let data = JsvValue::Array(Rc::new(RefCell::new(
                            rgba.into_iter()
                                .map(|byte| JsvValue::Number(f64::from(byte)))
                                .collect(),
                        )));
                        let mut properties = std::collections::HashMap::new();
                        properties.insert("data".to_string(), data);
                        properties.insert("width".to_string(), JsvValue::Number(f64::from(width)));
                        properties
                            .insert("height".to_string(), JsvValue::Number(f64::from(height)));
                        Ok(JsvValue::Object(Rc::new(RefCell::new(
                            crate::javascript::JsvObject {
                                properties,
                                prototype: None,
                            },
                        ))))
                    }
                    _ => Err(format!(
                        "SecurityError: CanvasRenderingContext2D method '{}' is unavailable",
                        method
                    )),
                }
            }
            (element, "attachShadow") => {
                let node = self.locator(element)?;
                let options = arguments.first();
                let mode = match Self::option_string(options, "mode").as_deref() {
                    Some("open") | None => crate::live_dom::ShadowMode::Open,
                    Some("closed") => crate::live_dom::ShadowMode::Closed,
                    _ => {
                        return Err(
                            "TypeError: attachShadow mode must be 'open' or 'closed'".to_string()
                        )
                    }
                };
                let root = self.dom.attach_shadow(node, mode)?;
                self.report.dom_mutations = self.report.dom_mutations.saturating_add(1);
                self.element_for(root)
            }
            (form, "submit") => {
                let node = self.locator(form)?;
                if self.element_tag(node).as_deref() != Some("form") {
                    return Err("TypeError: submit is only available on form elements".to_string());
                }
                self.queue_pending_dispatch(form, "submit", None);
                Ok(JsvValue::Undefined)
            }
            _ => Err(format!(
                "SecurityError: host method '{}' is unavailable",
                method
            )),
        }
    }
}

/// Persistent per-document page runtime (Phase 21).
///
/// Owns the live DOM, one interpreter, the Realm, JavaScript event
/// listeners, timers, origin storage and session history for the whole
/// document lifetime. Script turns reuse the same engine so top-level
/// bindings, closures and registered callbacks survive across `<script>`
/// tags, event dispatches and timer pumps.
pub struct PageRuntime {
    live_dom: LiveDocument,
    engine: crate::javascript::JsvEngine,
    realm: RuntimeRealm,
    state: PageRuntimeState,
    report: RuntimeReport,
}

impl PageRuntime {
    /// Create a page runtime from a parsed element tree.
    pub fn from_element(
        dom: &Element,
        css_rules: Vec<CssRule>,
        viewport_width: u32,
        base_url: &str,
    ) -> Result<Self, String> {
        Self::from_element_with_storage_dir(dom, css_rules, viewport_width, base_url, None::<&Path>)
    }

    /// Create a page runtime with an optional browser-profile storage root.
    /// Each origin receives a stable private directory beneath this root.
    #[allow(clippy::field_reassign_with_default)]
    pub fn from_element_with_storage_dir(
        dom: &Element,
        css_rules: Vec<CssRule>,
        viewport_width: u32,
        base_url: &str,
        storage_dir: Option<impl AsRef<Path>>,
    ) -> Result<Self, String> {
        let mut realm = RuntimeRealm::new(1, crate::runtime_core::RuntimeLimits::default())?;
        realm.heap.set_property(
            realm.document,
            "URL",
            crate::runtime_core::RuntimeValue::String(base_url.to_string()),
        )?;
        let mut state = PageRuntimeState::default();
        state.current_url = base_url.to_string();
        state.history = PageHistory::new(base_url, "");
        let origin = runtime_origin(base_url);
        let origin_storage = storage_dir.as_ref().map(|root| {
            root.as_ref()
                .join("web-platform")
                .join(format!("origin-{:016x}", stable_origin_hash(&origin)))
        });
        state.indexeddb = crate::indexeddb::IndexedDBEngine::new(
            &origin,
            origin_storage
                .as_ref()
                .map(|path| path.join("indexeddb.json")),
        );
        state.cache_storage = crate::cache_api::CacheStorage::new(
            &origin,
            origin_storage.as_ref().map(|path| path.join("caches.json")),
        );
        state.service_workers = crate::service_worker::ServiceWorkerContainer::new(&origin);
        // Host element handles start above the reserved host-object ids.
        state.next_element_id = 32;
        state.capabilities.insert(HostCapability::Canvas2D);
        state.capabilities.insert(HostCapability::SvgShapes);
        let live_dom = LiveDocument::from_element(dom, css_rules, viewport_width);
        Ok(Self {
            live_dom,
            engine: crate::javascript::JsvEngine::new(),
            realm,
            state,
            report: RuntimeReport::default(),
        })
    }

    /// Create a page runtime from raw HTML (parsing is bounded by the
    /// parser's own limits).
    pub fn from_html(
        html: &str,
        css_rules: Vec<CssRule>,
        viewport_width: u32,
        base_url: &str,
    ) -> Result<Self, String> {
        Self::from_element(
            &crate::parser::parse_html(html),
            css_rules,
            viewport_width,
            base_url,
        )
    }

    /// Raw-HTML counterpart of [`Self::from_element_with_storage_dir`].
    pub fn from_html_with_storage_dir(
        html: &str,
        css_rules: Vec<CssRule>,
        viewport_width: u32,
        base_url: &str,
        storage_dir: Option<impl AsRef<Path>>,
    ) -> Result<Self, String> {
        Self::from_element_with_storage_dir(
            &crate::parser::parse_html(html),
            css_rules,
            viewport_width,
            base_url,
            storage_dir,
        )
    }

    /// Run one script through the persistent engine. Returns the script's
    /// completion value; the report collects errors.
    pub fn execute_script(&mut self, script: &str) -> Result<JsvValue, String> {
        self.report.scripts_seen = self.report.scripts_seen.saturating_add(1);
        if script.len() > MAX_SCRIPT_BYTES {
            self.report.truncated = true;
            return Err("Inline script exceeds 2 MB".to_string());
        }
        let instruction_cost = u64::try_from(script.chars().count()).unwrap_or(u64::MAX);
        if let Err(error) = self.realm.execution.consume(instruction_cost) {
            self.report.truncated = true;
            return Err(error);
        }
        let result = {
            let mut host = RuntimeHost::new(
                &mut self.live_dom,
                &mut self.report,
                &mut self.realm,
                &mut self.state,
            );
            self.engine.execute_with_host(script, &mut host)
        };
        self.flush_pending()?;
        result
    }

    /// Evaluate a snippet in the page context (embedders and tests).
    pub fn evaluate(&mut self, code: &str) -> Result<JsvValue, String> {
        let result = {
            let mut host = RuntimeHost::new(
                &mut self.live_dom,
                &mut self.report,
                &mut self.realm,
                &mut self.state,
            );
            self.engine.execute_with_host(code, &mut host)
        };
        self.flush_pending()?;
        result
    }

    /// Execute every inline `<script>` in the current document through the
    /// persistent engine (external-script loading is owned by the network
    /// layer; only inline bodies execute here).
    pub fn run_document(&mut self) -> Result<(), String> {
        let dom = self.dom_element();
        let scripts: Vec<String> = dom
            .find_all_tags("script")
            .into_iter()
            .filter(|script| script.get_attr("src").is_none())
            .take(MAX_SCRIPTS + 1)
            .map(|script| script.text.clone())
            .collect();
        if scripts.len() > MAX_SCRIPTS {
            self.report.truncated = true;
        }
        for script in scripts.into_iter().take(MAX_SCRIPTS) {
            match self.execute_script(&script) {
                Ok(_) => {
                    self.report.scripts_executed = self.report.scripts_executed.saturating_add(1);
                }
                Err(error) => self.report.errors.push(error),
            }
        }
        Ok(())
    }

    pub fn live_document(&self) -> &LiveDocument {
        &self.live_dom
    }

    /// Refresh the retained render state (recompute styles/layout/paint when
    /// dirty) and return it.
    pub fn refresh_render(&mut self) -> &crate::live_dom::LiveRenderState {
        self.live_dom.refresh()
    }

    pub fn report(&self) -> &RuntimeReport {
        &self.report
    }

    pub fn into_report(mut self) -> RuntimeReport {
        self.report.scheduled_tasks = self.state.timers.len();
        self.report.realm_heap_bytes = self.realm.heap.used_bytes();
        self.report.truncated |= self.state.history.was_truncated();
        self.report
    }

    /// Refresh the retained render state and return the current DOM element
    /// tree (used to feed the renderer after script turns).
    pub fn dom_element(&mut self) -> Element {
        self.live_dom.refresh().dom.clone()
    }

    pub fn current_url(&self) -> &str {
        &self.state.current_url
    }

    /// Register a module source for dynamic `import()` (Phase 21 persistent
    /// module loading). Sources are supplied by the embedder/network layer;
    /// the runtime performs no I/O.
    pub fn register_module(&mut self, specifier: &str, source: &str) -> Result<(), String> {
        self.engine.modules.register(specifier, source)
    }

    pub fn history_length(&self) -> usize {
        self.state.history.length()
    }

    pub fn pending_timers(&self) -> usize {
        self.state.timers.len()
    }

    pub fn now_ms(&self) -> u64 {
        self.state.now_ms
    }

    /// Combined native + JavaScript click dispatch with default actions.
    pub fn click(&mut self, target: NodeId) -> Result<DispatchReport, String> {
        let mut event = DomEvent::new("click", target);
        let mut report = self.live_dom.dispatch_event(target, &mut event)?;
        if !event.propagation_stopped {
            report.invoked_listeners = report
                .invoked_listeners
                .saturating_add(self.js_dispatch_pass(target, &mut event)?);
        }
        if !event.default_prevented {
            let actions = self.live_dom.apply_click_default(target)?;
            let has_change = actions
                .iter()
                .any(|action| matches!(action, DefaultAction::ToggleChecked(..)));
            let submitted_form = actions.iter().find_map(|action| match action {
                DefaultAction::SubmitForm(form) => Some(*form),
                _ => None,
            });
            let selected = actions
                .iter()
                .any(|action| matches!(action, DefaultAction::SelectOption(..)));
            report.default_actions.extend(actions);
            if has_change || selected {
                let mut change = DomEvent::new("change", target);
                let _ = self.live_dom.dispatch_event(target, &mut change)?;
                let _ = self.js_dispatch_pass(target, &mut change)?;
            }
            if let Some(form) = submitted_form {
                self.run_submit_flow(form)?;
            }
        }
        report.default_prevented = event.default_prevented;
        Ok(report)
    }

    /// The cancelable `submit` event followed by constraint validation and
    /// browser-owned submission when nothing prevented or failed.
    fn run_submit_flow(&mut self, form: NodeId) -> Result<(), String> {
        let mut submit = DomEvent::new("submit", form);
        let _ = self.js_dispatch_pass(form, &mut submit)?;
        if submit.default_prevented {
            return Ok(());
        }
        if self.validate_form(form)? {
            self.collect_submission(form)?;
        }
        Ok(())
    }

    /// Constraint validation for the documented Phase 21 profile: `required`
    /// controls must hold a non-empty value. Failures are recorded in the
    /// report and block the submission.
    fn validate_form(&mut self, form: NodeId) -> Result<bool, String> {
        let mut valid = true;
        for node in self.live_dom.document_order() {
            if node == form || !self.is_descendant_of(node, form) {
                continue;
            }
            if self.live_dom.get_attribute(node, "required").is_none() {
                continue;
            }
            let Some(name) = self
                .live_dom
                .get_attribute(node, "name")
                .map(str::to_string)
            else {
                continue;
            };
            let tag = match &self.live_dom.node(node).map(|entry| &entry.kind) {
                Some(crate::live_dom::LiveNodeKind::Element { tag, .. }) => tag.clone(),
                _ => continue,
            };
            let empty = match tag.as_str() {
                "textarea" => self
                    .live_dom
                    .text_content(node)
                    .unwrap_or_default()
                    .is_empty(),
                "select" => self
                    .selected_option_value_public(node)
                    .unwrap_or_default()
                    .is_empty(),
                "input" => match self.live_dom.get_attribute(node, "type").unwrap_or("text") {
                    "checkbox" | "radio" => self.live_dom.get_attribute(node, "checked").is_none(),
                    _ => self
                        .live_dom
                        .get_attribute(node, "value")
                        .unwrap_or("")
                        .is_empty(),
                },
                _ => false,
            };
            if empty {
                valid = false;
                if self.report.validation_errors.len() < 64 {
                    self.report
                        .validation_errors
                        .push(format!("required field '{}' is empty", name));
                }
            }
        }
        Ok(valid)
    }

    /// Combined native + JavaScript focus transition, including the `change`
    /// event for text-entry controls that lose focus.
    pub fn focus(&mut self, node: NodeId) -> Result<(), String> {
        let previous = self.live_dom.focused();
        self.live_dom.focus(node)?;
        let mut focus = DomEvent::new("focus", node);
        focus.bubbles = false;
        let _ = self.js_dispatch_pass(node, &mut focus)?;
        let mut focusin = DomEvent::new("focusin", node);
        let _ = self.js_dispatch_pass(node, &mut focusin)?;
        if let Some(previous) = previous.filter(|candidate| *candidate != node) {
            let mut blur = DomEvent::new("blur", previous);
            blur.bubbles = false;
            let _ = self.js_dispatch_pass(previous, &mut blur)?;
            let mut focusout = DomEvent::new("focusout", previous);
            let _ = self.js_dispatch_pass(previous, &mut focusout)?;
            if self.is_text_entry(previous) {
                let mut change = DomEvent::new("change", previous);
                let _ = self.js_dispatch_pass(previous, &mut change)?;
            }
        }
        Ok(())
    }

    /// Combined native + JavaScript keyboard dispatch with text entry and
    /// form defaults.
    pub fn dispatch_keyboard(
        &mut self,
        event_type: &str,
        key: &str,
    ) -> Result<DispatchReport, String> {
        let target = self
            .live_dom
            .focused()
            .unwrap_or_else(|| self.live_dom.root());
        let mut event = DomEvent::new(event_type, target);
        event.key = Some(key.chars().take(128).collect());
        let mut report = self.live_dom.dispatch_event(target, &mut event)?;
        report.invoked_listeners = report
            .invoked_listeners
            .saturating_add(self.js_dispatch_pass(target, &mut event)?);
        if event_type == "keydown" && !event.default_prevented {
            let actions = self.live_dom.apply_key_default(target, key)?;
            let has_input = actions
                .iter()
                .any(|action| matches!(action, DefaultAction::InsertText(..)));
            report.default_actions.extend(actions);
            if has_input {
                let mut input = DomEvent::new("input", target);
                let _ = self.live_dom.dispatch_event(target, &mut input)?;
                let _ = self.js_dispatch_pass(target, &mut input)?;
            }
        }
        report.default_prevented = event.default_prevented;
        Ok(report)
    }

    /// Combined native + JavaScript pointer dispatch.
    pub fn dispatch_pointer(
        &mut self,
        event_type: &str,
        x: i32,
        y: i32,
    ) -> Result<Option<DispatchReport>, String> {
        self.live_dom.refresh();
        let Some(target) = self.live_dom.hit_test(x as f64, y as f64) else {
            return Ok(None);
        };
        let mut event = DomEvent::new(event_type, target);
        event.pointer_x = Some(x);
        event.pointer_y = Some(y);
        let mut report = self.live_dom.dispatch_event(target, &mut event)?;
        report.invoked_listeners = report
            .invoked_listeners
            .saturating_add(self.js_dispatch_pass(target, &mut event)?);
        Ok(Some(report))
    }

    /// Dispatch a host-created event (CustomEvent-style) through both the
    /// native and JavaScript listener passes.
    pub fn dispatch_custom_event(
        &mut self,
        target: NodeId,
        event_type: &str,
        detail: Option<String>,
        bubbles: bool,
    ) -> Result<DispatchReport, String> {
        let mut event = DomEvent::new(event_type, target);
        event.bubbles = bubbles;
        event.detail = detail;
        let mut report = self.live_dom.dispatch_event(target, &mut event)?;
        report.invoked_listeners = report
            .invoked_listeners
            .saturating_add(self.js_dispatch_pass(target, &mut event)?);
        Ok(report)
    }

    /// Advance the timer clock and run every due callback through the
    /// persistent engine. Interval timers reschedule before invocation so a
    /// slow callback cannot starve later due times.
    pub fn pump_timers(&mut self, elapsed_ms: u64) -> Result<usize, String> {
        self.state.now_ms = self.state.now_ms.saturating_add(elapsed_ms);
        let mut fired = 0usize;
        loop {
            if fired >= MAX_TIMERS_PER_PUMP {
                break;
            }
            let due = self
                .state
                .timers
                .iter()
                .filter(|(_, timer)| timer.due_ms <= self.state.now_ms)
                .min_by_key(|(id, timer)| (timer.due_ms, **id))
                .map(|(id, timer)| (*id, timer.clone()));
            let Some((id, timer)) = due else {
                break;
            };
            if timer.interval {
                if let Some(entry) = self.state.timers.get_mut(&id) {
                    entry.due_ms = self.state.now_ms.saturating_add(entry.delay_ms);
                }
            } else {
                self.state.timers.remove(&id);
            }
            let outcome = {
                let mut host = RuntimeHost::new(
                    &mut self.live_dom,
                    &mut self.report,
                    &mut self.realm,
                    &mut self.state,
                );
                self.engine
                    .invoke_callback(&timer.callback, Vec::new(), &mut host)
            };
            if let Err(error) = outcome {
                self.report.errors.push(error);
            }
            self.report.timers_fired = self.report.timers_fired.saturating_add(1);
            fired += 1;
        }
        self.flush_pending()?;
        Ok(fired)
    }

    /// Execute the JavaScript listener pass for an in-flight event over the
    /// capture → target → bubble path, honouring `stopPropagation` from both
    /// native and JavaScript listeners.
    fn js_dispatch_pass(&mut self, target: NodeId, event: &mut DomEvent) -> Result<usize, String> {
        let path = self.live_dom.event_path(target)?;
        let mut invoked = 0usize;
        // Capture phase: ancestors only, root first.
        for node in path.iter().skip(1).rev() {
            if event.propagation_stopped {
                break;
            }
            invoked = invoked.saturating_add(self.invoke_js_listeners(*node, event, true)?);
        }
        // At-target phase: capture and bubble listeners both fire here.
        if !event.immediate_propagation_stopped {
            invoked = invoked.saturating_add(self.invoke_js_listeners(target, event, true)?);
        }
        if !event.immediate_propagation_stopped {
            invoked = invoked.saturating_add(self.invoke_js_listeners(target, event, false)?);
        }
        // Bubble phase: ancestors, then the window.
        if event.bubbles && !event.propagation_stopped {
            for node in path.iter().skip(1) {
                if event.propagation_stopped {
                    break;
                }
                invoked = invoked.saturating_add(self.invoke_js_listeners(*node, event, false)?);
            }
            if !event.propagation_stopped {
                invoked = invoked.saturating_add(self.invoke_js_listeners(
                    LISTENER_WINDOW,
                    event,
                    false,
                )?);
            }
        }
        if invoked > 0 && self.report.events_dispatched.len() < 256 {
            self.report
                .events_dispatched
                .push(format!("{} {}", event.event_type, invoked));
        }
        Ok(invoked)
    }

    fn invoke_js_listeners(
        &mut self,
        node: u64,
        event: &mut DomEvent,
        capture: bool,
    ) -> Result<usize, String> {
        let listeners: Vec<JsListener> = self
            .state
            .listeners
            .get(&node)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|listener| {
                        listener.event_type == event.event_type && listener.capture == capture
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if listeners.is_empty() {
            return Ok(0);
        }
        let mut invoked = 0usize;
        let mut remove_once = Vec::new();
        for listener in listeners {
            if event.immediate_propagation_stopped {
                break;
            }
            let outcome = {
                let mut host = RuntimeHost::new(
                    &mut self.live_dom,
                    &mut self.report,
                    &mut self.realm,
                    &mut self.state,
                );
                host.begin_event(node, event);
                let result = self.engine.invoke_callback(
                    &listener.callback,
                    vec![JsvValue::HostObject(HOST_EVENT)],
                    &mut host,
                );
                host.finish_event(event);
                result
            };
            if let Err(error) = outcome {
                self.report.errors.push(error);
            }
            invoked = invoked.saturating_add(1);
            if listener.once {
                remove_once.push(listener.id);
            }
        }
        if !remove_once.is_empty() {
            if let Some(entries) = self.state.listeners.get_mut(&node) {
                entries.retain(|listener| !remove_once.contains(&listener.id));
            }
        }
        Ok(invoked)
    }

    fn is_text_entry(&self, node: NodeId) -> bool {
        matches!(
            self.live_dom.node(node).map(|entry| &entry.kind),
            Some(crate::live_dom::LiveNodeKind::Element { tag, .. })
                if matches!(tag.as_str(), "textarea")
                    || (tag == "input"
                        && !matches!(
                            self.live_dom.get_attribute(node, "type"),
                            Some("checkbox" | "radio" | "submit" | "button")
                        ))
        )
    }

    /// Drain dispatches queued by host calls during a script turn (JS-driven
    /// clicks, focus changes, popstate/hashchange, form submits). The native
    /// dispatch already happened inside the host; this pass executes the
    /// JavaScript listener side after the turn completes.
    pub fn flush_pending(&mut self) -> Result<usize, String> {
        let pending = std::mem::take(&mut self.state.pending_dispatches);
        let mut total = 0usize;
        for dispatch in pending {
            let target = if dispatch.node == LISTENER_WINDOW {
                self.live_dom.root()
            } else {
                if !self.live_dom.node(dispatch.node).is_some() {
                    continue;
                }
                dispatch.node
            };
            let mut event = DomEvent::new(&dispatch.event_type, target);
            event.detail = dispatch.detail;
            if event.event_type == "submit" {
                total = total.saturating_add(self.js_dispatch_pass(target, &mut event)?);
                if !event.default_prevented {
                    self.collect_submission(target)?;
                }
            } else {
                total = total.saturating_add(self.js_dispatch_pass(target, &mut event)?);
            }
        }
        Ok(total)
    }

    /// Serialize a form's named controls into a URL-encoded submission record
    /// owned by the browser (GET navigation / POST body boundary).
    fn collect_submission(&mut self, form: NodeId) -> Result<(), String> {
        let mut pairs = Vec::new();
        for node in self.live_dom.document_order() {
            if node == form || self.is_descendant_of(node, form) {
                let Some(name) = self
                    .live_dom
                    .get_attribute(node, "name")
                    .map(str::to_string)
                else {
                    continue;
                };
                let tag = match &self.live_dom.node(node).map(|entry| &entry.kind) {
                    Some(crate::live_dom::LiveNodeKind::Element { tag, .. }) => tag.clone(),
                    _ => continue,
                };
                let value = match tag.as_str() {
                    "textarea" => self.live_dom.text_content(node).unwrap_or_default(),
                    "select" => {
                        let mut selected = self
                            .live_dom
                            .get_attribute(node, "value")
                            .map(str::to_string);
                        if selected.is_none() {
                            for child in self.live_dom.document_order() {
                                if self.is_descendant_of(child, node)
                                    && matches!(
                                        &self.live_dom.node(child).map(|entry| &entry.kind),
                                        Some(crate::live_dom::LiveNodeKind::Element { tag, .. })
                                            if tag == "option"
                                    )
                                    && self.live_dom.get_attribute(child, "selected").is_some()
                                {
                                    selected = self
                                        .live_dom
                                        .get_attribute(child, "value")
                                        .map(str::to_string)
                                        .or_else(|| self.live_dom.text_content(child).ok());
                                }
                            }
                        }
                        selected.unwrap_or_default()
                    }
                    "input" => match self.live_dom.get_attribute(node, "type").unwrap_or("text") {
                        "checkbox" | "radio" => {
                            if self.live_dom.get_attribute(node, "checked").is_none() {
                                continue;
                            }
                            self.live_dom
                                .get_attribute(node, "value")
                                .unwrap_or("on")
                                .to_string()
                        }
                        "submit" | "button" | "reset" | "file" => continue,
                        _ => self
                            .live_dom
                            .get_attribute(node, "value")
                            .unwrap_or("")
                            .to_string(),
                    },
                    _ => continue,
                };
                if pairs.len() >= MAX_FORM_ENTRIES {
                    break;
                }
                pairs.push((name, value));
            }
        }
        if self.report.submitted_forms.len() < 128 {
            let body = pairs
                .iter()
                .map(|(name, value)| format!("{}={}", url_encode(name), url_encode(value)))
                .collect::<Vec<_>>()
                .join("&");
            let action = self
                .live_dom
                .get_attribute(form, "action")
                .unwrap_or("")
                .to_string();
            let method = self
                .live_dom
                .get_attribute(form, "method")
                .unwrap_or("get")
                .to_ascii_lowercase();
            self.report.submitted_forms.push(format!(
                "{} {} {}",
                method,
                action,
                body.chars().count()
            ));
        }
        Ok(())
    }

    fn is_descendant_of(&self, candidate: NodeId, ancestor: NodeId) -> bool {
        let mut current = self.live_dom.node(candidate).and_then(|node| node.parent);
        while let Some(id) = current {
            if id == ancestor {
                return true;
            }
            current = self.live_dom.node(id).and_then(|node| node.parent);
        }
        false
    }

    /// Selected `<option>` value of a `<select>` (attribute value first, then
    /// option text), used by validation and submission collection.
    fn selected_option_value_public(&self, node: NodeId) -> Option<String> {
        let mut first: Option<String> = None;
        let mut selected = self
            .live_dom
            .get_attribute(node, "value")
            .map(str::to_string);
        if selected.is_none() {
            for child in self.live_dom.document_order() {
                if !self.is_descendant_of(child, node) {
                    continue;
                }
                if matches!(
                    &self.live_dom.node(child).map(|entry| &entry.kind),
                    Some(crate::live_dom::LiveNodeKind::Element { tag, .. }) if tag == "option"
                ) {
                    let value = self
                        .live_dom
                        .get_attribute(child, "value")
                        .map(str::to_string)
                        .or_else(|| self.live_dom.text_content(child).ok());
                    if first.is_none() {
                        first = value.clone();
                    }
                    if self.live_dom.get_attribute(child, "selected").is_some() {
                        selected = value;
                    }
                }
            }
        }
        selected.or(first)
    }

    /// Drain all pending timers and dispatches and return the current DOM.
    pub fn settle(mut self) -> Element {
        while !self.state.timers.is_empty() && self.pump_timers(MAX_TIMER_DELAY_MS).unwrap_or(0) > 0
        {
            // Drain until the queue is empty or a budget error stops it.
        }
        self.dom_element()
    }
}

/// Parse a canvas fill/stroke style into RGBA (shared with the SVG shape
/// collector). Unsupported values resolve to `None` (fail closed).
fn parse_css_color_host(value: &str) -> Option<crate::paint::Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let parse = |s: &str| u8::from_str_radix(s, 16).ok();
        if hex.len() == 6 {
            let (r, g, b) = (&hex[0..2], &hex[2..4], &hex[4..6]);
            return Some(crate::paint::Rgba {
                r: parse(r)? as f32 / 255.0,
                g: parse(g)? as f32 / 255.0,
                b: parse(b)? as f32 / 255.0,
                a: 1.0,
            });
        }
        if hex.len() == 3 {
            let (r, g, b) = (&hex[0..1], &hex[1..2], &hex[2..3]);
            return Some(crate::paint::Rgba {
                r: parse(r)? as f32 / 15.0,
                g: parse(g)? as f32 / 15.0,
                b: parse(b)? as f32 / 15.0,
                a: 1.0,
            });
        }
    }
    let named = match value.to_ascii_lowercase().as_str() {
        "red" => (1.0, 0.0, 0.0),
        "green" | "lime" => (0.0, 1.0, 0.0),
        "blue" => (0.0, 0.0, 1.0),
        "black" => (0.0, 0.0, 0.0),
        "white" => (1.0, 1.0, 1.0),
        "yellow" => (1.0, 1.0, 0.0),
        "orange" => (1.0, 0.65, 0.0),
        "gray" | "grey" => (0.5, 0.5, 0.5),
        "purple" => (0.5, 0.0, 0.5),
        _ => return None,
    };
    Some(crate::paint::Rgba {
        r: named.0,
        g: named.1,
        b: named.2,
        a: 1.0,
    })
}

fn url_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char)
            }
            b' ' => output.push('+'),
            _ => output.push_str(&format!("%{:02X}", byte)),
        }
    }
    output
}

pub fn run_inline_scripts(dom: &mut Element, base_url: &str) -> RuntimeReport {
    let mut page = match PageRuntime::from_element(dom, Vec::new(), 800, base_url) {
        Ok(page) => page,
        Err(error) => {
            let mut report = RuntimeReport::default();
            report.errors.push(error);
            report.truncated = true;
            return report;
        }
    };
    let _ = page.run_document();
    page.report
        .console
        .extend(page.engine.console_output.clone());
    *dom = page.dom_element();
    page.into_report()
}

fn media_event_name(event: MediaEvent) -> &'static str {
    match event {
        MediaEvent::LoadStart => "loadstart",
        MediaEvent::DurationChange => "durationchange",
        MediaEvent::LoadedMetadata => "loadedmetadata",
        MediaEvent::LoadedData => "loadeddata",
        MediaEvent::CanPlay => "canplay",
        MediaEvent::CanPlayThrough => "canplaythrough",
        MediaEvent::Play => "play",
        MediaEvent::Playing => "playing",
        MediaEvent::Pause => "pause",
        MediaEvent::Seeking => "seeking",
        MediaEvent::Seeked => "seeked",
        MediaEvent::TimeUpdate => "timeupdate",
        MediaEvent::Progress => "progress",
        MediaEvent::Waiting => "waiting",
        MediaEvent::Stalled => "stalled",
        MediaEvent::RateChange => "ratechange",
        MediaEvent::VolumeChange => "volumechange",
        MediaEvent::Ended => "ended",
        MediaEvent::Error => "error",
        MediaEvent::Emptied => "emptied",
    }
}

fn resolve_same_origin(base_url: &str, candidate: &str) -> Option<String> {
    let base = url::Url::parse(base_url).ok()?;
    if !matches!(base.scheme(), "http" | "https") {
        return None;
    }
    let resolved = base.join(candidate).ok()?;
    (resolved.scheme() == base.scheme()
        && resolved.host_str() == base.host_str()
        && resolved.port_or_known_default() == base.port_or_known_default())
    .then(|| String::from(resolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn javascript_byte_array(bytes: &[u8]) -> String {
        format!(
            "[{}]",
            bytes
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    #[test]
    fn applies_dom_storage_and_same_origin_fetch_operations() {
        let mut dom = crate::parser::parse_html(
            "<p id='message'>old</p><script>document.getElementById('message').textContent='new';document.getElementById('message').setAttribute('aria-live','polite');localStorage.setItem('theme','dark');fetch('/api/data');fetch('https://evil.test/x');</script>",
        );
        let report = run_inline_scripts(&mut dom, "https://example.com/page");
        assert_eq!(
            crate::dom::get_element_by_id_mut(&mut dom, "message")
                .unwrap()
                .text,
            "new"
        );
        assert_eq!(report.dom_mutations, 2);
        assert_eq!(report.storage_writes, vec![("theme".into(), "dark".into())]);
        assert_eq!(report.fetch_requests, vec!["https://example.com/api/data"]);
    }

    #[test]
    fn event_loop_prioritizes_microtasks_and_bounds_timers() {
        let mut event_loop = BoundedEventLoop::new();
        event_loop.set_timeout("timer", 10).unwrap();
        event_loop.queue_microtask("microtask").unwrap();
        let first = event_loop.drain_ready(8);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, WebTaskKind::Microtask);

        event_loop.advance_time(10);
        let second = event_loop.drain_ready(8);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].source, "timer");

        assert!(event_loop
            .queue_microtask("x".repeat(MAX_TASK_SOURCE_BYTES + 1))
            .is_err());
        assert!(event_loop.was_truncated());
    }

    #[test]
    fn unicode_dom_assignment_never_slices_inside_a_character() {
        let value = "ắ".repeat(1_000);
        let mut dom = crate::parser::parse_html(&format!(
            "<p id='message'>old</p><script>document.getElementById('message').textContent='{value}'</script>"
        ));
        let report = run_inline_scripts(&mut dom, "https://example.test/");
        assert_eq!(report.dom_mutations, 1);
        assert_eq!(
            crate::dom::get_element_by_id_mut(&mut dom, "message")
                .unwrap()
                .text,
            value
        );
    }

    #[test]
    fn host_calls_follow_ast_values_not_source_text_scanning() {
        let mut dom = crate::parser::parse_html(
            "<p id='message'>old</p><script>let fake=\"fetch('https://evil.test/only-text')\";let target=document.getElementById('message');let value='new';target.textContent=value;let store=window.localStorage;store.setItem('theme','dark');let endpoint='/api/data';fetch(endpoint);</script>",
        );
        let report = run_inline_scripts(&mut dom, "https://example.com/page");

        assert_eq!(report.errors, Vec::<String>::new());
        assert_eq!(report.dom_mutations, 1);
        assert_eq!(
            crate::dom::get_element_by_id(&dom, "message").unwrap().text,
            "new"
        );
        assert_eq!(report.storage_writes, vec![("theme".into(), "dark".into())]);
        assert_eq!(report.fetch_requests, vec!["https://example.com/api/data"]);
        assert!(report.realm_heap_bytes > 0);
    }

    #[test]
    fn live_host_supports_create_append_attributes_focus_and_form_defaults() {
        let mut dom = crate::parser::parse_html(
            "<main><p id='message'>old</p><input id='choice' type='checkbox'><input id='name'><script>let message=document.getElementById('message');let child=document.createElement('span');child.className='created';child.textContent='new';message.appendChild(child);document.getElementById('choice').click();document.getElementById('name').focus();document.getElementById('name').addEventListener('input',value=>value);</script></main>",
        );
        let report = run_inline_scripts(&mut dom, "https://example.com/");
        let child = crate::dom::query_selector(&dom, ".created").unwrap();
        assert_eq!(child.text, "new");
        assert!(crate::dom::get_element_by_id(&dom, "choice")
            .unwrap()
            .attrs
            .contains_key("checked"));
        assert_eq!(report.event_listeners, 1);
        assert!(report.dom_mutations >= 4);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }

    #[test]
    fn host_capabilities_reject_unsupported_mutations_and_bound_work() {
        let mut dom = crate::parser::parse_html(
            "<p id='message'>old</p><script>let target=document.getElementById('message');target.onclick='bad()';</script>",
        );
        let report = run_inline_scripts(&mut dom, "https://example.com/");
        assert_eq!(
            crate::dom::get_element_by_id(&dom, "message").unwrap().text,
            "old"
        );
        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("not writable")));

        let operations = (0..(MAX_HOST_OPERATIONS / 2 + 2))
            .map(|_| "document.getElementById('message');")
            .collect::<String>();
        let mut dom = crate::parser::parse_html(&format!(
            "<p id='message'>old</p><script>{operations}</script>"
        ));
        let report = run_inline_scripts(&mut dom, "https://example.com/");
        assert!(report.truncated);
        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("host-operation budget")));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn media_source_and_html_media_element_are_javascript_visible_and_bounded() {
        let init = crate::iso_bmff::fixture::init(1, 1_000, b"vide", b"avc1");
        let payload = [1u8, 2, 3];
        let samples = vec![payload.as_slice(); 6];
        let media = crate::iso_bmff::fixture::media(1, 0, 1_000, &samples);
        let script = format!(
            "let video=document.createElement('video');\
             let source=MediaSource();\
             let buffer=source.addSourceBuffer('video/mp4; codecs=\"avc1\"');\
             buffer.appendBuffer({});\
             buffer.appendBuffer({});\
             source.duration=6;\
             source.endOfStream();\
             video.srcObject=source;\
             video.play();\
             video.pause();\
             video.currentTime=2;\
             video.volume=0.4;",
            javascript_byte_array(&init),
            javascript_byte_array(&media),
        );
        let mut dom = crate::parser::parse_html(&format!("<main><script>{script}</script></main>"));
        let report = run_inline_scripts(&mut dom, "https://media.test/watch");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.media_elements, 1);
        assert_eq!(report.media_sources, 1);
        assert_eq!(report.source_buffers, 1);
        assert!(report
            .media_events
            .iter()
            .any(|event| event.ends_with(":playing")));
        assert!(report
            .media_events
            .iter()
            .any(|event| event.ends_with(":seeked")));
        assert!(report.realm_heap_bytes > 0);
    }
}

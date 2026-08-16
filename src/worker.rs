//! Length-prefixed IPC boundary for untrusted document preparation.
//!
//! The desktop process never parses untrusted HTML directly. A short-lived
//! worker receives a bounded request, performs parse/style/layout work and
//! returns a bounded response. A crash, panic or timeout is reported as a tab
//! load failure instead of terminating the browser shell.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::css_parser::CssRule;
use crate::document::PreparedDocument;

pub const MAX_WORKER_REQUEST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_WORKER_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
pub const DEFAULT_WORKER_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_WORKER_STDERR_BYTES: u64 = 64 * 1024;
const PDF_REQUEST_MAGIC: &[u8; 8] = b"GHPDF001";
const COMPRESSED_PAYLOAD_MAGIC: &[u8; 8] = b"GHZ10001";
const COMPRESSION_THRESHOLD: usize = 64 * 1024;
#[cfg(windows)]
const WORKER_PROCESS_MEMORY_LIMIT: usize = 512 * 1024 * 1024;

#[cfg(windows)]
#[derive(Debug)]
struct WorkerContainment {
    job: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl WorkerContainment {
    fn attach(child: &std::process::Child) -> Result<Self, WorkerError> {
        use std::os::windows::io::AsRawHandle;

        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };

        let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|error| WorkerError::Io(format!("cannot create worker job: {error}")))?;
        let containment = Self { job };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        limits.ProcessMemoryLimit = WORKER_PROCESS_MEMORY_LIMIT;
        unsafe {
            SetInformationJobObject(
                containment.job,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(|error| {
                WorkerError::Io(format!("cannot configure worker job limits: {error}"))
            })?;
            let process = HANDLE(child.as_raw_handle() as isize);
            AssignProcessToJobObject(containment.job, process).map_err(|error| {
                WorkerError::Io(format!("cannot contain worker process: {error}"))
            })?;
        }
        Ok(containment)
    }
}

#[cfg(windows)]
impl Drop for WorkerContainment {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

#[cfg(windows)]
pub fn apply_restricted_worker_token() -> Result<(), WorkerError> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, PSID};
    use windows::Win32::Security::{
        AllocateAndInitializeSid, CreateRestrictedToken, DuplicateTokenEx, FreeSid,
        IsTokenRestricted, SecurityImpersonation, TokenImpersonation, DISABLE_MAX_PRIVILEGE,
        SECURITY_NULL_SID_AUTHORITY, SID_AND_ATTRIBUTES, TOKEN_DUPLICATE, TOKEN_IMPERSONATE,
        TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken, SetThreadToken};

    struct TokenHandle(HANDLE);
    impl Drop for TokenHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    struct SidHandle(PSID);
    impl Drop for SidHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = FreeSid(self.0);
            }
        }
    }

    let mut process_token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_IMPERSONATE,
            &mut process_token,
        )
    }
    .map_err(|error| WorkerError::Io(format!("cannot open the worker token: {error}")))?;
    let process_token = TokenHandle(process_token);
    let null_authority = SECURITY_NULL_SID_AUTHORITY;
    let mut null_sid = PSID::default();
    unsafe {
        AllocateAndInitializeSid(
            std::ptr::addr_of!(null_authority),
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut null_sid,
        )
    }
    .map_err(|error| WorkerError::Io(format!("cannot allocate worker restriction SID: {error}")))?;
    let null_sid = SidHandle(null_sid);
    let restricting_sids = [SID_AND_ATTRIBUTES {
        Sid: null_sid.0,
        Attributes: 0,
    }];
    let mut restricted_token = HANDLE::default();
    unsafe {
        CreateRestrictedToken(
            process_token.0,
            DISABLE_MAX_PRIVILEGE,
            None,
            None,
            Some(&restricting_sids),
            &mut restricted_token,
        )
    }
    .map_err(|error| {
        WorkerError::Io(format!("cannot create a restricted worker token: {error}"))
    })?;
    let restricted_token = TokenHandle(restricted_token);
    let mut impersonation_token = HANDLE::default();
    unsafe {
        DuplicateTokenEx(
            restricted_token.0,
            TOKEN_QUERY | TOKEN_IMPERSONATE,
            None,
            SecurityImpersonation,
            TokenImpersonation,
            &mut impersonation_token,
        )
    }
    .map_err(|error| {
        WorkerError::Io(format!(
            "cannot create a restricted impersonation token: {error}"
        ))
    })?;
    let impersonation_token = TokenHandle(impersonation_token);
    unsafe { SetThreadToken(None, impersonation_token.0) }.map_err(|error| {
        WorkerError::Io(format!("cannot apply the restricted worker token: {error}"))
    })?;
    unsafe { IsTokenRestricted(impersonation_token.0) }
        .map_err(|error| WorkerError::Io(format!("worker token is not restricted: {error}")))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn apply_restricted_worker_token() -> Result<(), WorkerError> {
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreparationRequest {
    pub html: String,
    pub fallback_title: String,
    pub base_rules: Vec<CssRule>,
    pub viewport_width: u32,
    pub viewport_height: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct PdfPreparationMeta {
    fallback_title: String,
    base_rules: Vec<CssRule>,
    viewport_width: u32,
    viewport_height: u32,
}

#[derive(Debug, Serialize, Deserialize)]
enum PreparationResponse {
    Prepared(Box<PreparedDocument>),
    Error(String),
}

#[derive(Debug)]
pub enum WorkerError {
    Io(String),
    Protocol(String),
    Worker(String),
    Timeout,
    Cancelled,
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "worker I/O failed: {error}"),
            Self::Protocol(error) => write!(formatter, "worker protocol failed: {error}"),
            Self::Worker(error) => write!(formatter, "worker rejected document: {error}"),
            Self::Timeout => write!(formatter, "document worker timed out"),
            Self::Cancelled => write!(formatter, "document worker was cancelled"),
        }
    }
}

impl std::error::Error for WorkerError {}

/// Thread-safe cancellation state shared by a navigation and its worker.
///
/// `worker_started` is observable so callers and fault-injection tests can
/// distinguish pre-flight cancellation from termination of a live process.
#[derive(Clone, Debug, Default)]
pub struct WorkerCancellationToken {
    cancelled: Arc<AtomicBool>,
    worker_started: Arc<AtomicBool>,
}

impl WorkerCancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn worker_started(&self) -> bool {
        self.worker_started.load(Ordering::Acquire)
    }

    fn mark_worker_started(&self) {
        self.worker_started.store(true, Ordering::Release);
    }
}

pub fn prepare_document_isolated(
    html: &str,
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
) -> Result<PreparedDocument, WorkerError> {
    let worker = worker_executable()?;
    let request = PreparationRequest {
        html: html.to_string(),
        fallback_title: fallback_title.to_string(),
        base_rules: base_rules.to_vec(),
        viewport_width,
        viewport_height,
    };
    prepare_with_program(&worker, &request, DEFAULT_WORKER_TIMEOUT)
}

pub fn prepare_pdf_isolated(
    pdf_bytes: &[u8],
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
) -> Result<PreparedDocument, WorkerError> {
    let worker = worker_executable()?;
    prepare_pdf_with_program(
        &worker,
        pdf_bytes,
        fallback_title,
        base_rules,
        viewport_width,
        viewport_height,
        DEFAULT_WORKER_TIMEOUT,
    )
}

pub fn prepare_pdf_with_program(
    program: &Path,
    pdf_bytes: &[u8],
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
    timeout: Duration,
) -> Result<PreparedDocument, WorkerError> {
    prepare_pdf_with_program_cancellable(
        program,
        pdf_bytes,
        fallback_title,
        base_rules,
        viewport_width,
        viewport_height,
        timeout,
        &WorkerCancellationToken::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_pdf_with_program_cancellable(
    program: &Path,
    pdf_bytes: &[u8],
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
    timeout: Duration,
    cancellation: &WorkerCancellationToken,
) -> Result<PreparedDocument, WorkerError> {
    let meta = PdfPreparationMeta {
        fallback_title: fallback_title.to_string(),
        base_rules: base_rules.to_vec(),
        viewport_width,
        viewport_height,
    };
    let encoded_meta =
        serde_json::to_vec(&meta).map_err(|error| WorkerError::Protocol(error.to_string()))?;
    let meta_length = u32::try_from(encoded_meta.len())
        .map_err(|_| WorkerError::Protocol("PDF metadata is too large".to_string()))?;
    let mut payload = Vec::with_capacity(12 + encoded_meta.len() + pdf_bytes.len());
    payload.extend_from_slice(PDF_REQUEST_MAGIC);
    payload.extend_from_slice(&meta_length.to_le_bytes());
    payload.extend_from_slice(&encoded_meta);
    payload.extend_from_slice(pdf_bytes);
    let response = exchange_with_program(program, &payload, timeout, cancellation)?;
    match response {
        PreparationResponse::Prepared(document) => Ok(*document),
        PreparationResponse::Error(error) => Err(WorkerError::Worker(error)),
    }
}

pub fn prepare_with_program(
    program: &Path,
    request: &PreparationRequest,
    timeout: Duration,
) -> Result<PreparedDocument, WorkerError> {
    prepare_with_program_cancellable(program, request, timeout, &WorkerCancellationToken::new())
}

pub fn prepare_with_program_cancellable(
    program: &Path,
    request: &PreparationRequest,
    timeout: Duration,
    cancellation: &WorkerCancellationToken,
) -> Result<PreparedDocument, WorkerError> {
    let encoded =
        serde_json::to_vec(request).map_err(|error| WorkerError::Protocol(error.to_string()))?;
    let response = exchange_with_program(program, &encoded, timeout, cancellation)?;
    match response {
        PreparationResponse::Prepared(document) => Ok(*document),
        PreparationResponse::Error(error) => Err(WorkerError::Worker(error)),
    }
}

fn exchange_with_program(
    program: &Path,
    encoded: &[u8],
    timeout: Duration,
    cancellation: &WorkerCancellationToken,
) -> Result<PreparationResponse, WorkerError> {
    if encoded.len() > MAX_WORKER_REQUEST_BYTES {
        return Err(WorkerError::Protocol(format!(
            "request exceeds {} bytes",
            MAX_WORKER_REQUEST_BYTES
        )));
    }
    if cancellation.is_cancelled() {
        return Err(WorkerError::Cancelled);
    }

    let mut command = Command::new(program);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|error| WorkerError::Io(error.to_string()))?;
    #[cfg(windows)]
    let _containment = match WorkerContainment::attach(&child) {
        Ok(containment) => containment,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    cancellation.mark_worker_started();
    if cancellation.is_cancelled() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(WorkerError::Cancelled);
    }
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| WorkerError::Io("worker stdin is unavailable".to_string()))?;
    let wire_request = encode_wire_payload(encoded, MAX_WORKER_REQUEST_BYTES)?;
    write_frame(&mut stdin, &wire_request)?;
    drop(stdin);
    if cancellation.is_cancelled() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(WorkerError::Cancelled);
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| WorkerError::Io("worker stdout is unavailable".to_string()))?;
    let reader = std::thread::spawn(move || {
        read_frame(
            &mut stdout,
            MAX_WORKER_RESPONSE_BYTES + COMPRESSED_PAYLOAD_MAGIC.len() + 8,
        )
    });
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| WorkerError::Io("worker stderr is unavailable".to_string()))?;
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr
            .by_ref()
            .take(MAX_WORKER_STDERR_BYTES)
            .read_to_end(&mut bytes);
        result.map(|_| String::from_utf8_lossy(&bytes).trim().to_string())
    });

    let start = Instant::now();
    let status = loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let _ = stderr_reader.join();
            return Err(WorkerError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = stderr_reader.join();
                return Err(WorkerError::Timeout);
            }
            Err(error) => return Err(WorkerError::Io(error.to_string())),
        }
    };

    let stderr = stderr_reader
        .join()
        .map_err(|_| WorkerError::Protocol("worker stderr reader panicked".to_string()))?
        .map_err(|error| WorkerError::Io(error.to_string()))?;
    if !status.success() {
        let _ = reader.join();
        let detail = if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        };
        return Err(WorkerError::Worker(format!(
            "worker exited with status {status}{detail}"
        )));
    }
    let wire_payload = reader
        .join()
        .map_err(|_| WorkerError::Protocol("worker output reader panicked".to_string()))??;
    let payload = decode_wire_payload(wire_payload, MAX_WORKER_RESPONSE_BYTES)?;
    let response: PreparationResponse = serde_json::from_slice(&payload)
        .map_err(|error| WorkerError::Protocol(error.to_string()))?;
    Ok(response)
}

pub fn run_worker_stdio() -> Result<(), WorkerError> {
    apply_restricted_worker_token()?;
    let mut input = std::io::stdin().lock();
    let wire_payload = read_frame(
        &mut input,
        MAX_WORKER_REQUEST_BYTES + COMPRESSED_PAYLOAD_MAGIC.len() + 8,
    )?;
    let payload = decode_wire_payload(wire_payload, MAX_WORKER_REQUEST_BYTES)?;
    let response = if payload.starts_with(PDF_REQUEST_MAGIC) {
        prepare_pdf_request(&payload)
    } else {
        match serde_json::from_slice::<PreparationRequest>(&payload) {
            Ok(request) => {
                PreparationResponse::Prepared(Box::new(crate::document::prepare_document_static(
                    &request.html,
                    &request.fallback_title,
                    &request.base_rules,
                    request.viewport_width,
                    request.viewport_height,
                )))
            }
            Err(error) => PreparationResponse::Error(format!("invalid request: {error}")),
        }
    };
    let encoded =
        serde_json::to_vec(&response).map_err(|error| WorkerError::Protocol(error.to_string()))?;
    if encoded.len() > MAX_WORKER_RESPONSE_BYTES {
        return Err(WorkerError::Protocol(format!(
            "response exceeds {} bytes",
            MAX_WORKER_RESPONSE_BYTES
        )));
    }
    let wire_response = encode_wire_payload(&encoded, MAX_WORKER_RESPONSE_BYTES)?;
    let mut output = std::io::stdout().lock();
    write_frame(&mut output, &wire_response)
}

fn encode_wire_payload(payload: &[u8], limit: usize) -> Result<Vec<u8>, WorkerError> {
    if payload.len() > limit {
        return Err(WorkerError::Protocol(format!(
            "payload exceeds {limit} bytes"
        )));
    }
    if payload.len() < COMPRESSION_THRESHOLD {
        return Ok(payload.to_vec());
    }

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(payload)
        .map_err(|error| WorkerError::Io(error.to_string()))?;
    let compressed = encoder
        .finish()
        .map_err(|error| WorkerError::Io(error.to_string()))?;
    if compressed.len() + COMPRESSED_PAYLOAD_MAGIC.len() + 8 >= payload.len() {
        return Ok(payload.to_vec());
    }

    let mut wire = Vec::with_capacity(COMPRESSED_PAYLOAD_MAGIC.len() + 8 + compressed.len());
    wire.extend_from_slice(COMPRESSED_PAYLOAD_MAGIC);
    wire.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    wire.extend_from_slice(&compressed);
    Ok(wire)
}

fn decode_wire_payload(payload: Vec<u8>, limit: usize) -> Result<Vec<u8>, WorkerError> {
    if !payload.starts_with(COMPRESSED_PAYLOAD_MAGIC) {
        if payload.len() > limit {
            return Err(WorkerError::Protocol(format!(
                "payload exceeds {limit} bytes"
            )));
        }
        return Ok(payload);
    }
    if payload.len() < COMPRESSED_PAYLOAD_MAGIC.len() + 8 {
        return Err(WorkerError::Protocol(
            "truncated compressed payload header".to_string(),
        ));
    }

    let length_offset = COMPRESSED_PAYLOAD_MAGIC.len();
    let expected = usize::try_from(u64::from_le_bytes(
        payload[length_offset..length_offset + 8]
            .try_into()
            .map_err(|_| WorkerError::Protocol("invalid payload length".to_string()))?,
    ))
    .map_err(|_| WorkerError::Protocol("payload length overflow".to_string()))?;
    if expected > limit {
        return Err(WorkerError::Protocol(format!(
            "expanded payload exceeds {limit} bytes"
        )));
    }

    let mut decoder = flate2::read::ZlibDecoder::new(&payload[length_offset + 8..]);
    let mut decoded = Vec::with_capacity(expected.min(8 * 1024 * 1024));
    decoder
        .by_ref()
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut decoded)
        .map_err(|error| WorkerError::Protocol(format!("invalid compressed payload: {error}")))?;
    if decoded.len() != expected {
        return Err(WorkerError::Protocol(
            "compressed payload length mismatch".to_string(),
        ));
    }
    Ok(decoded)
}

fn prepare_pdf_request(payload: &[u8]) -> PreparationResponse {
    if payload.len() < 12 {
        return PreparationResponse::Error("truncated PDF request".to_string());
    }
    let meta_length =
        u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]) as usize;
    let Some(meta_end) = 12_usize.checked_add(meta_length) else {
        return PreparationResponse::Error("PDF metadata length overflow".to_string());
    };
    if meta_end > payload.len() {
        return PreparationResponse::Error("truncated PDF metadata".to_string());
    }
    let meta: PdfPreparationMeta = match serde_json::from_slice(&payload[12..meta_end]) {
        Ok(meta) => meta,
        Err(error) => {
            return PreparationResponse::Error(format!("invalid PDF metadata: {error}"));
        }
    };
    let html = match crate::pdf::render_to_html(&payload[meta_end..], &meta.fallback_title) {
        Ok(html) => html,
        Err(error) => return PreparationResponse::Error(error.to_string()),
    };
    PreparationResponse::Prepared(Box::new(crate::document::prepare_document_static(
        &html,
        &meta.fallback_title,
        &meta.base_rules,
        meta.viewport_width,
        meta.viewport_height,
    )))
}

fn worker_executable() -> Result<PathBuf, WorkerError> {
    if let Some(explicit) = std::env::var_os("GHITA_RENDERER_WORKER") {
        return Ok(PathBuf::from(explicit));
    }
    let current = std::env::current_exe().map_err(|error| WorkerError::Io(error.to_string()))?;
    let suffix = std::env::consts::EXE_SUFFIX;
    Ok(current.with_file_name(format!("ghita-renderer-worker{suffix}")))
}

fn write_frame(writer: &mut impl Write, payload: &[u8]) -> Result<(), WorkerError> {
    let length = u64::try_from(payload.len())
        .map_err(|_| WorkerError::Protocol("frame length overflow".to_string()))?;
    writer
        .write_all(&length.to_le_bytes())
        .and_then(|_| writer.write_all(payload))
        .and_then(|_| writer.flush())
        .map_err(|error| WorkerError::Io(error.to_string()))
}

fn read_frame(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, WorkerError> {
    let mut length_bytes = [0_u8; 8];
    reader
        .read_exact(&mut length_bytes)
        .map_err(|error| WorkerError::Io(error.to_string()))?;
    let length = usize::try_from(u64::from_le_bytes(length_bytes))
        .map_err(|_| WorkerError::Protocol("frame length overflow".to_string()))?;
    if length > limit {
        return Err(WorkerError::Protocol(format!(
            "frame exceeds {limit} bytes"
        )));
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|error| WorkerError::Io(error.to_string()))?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_and_limit() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"document").unwrap();
        assert_eq!(read_frame(&mut buffer.as_slice(), 32).unwrap(), b"document");
        assert!(read_frame(&mut buffer.as_slice(), 2).is_err());
    }

    #[test]
    fn request_serialization_is_bounded() {
        let request = PreparationRequest {
            html: "<h1>safe</h1>".to_string(),
            fallback_title: "local".to_string(),
            base_rules: Vec::new(),
            viewport_width: 800,
            viewport_height: 600,
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: PreparationRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.viewport_width, 800);
        assert_eq!(decoded.html, "<h1>safe</h1>");
    }

    #[test]
    fn compressed_wire_payload_round_trips_with_expansion_limit() {
        let payload = vec![b'a'; COMPRESSION_THRESHOLD * 2];
        let encoded = encode_wire_payload(&payload, payload.len()).unwrap();
        assert!(encoded.starts_with(COMPRESSED_PAYLOAD_MAGIC));
        assert!(encoded.len() < payload.len());
        assert_eq!(
            decode_wire_payload(encoded.clone(), payload.len()).unwrap(),
            payload
        );
        assert!(decode_wire_payload(encoded, 1_024).is_err());
    }
}

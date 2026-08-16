//! Bounded, origin-partitioned background work for controlled local web apps.
//!
//! This module deliberately models lifecycle, authority and budgets instead of
//! executing arbitrary scripts after UI exit. A browser integration supplies
//! the actual service-worker callback only while the matching registration and
//! permission remain valid.

use std::collections::{BTreeMap, VecDeque};

pub const MAX_WORKERS_PER_ORIGIN: usize = 32;
pub const MAX_WORKERS_TOTAL: usize = 256;
pub const MAX_QUEUED_TASKS_PER_WORKER: usize = 256;
pub const MAX_TASK_PAYLOAD_BYTES: usize = 256 * 1024;
pub const MAX_QUEUED_PAYLOAD_BYTES_PER_WORKER: usize = 4 * 1024 * 1024;
pub const MAX_TASKS_PER_WAKE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundWorkerState {
    Registered,
    Running,
    Sleeping,
    Stopped,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskKind {
    Fetch,
    Sync,
    Push,
    NotificationClick,
    Message,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundTask {
    pub kind: BackgroundTaskKind,
    pub payload: Vec<u8>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundWorkerPolicy {
    pub max_wake_ms: u64,
    pub max_tasks_per_wake: usize,
    pub allow_after_ui_exit: bool,
}

impl Default for BackgroundWorkerPolicy {
    fn default() -> Self {
        Self {
            max_wake_ms: 30_000,
            max_tasks_per_wake: MAX_TASKS_PER_WAKE,
            allow_after_ui_exit: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundWorker {
    pub id: u64,
    pub origin: String,
    pub scope: String,
    pub state: BackgroundWorkerState,
    pub policy: BackgroundWorkerPolicy,
    pub ui_attached: bool,
    pub permission_valid: bool,
    queue: VecDeque<BackgroundTask>,
    queued_payload_bytes: usize,
}

#[derive(Debug, Default)]
pub struct BackgroundWorkerManager {
    next_id: u64,
    workers: BTreeMap<u64, BackgroundWorker>,
}

impl BackgroundWorkerManager {
    pub fn register(
        &mut self,
        origin: impl Into<String>,
        scope: impl Into<String>,
        policy: BackgroundWorkerPolicy,
    ) -> Result<u64, String> {
        let origin = canonical_origin(&origin.into())?;
        let scope = normalized_scope(&scope.into())?;
        if self.workers.len() >= MAX_WORKERS_TOTAL {
            return Err("QuotaExceededError: global background worker budget exceeded".to_string());
        }
        if self
            .workers
            .values()
            .filter(|worker| worker.origin == origin)
            .count()
            >= MAX_WORKERS_PER_ORIGIN
        {
            return Err("QuotaExceededError: background worker budget exceeded".to_string());
        }
        let id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "background worker id overflow".to_string())?;
        self.next_id = id;
        self.workers.insert(
            id,
            BackgroundWorker {
                id,
                origin,
                scope,
                state: BackgroundWorkerState::Registered,
                policy,
                ui_attached: true,
                permission_valid: true,
                queue: VecDeque::new(),
                queued_payload_bytes: 0,
            },
        );
        Ok(id)
    }

    pub fn worker(&self, id: u64) -> Option<&BackgroundWorker> {
        self.workers.get(&id)
    }

    pub fn attach_ui(&mut self, id: u64, attached: bool) -> Result<(), String> {
        let worker = self
            .workers
            .get_mut(&id)
            .ok_or_else(|| "InvalidStateError: worker is not registered".to_string())?;
        worker.ui_attached = attached;
        if !attached && !worker.policy.allow_after_ui_exit {
            worker.state = BackgroundWorkerState::Sleeping;
        }
        Ok(())
    }

    pub fn set_permission(&mut self, id: u64, valid: bool) -> Result<(), String> {
        let worker = self
            .workers
            .get_mut(&id)
            .ok_or_else(|| "InvalidStateError: worker is not registered".to_string())?;
        worker.permission_valid = valid;
        if !valid {
            worker.queue.clear();
            worker.queued_payload_bytes = 0;
            worker.state = BackgroundWorkerState::Revoked;
        }
        Ok(())
    }

    pub fn enqueue(&mut self, id: u64, task: BackgroundTask) -> Result<(), String> {
        if task.payload.len() > MAX_TASK_PAYLOAD_BYTES {
            return Err("QuotaExceededError: background task payload exceeds budget".to_string());
        }
        let worker = self
            .workers
            .get_mut(&id)
            .ok_or_else(|| "InvalidStateError: worker is not registered".to_string())?;
        if !worker.permission_valid || worker.state == BackgroundWorkerState::Revoked {
            return Err("NotAllowedError: background worker permission was revoked".to_string());
        }
        if !worker.ui_attached && !worker.policy.allow_after_ui_exit {
            return Err("InvalidStateError: worker is not allowed after UI exit".to_string());
        }
        if worker.queue.len() >= MAX_QUEUED_TASKS_PER_WORKER {
            return Err("QuotaExceededError: background task queue is full".to_string());
        }
        let projected_bytes = worker
            .queued_payload_bytes
            .checked_add(task.payload.len())
            .ok_or_else(|| {
                "QuotaExceededError: background task byte budget overflow".to_string()
            })?;
        if projected_bytes > MAX_QUEUED_PAYLOAD_BYTES_PER_WORKER {
            return Err("QuotaExceededError: background task byte budget exceeded".to_string());
        }
        worker.queued_payload_bytes = projected_bytes;
        worker.queue.push_back(task);
        Ok(())
    }

    /// Move a bounded batch to the browser-owned dispatcher. The caller must
    /// execute the associated page/service-worker callback and report a
    /// success/failure before waking the worker again.
    pub fn wake(&mut self, id: u64, now_ms: u64) -> Result<Vec<BackgroundTask>, String> {
        let worker = self
            .workers
            .get_mut(&id)
            .ok_or_else(|| "InvalidStateError: worker is not registered".to_string())?;
        if !worker.permission_valid || worker.state == BackgroundWorkerState::Revoked {
            return Err("NotAllowedError: background worker permission was revoked".to_string());
        }
        if !worker.ui_attached && !worker.policy.allow_after_ui_exit {
            return Ok(Vec::new());
        }
        worker.state = BackgroundWorkerState::Running;
        let max_tasks = worker.policy.max_tasks_per_wake.min(MAX_TASKS_PER_WAKE);
        let deadline = now_ms.saturating_add(worker.policy.max_wake_ms);
        let mut tasks = Vec::new();
        while tasks.len() < max_tasks {
            let Some(task) = worker.queue.front() else {
                break;
            };
            if task.created_at_ms > deadline {
                break;
            }
            if let Some(task) = worker.queue.pop_front() {
                worker.queued_payload_bytes = worker
                    .queued_payload_bytes
                    .saturating_sub(task.payload.len());
                tasks.push(task);
            }
        }
        worker.state = if worker.queue.is_empty() {
            BackgroundWorkerState::Sleeping
        } else {
            BackgroundWorkerState::Registered
        };
        Ok(tasks)
    }

    pub fn unregister(&mut self, id: u64) -> bool {
        self.workers.remove(&id).is_some()
    }

    pub fn clear_origin(&mut self, origin: &str) -> Result<usize, String> {
        let origin = canonical_origin(origin)?;
        let ids: Vec<u64> = self
            .workers
            .iter()
            .filter_map(|(id, worker)| (worker.origin == origin).then_some(*id))
            .collect();
        for id in &ids {
            self.workers.remove(id);
        }
        Ok(ids.len())
    }
}

fn canonical_origin(value: &str) -> Result<String, String> {
    let url =
        url::Url::parse(value).map_err(|_| "SecurityError: invalid worker origin".to_string())?;
    if url.scheme() != "https" && url.host_str() != Some("localhost") {
        return Err("SecurityError: workers require a secure origin".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

fn normalized_scope(value: &str) -> Result<String, String> {
    if !value.starts_with('/') || value.len() > 4_096 || value.contains("..") {
        return Err("SecurityError: invalid worker scope".to_string());
    }
    Ok(value.to_string())
}

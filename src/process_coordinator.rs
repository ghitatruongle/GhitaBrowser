//! Browser-owned Phase 23 native process control plane.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::child_process::{ChildProcessManager, ProcessState};
use crate::crash_recovery::CrashRecoveryEngine;
use crate::ipc::IpcCommand;
use crate::process_architecture::{ProcessId, ProcessRole, TabId};

/// Owns the long-lived restricted service processes and one renderer control
/// process per origin. The existing one-shot document worker remains the data
/// plane for bounded parse/layout output; this coordinator supplies persistent
/// versioned IPC, watchdog and crash recovery to the real desktop product.
pub struct BrowserProcessCoordinator {
    program: PathBuf,
    pub manager: ChildProcessManager,
    pub recovery: CrashRecoveryEngine,
    service_processes: HashMap<ProcessRole, ProcessId>,
    renderers_by_origin: HashMap<String, ProcessId>,
}

impl BrowserProcessCoordinator {
    pub fn discover() -> Result<Self, String> {
        let current = std::env::current_exe()
            .map_err(|error| format!("Cannot locate browser executable: {error}"))?;
        let program = current.with_file_name(format!(
            "ghita-browser-child{}",
            std::env::consts::EXE_SUFFIX
        ));
        Self::start(program)
    }

    pub fn start(program: impl AsRef<Path>) -> Result<Self, String> {
        let program = program.as_ref().to_path_buf();
        if !program.is_file() {
            return Err(format!(
                "Native browser child is unavailable at {}",
                program.display()
            ));
        }
        let mut coordinator = Self {
            program,
            manager: ChildProcessManager::new(),
            recovery: CrashRecoveryEngine::new(),
            service_processes: HashMap::new(),
            renderers_by_origin: HashMap::new(),
        };
        for role in [ProcessRole::Network, ProcessRole::Media, ProcessRole::Gpu] {
            let process = coordinator
                .manager
                .spawn_native_process(role.clone(), &coordinator.program)?;
            let reply = coordinator
                .manager
                .send_native_command(process, IpcCommand::Heartbeat)?;
            if !reply.accepted {
                return Err(format!("{role} child rejected its startup heartbeat"));
            }
            coordinator.service_processes.insert(role, process);
        }
        Ok(coordinator)
    }

    pub fn attach_tab(&mut self, tab_id: usize, url: &str) -> Result<ProcessId, String> {
        let origin = site_origin(url)?;
        let process = match self.renderers_by_origin.get(&origin).copied() {
            Some(process)
                if self
                    .manager
                    .processes
                    .get(&process)
                    .is_some_and(|child| child.state == ProcessState::Running) =>
            {
                process
            }
            _ => {
                let process = self.manager.spawn_native_process(
                    ProcessRole::Renderer {
                        origin: origin.clone(),
                    },
                    &self.program,
                )?;
                self.renderers_by_origin.insert(origin.clone(), process);
                process
            }
        };
        self.recovery
            .register_tab(TabId(tab_id as u64), url.to_string(), process);
        let reply = self.manager.send_native_command(
            process,
            IpcCommand::Navigate {
                url: url.to_string(),
            },
        )?;
        if !reply.accepted {
            return Err("Renderer process rejected navigation".to_string());
        }
        Ok(process)
    }

    pub fn heartbeat_and_recover(&mut self) -> Vec<String> {
        let process_ids: Vec<ProcessId> = self.manager.processes.keys().copied().collect();
        let mut failures = Vec::new();
        for process in process_ids {
            if self
                .manager
                .processes
                .get(&process)
                .is_some_and(|child| child.state != ProcessState::Running)
            {
                continue;
            }
            if let Err(error) = self
                .manager
                .send_native_command(process, IpcCommand::Heartbeat)
            {
                failures.push(format!("{process:?}: {error}"));
                self.recovery.inject_fault_crash(&mut self.manager, process);
                if let Ok(replacement) = self.recovery.recover_process(&mut self.manager, process) {
                    for renderer in self.renderers_by_origin.values_mut() {
                        if *renderer == process {
                            *renderer = replacement;
                        }
                    }
                    for service in self.service_processes.values_mut() {
                        if *service == process {
                            *service = replacement;
                        }
                    }
                }
            }
        }
        failures
    }

    pub fn native_process_count(&self) -> usize {
        self.manager
            .processes
            .keys()
            .filter(|process| self.manager.native_os_id(**process).is_some())
            .count()
    }
}

fn site_origin(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|_| "Cannot isolate an invalid URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("Only HTTP(S) pages receive renderer processes".to_string());
    }
    Ok(parsed.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_origin_is_partitioned() {
        assert_eq!(
            site_origin("https://example.test:8443/a").unwrap(),
            "https://example.test:8443"
        );
        assert!(site_origin("file:///tmp/a.html").is_err());
    }
}

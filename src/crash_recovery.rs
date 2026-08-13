//! Fault Injection Framework and Deterministic Child Crash Recovery for GhitaBrowser (Phase 23).
//! Recovers crashed child processes (Renderer, Network, Media, GPU) with generation increment and tab state preservation.

use crate::child_process::{ChildProcessManager, ProcessState};
use crate::process_architecture::{ProcessId, TabId};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct TabState {
    pub tab_id: TabId,
    pub url: String,
    pub title: String,
    pub process_id: ProcessId,
}

pub struct CrashRecoveryEngine {
    pub tabs: HashMap<TabId, TabState>,
    pub recovery_count: usize,
}

impl CrashRecoveryEngine {
    pub fn new() -> Self {
        Self {
            tabs: HashMap::new(),
            recovery_count: 0,
        }
    }

    pub fn register_tab(&mut self, tab_id: TabId, url: impl Into<String>, process_id: ProcessId) {
        let url = url.into();
        self.tabs.insert(
            tab_id,
            TabState {
                tab_id,
                url: url.clone(),
                title: format!("Tab - {url}"),
                process_id,
            },
        );
    }

    pub fn inject_fault_crash(&mut self, cpm: &mut ChildProcessManager, pid: ProcessId) -> bool {
        cpm.kill_native_process(pid);
        if let Some(child) = cpm.processes.get_mut(&pid) {
            child.state = ProcessState::Crashed;
            child.ipc.close();
            child.sandbox.terminate();
            true
        } else {
            false
        }
    }

    pub fn recover_process(
        &mut self,
        cpm: &mut ChildProcessManager,
        crashed_pid: ProcessId,
    ) -> Result<ProcessId, String> {
        let (role, native_program) = {
            let child = cpm
                .processes
                .get(&crashed_pid)
                .ok_or_else(|| format!("Crashed process {crashed_pid:?} not found"))?;
            if child.state != ProcessState::Crashed {
                return Err("Process is not in Crashed state".to_string());
            }
            (child.metadata.role.clone(), cpm.native_program(crashed_pid))
        };

        // Terminate old crashed entry
        cpm.terminate_process(crashed_pid);

        // Spawn replacement process with fresh generation
        let new_pid = if let Some(program) = native_program {
            cpm.spawn_native_process(role, &program)?
        } else {
            cpm.spawn_process(role)
        };

        // Update affected tabs to point to the new process ID while preserving tab state
        for tab in self.tabs.values_mut() {
            if tab.process_id == crashed_pid {
                tab.process_id = new_pid;
            }
        }

        self.recovery_count += 1;
        Ok(new_pid)
    }
}

impl Default for CrashRecoveryEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_architecture::ProcessRole;

    #[test]
    fn fault_injection_and_seamless_tab_crash_recovery() {
        let mut cpm = ChildProcessManager::new();
        let mut recovery = CrashRecoveryEngine::new();

        let renderer_1 = cpm.spawn_process(ProcessRole::Renderer {
            origin: "https://site-a.com".to_string(),
        });
        let renderer_2 = cpm.spawn_process(ProcessRole::Renderer {
            origin: "https://site-b.com".to_string(),
        });

        recovery.register_tab(TabId(1), "https://site-a.com/page1", renderer_1);
        recovery.register_tab(TabId(2), "https://site-b.com/page2", renderer_2);

        // Inject crash into renderer_1 (site A)
        assert!(recovery.inject_fault_crash(&mut cpm, renderer_1));
        assert_eq!(
            cpm.processes.get(&renderer_1).unwrap().state,
            ProcessState::Crashed
        );

        // Renderer 2 (site B) remains unaffected and alive
        assert_eq!(
            cpm.processes.get(&renderer_2).unwrap().state,
            ProcessState::Running
        );

        // Recover renderer 1
        let new_renderer_1 = recovery.recover_process(&mut cpm, renderer_1).unwrap();
        assert_ne!(new_renderer_1, renderer_1);

        // Tab 1 state is preserved and reassigned to new process
        let tab1 = recovery.tabs.get(&TabId(1)).unwrap();
        assert_eq!(tab1.url, "https://site-a.com/page1");
        assert_eq!(tab1.process_id, new_renderer_1);

        // Unrelated Tab 2 process remains renderer_2
        let tab2 = recovery.tabs.get(&TabId(2)).unwrap();
        assert_eq!(tab2.process_id, renderer_2);

        assert_eq!(recovery.recovery_count, 1);
    }
}

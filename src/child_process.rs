//! Child Process Manager and Watchdog for GhitaBrowser (Phase 23)
//! Manages child process spawning, IPC channel wiring, heartbeat monitoring, and termination.

use crate::ipc::{IpcChannel, IpcCommand};
use crate::process_architecture::{GenerationId, ProcessId, ProcessMetadata, ProcessRole};
use crate::sandbox::{JobObjectSandbox, SandboxPolicy};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long a native round-trip may take before the child is declared
/// wedged and killed. Without this a hung child froze the browser forever
/// inside an unbounded blocking read.
const NATIVE_REPLY_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Spawning,
    Running,
    Unresponsive,
    Crashed,
    Terminated,
}

pub struct ChildProcess {
    pub metadata: ProcessMetadata,
    pub ipc: IpcChannel,
    pub sandbox: JobObjectSandbox,
    pub last_heartbeat_ms: u64,
    pub state: ProcessState,
    native_program: Option<PathBuf>,
}

struct NativeChildProcess {
    child: Child,
    stdin: ChildStdin,
    /// Reply lines pushed by the dedicated reader thread so the caller can
    /// wait with a deadline instead of blocking forever.
    replies: mpsc::Receiver<String>,
}

#[derive(Default)]
pub struct ChildProcessManager {
    pub processes: HashMap<ProcessId, ChildProcess>,
    next_pid: u64,
    next_job_id: u64,
    next_generation: u64,
    now_ms: u64,
    native_processes: HashMap<ProcessId, NativeChildProcess>,
}

impl ChildProcessManager {
    pub fn new() -> Self {
        Self {
            processes: HashMap::new(),
            next_pid: 1,
            next_job_id: 100,
            next_generation: 1,
            now_ms: 0,
            native_processes: HashMap::new(),
        }
    }

    /// Spawn a real restricted child endpoint for the requested browser role.
    pub fn spawn_native_process(
        &mut self,
        role: ProcessRole,
        program: &Path,
    ) -> Result<ProcessId, String> {
        let pid = self.spawn_process(role.clone());
        let generation = self
            .processes
            .get(&pid)
            .expect("logical child exists")
            .metadata
            .generation;
        let encoded_role = serde_json::to_string(&role).map_err(|error| error.to_string())?;
        let mut command = Command::new(program);
        command
            .arg(pid.0.to_string())
            .arg(generation.0.to_string())
            .arg(encoded_role)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW | CREATE_SUSPENDED: the child starts paused so
            // it can be placed inside its job object before executing a single
            // instruction — anything it spawned pre-assignment would escape
            // the job's limits and kill-on-close.
            command.creation_flags(0x0800_0000 | 0x0000_0004);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("Cannot spawn browser child: {error}"))?;
        let containment = self
            .processes
            .get_mut(&pid)
            .expect("logical child exists")
            .sandbox
            .assign_native_process(&child);
        if let Err(error) = containment {
            let _ = child.kill();
            let _ = child.wait();
            self.processes.remove(&pid);
            return Err(error);
        }
        #[cfg(windows)]
        if let Err(error) = resume_child_main_thread(&child) {
            let _ = child.kill();
            let _ = child.wait();
            self.processes.remove(&pid);
            return Err(error);
        }
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Browser child stdin is unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Browser child stdout is unavailable".to_string())?;
        let replies = spawn_reply_reader(stdout);
        self.native_processes.insert(pid, NativeChildProcess { child, stdin, replies });
        self.processes
            .get_mut(&pid)
            .expect("logical child exists")
            .native_program = Some(program.to_path_buf());
        Ok(pid)
    }

    pub fn send_native_command(
        &mut self,
        pid: ProcessId,
        command: IpcCommand,
    ) -> Result<crate::ipc::NativeIpcReply, String> {
        let child = self
            .processes
            .get_mut(&pid)
            .ok_or_else(|| format!("Process {pid:?} not found"))?;
        child.ipc.send(command)?;
        let message = child
            .ipc
            .send_queue
            .pop()
            .ok_or_else(|| "IPC send queue unexpectedly empty".to_string())?;
        let native = self
            .native_processes
            .get_mut(&pid)
            .ok_or_else(|| "Process has no native child endpoint".to_string())?;
        let encoded = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        if encoded.len() > crate::ipc::MAX_IPC_MESSAGE_BYTES {
            return Err("IPC message exceeds its byte budget".to_string());
        }
        native
            .stdin
            .write_all(&encoded)
            .and_then(|_| native.stdin.write_all(b"\n"))
            .and_then(|_| native.stdin.flush())
            .map_err(|error| format!("Cannot write child IPC: {error}"))?;
        // The reader thread owns stdout, so a hung child surfaces here as a
        // timeout instead of an infinite block on the calling (UI) thread.
        let line = match native.replies.recv_timeout(NATIVE_REPLY_TIMEOUT) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let child = self.processes.get_mut(&pid);
                if let Some(child) = child {
                    child.state = ProcessState::Unresponsive;
                }
                self.kill_native_process(pid);
                return Err("native IPC reply timed out; child was killed".to_string());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.kill_native_process(pid);
                return Err("child IPC stream closed before reply".to_string());
            }
        };
        if line.len() > crate::ipc::MAX_IPC_MESSAGE_BYTES {
            return Err("IPC response exceeds its byte budget".to_string());
        }
        let reply: crate::ipc::NativeIpcReply =
            serde_json::from_str(&line).map_err(|error| format!("Invalid child IPC: {error}"))?;
        if reply.version != crate::ipc::IPC_VERSION
            || reply.process_id != pid
            || reply.generation != child.metadata.generation
            || reply.sequence != message.sequence
        {
            return Err("Stale or mismatched native IPC reply".to_string());
        }
        if reply.accepted {
            child.last_heartbeat_ms = self.now_ms;
        }
        Ok(reply)
    }

    pub fn native_program(&self, pid: ProcessId) -> Option<PathBuf> {
        self.processes
            .get(&pid)
            .and_then(|process| process.native_program.clone())
    }

    pub fn kill_native_process(&mut self, pid: ProcessId) -> bool {
        let Some(mut native) = self.native_processes.remove(&pid) else {
            return false;
        };
        let _ = native.child.kill();
        let _ = native.child.wait();
        true
    }

    pub fn native_os_id(&self, pid: ProcessId) -> Option<u32> {
        self.native_processes
            .get(&pid)
            .map(|process| process.child.id())
    }

    pub fn spawn_process(&mut self, role: ProcessRole) -> ProcessId {
        let pid = ProcessId(self.next_pid);
        self.next_pid += 1;

        let job_id = self.next_job_id;
        self.next_job_id += 1;

        let gen = GenerationId(self.next_generation);
        self.next_generation = self.next_generation.saturating_add(1);
        let meta = ProcessMetadata::new(pid, role.clone(), gen);
        let policy = SandboxPolicy::default_for_role(&role);
        let mut sandbox = JobObjectSandbox::new(job_id, policy);
        sandbox.assign_process(pid).expect("assign pid");

        let ipc = IpcChannel::new(pid.0, ProcessId(0), gen, pid, gen);

        let child = ChildProcess {
            metadata: meta,
            ipc,
            sandbox,
            last_heartbeat_ms: self.now_ms,
            state: ProcessState::Running,
            native_program: None,
        };

        self.processes.insert(pid, child);
        pid
    }

    pub fn send_command(&mut self, pid: ProcessId, command: IpcCommand) -> Result<(), String> {
        let child = self
            .processes
            .get_mut(&pid)
            .ok_or_else(|| format!("Process {pid:?} not found"))?;

        if child.state == ProcessState::Crashed || child.state == ProcessState::Terminated {
            return Err("Cannot send command to crashed/terminated process".to_string());
        }

        child.ipc.send(command)
    }

    pub fn record_heartbeat(&mut self, pid: ProcessId, now_ms: u64) {
        if let Some(child) = self.processes.get_mut(&pid) {
            child.last_heartbeat_ms = now_ms;
            if child.state == ProcessState::Unresponsive {
                child.state = ProcessState::Running;
            }
        }
    }

    pub fn advance_time(&mut self, elapsed_ms: u64) {
        self.now_ms = self.now_ms.saturating_add(elapsed_ms);
    }

    pub fn check_watchdog(&mut self, timeout_ms: u64) -> Vec<ProcessId> {
        let mut unresponsive = Vec::new();
        let exited: Vec<ProcessId> = self
            .native_processes
            .iter_mut()
            .filter_map(|(pid, native)| match native.child.try_wait() {
                Ok(Some(_)) | Err(_) => Some(*pid),
                Ok(None) => None,
            })
            .collect();
        for pid in exited {
            self.native_processes.remove(&pid);
            if let Some(child) = self.processes.get_mut(&pid) {
                child.state = ProcessState::Crashed;
                child.ipc.close();
                child.sandbox.terminate();
            }
            unresponsive.push(pid);
        }
        for (pid, child) in &mut self.processes {
            if child.state == ProcessState::Running
                && self.now_ms.saturating_sub(child.last_heartbeat_ms) > timeout_ms
            {
                child.state = ProcessState::Unresponsive;
                if !unresponsive.contains(pid) {
                    unresponsive.push(*pid);
                }
            }
        }
        unresponsive
    }

    pub fn terminate_process(&mut self, pid: ProcessId) -> bool {
        self.kill_native_process(pid);
        if let Some(mut child) = self.processes.remove(&pid) {
            child.state = ProcessState::Terminated;
            child.ipc.close();
            child.sandbox.terminate();
            true
        } else {
            false
        }
    }
}

/// Background reader that turns child stdout lines into channel messages.
/// The line length is bounded so a hostile child cannot balloon memory even
/// though reading happens off the caller's thread.
fn spawn_reply_reader(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("native-child-ipc-reader".into())
        .spawn(move || {
            use std::io::Read;
            let reader = BufReader::new(stdout);
            let mut inner = reader.take((crate::ipc::MAX_IPC_MESSAGE_BYTES + 1) as u64);
            loop {
                let mut line = String::new();
                match inner.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .expect("spawn native IPC reader thread");
    receiver
}

#[cfg(windows)]
fn resume_child_main_thread(child: &Child) -> Result<(), String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::Threading::{
        OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    let os_pid = child.id();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
            .map_err(|error| format!("cannot snapshot threads: {error}"))?;
        let resume_result = (|| {
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            if Thread32First(snapshot, &mut entry).is_err() {
                return Err("cannot enumerate child threads".to_string());
            }
            let mut resumed = false;
            loop {
                if entry.th32OwnerProcessID == os_pid {
                    if let Ok(thread) =
                        OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                    {
                        // Any non-negative return means the thread is running.
                        let _ = ResumeThread(thread);
                        resumed = true;
                    }
                }
                if Thread32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
            if resumed {
                Ok(())
            } else {
                Err("child main thread was not found to resume".to_string())
            }
        })();
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
        resume_result
    }
}

impl Drop for ChildProcessManager {
    fn drop(&mut self) {
        let process_ids: Vec<ProcessId> = self.processes.keys().copied().collect();
        for process_id in process_ids {
            self.terminate_process(process_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_and_watchdog_unresponsive_detection() {
        let mut cpm = ChildProcessManager::new();
        let pid = cpm.spawn_process(ProcessRole::Network);

        assert_eq!(
            cpm.processes.get(&pid).unwrap().state,
            ProcessState::Running
        );

        // Advance time past 5000 ms timeout without heartbeat
        cpm.advance_time(6000);
        let unresponsive = cpm.check_watchdog(5000);

        assert_eq!(unresponsive, vec![pid]);
        assert_eq!(
            cpm.processes.get(&pid).unwrap().state,
            ProcessState::Unresponsive
        );

        // Record heartbeat recovers process
        cpm.record_heartbeat(pid, 6000);
        assert_eq!(
            cpm.processes.get(&pid).unwrap().state,
            ProcessState::Running
        );
    }
}

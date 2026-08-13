//! Windows Job Object Sandbox and Per-Site Isolation for GhitaBrowser (Phase 23).
//! Implements process memory caps, UI restrictions, and site origin isolation policies.

use crate::process_architecture::{ProcessId, ProcessRole};

#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    pub memory_limit_bytes: usize,
    pub ui_restrictions: bool,
    pub kill_on_job_close: bool,
    pub allowed_origin: Option<String>,
}

impl SandboxPolicy {
    pub fn default_for_role(role: &ProcessRole) -> Self {
        match role {
            ProcessRole::Renderer { origin } => Self {
                memory_limit_bytes: 512 * 1024 * 1024, // 512 MB cap
                ui_restrictions: true,
                kill_on_job_close: true,
                allowed_origin: Some(origin.clone()),
            },
            ProcessRole::Network => Self {
                memory_limit_bytes: 256 * 1024 * 1024,
                ui_restrictions: true,
                kill_on_job_close: true,
                allowed_origin: None,
            },
            ProcessRole::Media => Self {
                memory_limit_bytes: 512 * 1024 * 1024,
                ui_restrictions: true,
                kill_on_job_close: true,
                allowed_origin: None,
            },
            ProcessRole::Gpu => Self {
                memory_limit_bytes: 1024 * 1024 * 1024,
                ui_restrictions: false,
                kill_on_job_close: true,
                allowed_origin: None,
            },
            ProcessRole::Browser => Self {
                memory_limit_bytes: 2048 * 1024 * 1024,
                ui_restrictions: false,
                kill_on_job_close: false,
                allowed_origin: None,
            },
        }
    }
}

pub struct JobObjectSandbox {
    pub job_id: u64,
    pub policy: SandboxPolicy,
    pub active_processes: Vec<ProcessId>,
    pub terminated: bool,
    #[cfg(windows)]
    native_job: Option<windows::Win32::Foundation::HANDLE>,
}

impl JobObjectSandbox {
    pub fn new(job_id: u64, policy: SandboxPolicy) -> Self {
        #[cfg(windows)]
        let native_job = create_native_job(&policy).ok();
        Self {
            job_id,
            policy,
            active_processes: Vec::new(),
            terminated: false,
            #[cfg(windows)]
            native_job,
        }
    }

    pub fn assign_process(&mut self, process_id: ProcessId) -> Result<(), String> {
        if self.terminated {
            return Err("Cannot assign process to terminated sandbox".to_string());
        }
        if !self.active_processes.contains(&process_id) {
            self.active_processes.push(process_id);
        }
        Ok(())
    }

    pub fn check_memory_usage(&self, current_bytes: usize) -> Result<(), String> {
        if current_bytes > self.policy.memory_limit_bytes {
            Err(format!(
                "Sandbox memory limit exceeded: {} > {} bytes",
                current_bytes, self.policy.memory_limit_bytes
            ))
        } else {
            Ok(())
        }
    }

    pub fn validate_site_access(&self, target_origin: &str) -> bool {
        match &self.policy.allowed_origin {
            Some(allowed) => allowed == target_origin,
            None => true, // Non-renderer processes have global network access
        }
    }

    pub fn terminate(&mut self) {
        self.terminated = true;
        self.active_processes.clear();
    }

    /// Attach a real Windows child process to the configured Job Object.
    #[cfg(windows)]
    pub fn assign_native_process(&mut self, child: &std::process::Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;

        let job = self
            .native_job
            .ok_or_else(|| "Native Windows Job Object is unavailable".to_string())?;
        let process = HANDLE(child.as_raw_handle() as isize);
        unsafe { AssignProcessToJobObject(job, process) }
            .map_err(|error| format!("Cannot assign process to Windows Job Object: {error}"))
    }

    #[cfg(not(windows))]
    pub fn assign_native_process(&mut self, _child: &std::process::Child) -> Result<(), String> {
        Err("Native process sandboxing is only available on Windows".to_string())
    }

    #[cfg(windows)]
    pub fn has_native_job(&self) -> bool {
        self.native_job.is_some()
    }

    #[cfg(not(windows))]
    pub fn has_native_job(&self) -> bool {
        false
    }
}

#[cfg(windows)]
fn create_native_job(policy: &SandboxPolicy) -> Result<windows::Win32::Foundation::HANDLE, String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_BASIC_UI_RESTRICTIONS,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS,
        JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
        JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
        JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    };

    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
        .map_err(|error| format!("Cannot create Windows Job Object: {error}"))?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    if policy.kill_on_job_close {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    }
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = policy.memory_limit_bytes;
    if let Err(error) = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(job);
        }
        return Err(format!("Cannot configure Windows Job Object: {error}"));
    }
    if policy.ui_restrictions {
        let restrictions = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES
                | JOB_OBJECT_UILIMIT_READCLIPBOARD
                | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
                | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                | JOB_OBJECT_UILIMIT_GLOBALATOMS
                | JOB_OBJECT_UILIMIT_DESKTOP
                | JOB_OBJECT_UILIMIT_EXITWINDOWS,
        };
        if let Err(error) = unsafe {
            SetInformationJobObject(
                job,
                JobObjectBasicUIRestrictions,
                std::ptr::addr_of!(restrictions).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
            )
        } {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(job);
            }
            return Err(format!(
                "Cannot configure Job Object UI restrictions: {error}"
            ));
        }
    }
    Ok(job)
}

#[cfg(windows)]
impl Drop for JobObjectSandbox {
    fn drop(&mut self) {
        if let Some(job) = self.native_job.take() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(job);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_memory_limits_and_site_isolation() {
        let policy = SandboxPolicy::default_for_role(&ProcessRole::Renderer {
            origin: "https://example.com".to_string(),
        });

        let mut sandbox = JobObjectSandbox::new(101, policy);
        sandbox.assign_process(ProcessId(1)).unwrap();

        // Memory check passes within limit
        assert!(sandbox.check_memory_usage(100 * 1024 * 1024).is_ok());

        // Memory check fails exceeding 512 MB
        assert!(sandbox.check_memory_usage(600 * 1024 * 1024).is_err());

        // Same origin access is allowed
        assert!(sandbox.validate_site_access("https://example.com"));
        // Cross origin access is denied for site-isolated renderer
        assert!(!sandbox.validate_site_access("https://evil.com"));
    }
}

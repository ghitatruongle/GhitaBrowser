//! Integration tests for Phase 23 — Job Object Sandbox and Site Isolation.

use ghitabrowser::process_architecture::{ProcessId, ProcessRole};
use ghitabrowser::sandbox::{JobObjectSandbox, SandboxPolicy};

#[test]
fn sandbox_per_role_limits_and_memory_caps() {
    let renderer_policy = SandboxPolicy::default_for_role(&ProcessRole::Renderer {
        origin: "https://secure.site.com".to_string(),
    });
    assert_eq!(renderer_policy.memory_limit_bytes, 512 * 1024 * 1024);
    assert!(renderer_policy.ui_restrictions);

    let network_policy = SandboxPolicy::default_for_role(&ProcessRole::Network);
    assert_eq!(network_policy.memory_limit_bytes, 256 * 1024 * 1024);

    let gpu_policy = SandboxPolicy::default_for_role(&ProcessRole::Gpu);
    assert_eq!(gpu_policy.memory_limit_bytes, 1024 * 1024 * 1024);

    let mut sandbox = JobObjectSandbox::new(500, renderer_policy);
    sandbox.assign_process(ProcessId(10)).expect("assign");

    assert!(sandbox.check_memory_usage(200 * 1024 * 1024).is_ok());
    assert!(sandbox.check_memory_usage(600 * 1024 * 1024).is_err());
}

#[test]
fn site_origin_isolation_policy() {
    let policy = SandboxPolicy::default_for_role(&ProcessRole::Renderer {
        origin: "https://bank.example.com".to_string(),
    });

    let sandbox = JobObjectSandbox::new(501, policy);

    // Allowed origin
    assert!(sandbox.validate_site_access("https://bank.example.com"));

    // Cross-origin sites fail closed
    assert!(!sandbox.validate_site_access("https://phishing.example.org"));
}

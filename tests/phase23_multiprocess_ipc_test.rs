//! Integration tests for Phase 23 — Multi-process versioned IPC and message framing.

use ghitabrowser::ipc::{IpcChannel, IpcCommand, IpcMessage, IPC_VERSION};
use ghitabrowser::process_architecture::{GenerationId, ProcessId};

#[cfg(windows)]
use ghitabrowser::child_process::ChildProcessManager;
#[cfg(windows)]
use ghitabrowser::process_architecture::ProcessRole;
#[cfg(windows)]
use ghitabrowser::process_coordinator::BrowserProcessCoordinator;

#[test]
fn ipc_versioning_command_delivery_and_sequence_tracking() {
    let mut channel = IpcChannel::new(
        100,
        ProcessId(1),
        GenerationId(1),
        ProcessId(2),
        GenerationId(1),
    );

    // Send commands
    channel
        .send(IpcCommand::Navigate {
            url: "https://example.com/home".to_string(),
        })
        .expect("send navigate");

    channel
        .send(IpcCommand::RenderFrame {
            html: "<h1>Title</h1>".to_string(),
        })
        .expect("send render");

    assert_eq!(channel.send_queue.len(), 2);
    assert_eq!(channel.sequence, 3);

    // Deliver incoming
    let msg1 = channel.send_queue.remove(0);
    channel.deliver_incoming(msg1).expect("deliver msg1");

    let received = channel.receive().expect("receive msg1");
    assert_eq!(received.version, IPC_VERSION);
    assert_eq!(
        received.command,
        IpcCommand::Navigate {
            url: "https://example.com/home".to_string()
        }
    );
}

#[test]
fn ipc_stale_generation_and_version_mismatch_rejection() {
    let mut channel = IpcChannel::new(
        101,
        ProcessId(1),
        GenerationId(2), // Current target generation is 2
        ProcessId(2),
        GenerationId(2),
    );

    // Stale generation 1 message is rejected
    let stale_msg = IpcMessage {
        version: IPC_VERSION,
        sender_id: ProcessId(1),
        sender_generation: GenerationId(1),
        target_id: ProcessId(2),
        target_generation: GenerationId(1), // Stale!
        sequence: 1,
        command: IpcCommand::Heartbeat,
    };

    let err = channel.deliver_incoming(stale_msg).expect_err("stale err");
    assert_eq!(err, "StaleGenerationMessageDropped");

    // Version mismatch is rejected
    let bad_version_msg = IpcMessage {
        version: 999, // Invalid version!
        sender_id: ProcessId(1),
        sender_generation: GenerationId(2),
        target_id: ProcessId(2),
        target_generation: GenerationId(2),
        sequence: 2,
        command: IpcCommand::Heartbeat,
    };

    let err_v = channel
        .deliver_incoming(bad_version_msg)
        .expect_err("version err");
    assert!(err_v.contains("IPC version mismatch"));
}

#[cfg(windows)]
#[test]
fn real_child_process_uses_versioned_ipc_and_windows_job_object() {
    let program = std::path::Path::new(env!("CARGO_BIN_EXE_ghita-browser-child"));
    let mut manager = ChildProcessManager::new();
    let process = manager
        .spawn_native_process(ProcessRole::Network, program)
        .expect("spawn real network child");
    assert!(manager.native_os_id(process).is_some());
    assert!(manager
        .processes
        .get(&process)
        .expect("child metadata")
        .sandbox
        .has_native_job());
    let reply = manager
        .send_native_command(process, IpcCommand::Heartbeat)
        .expect("real IPC heartbeat");
    assert!(reply.accepted);
    assert_eq!(reply.process_id, process);
    assert!(manager.terminate_process(process));
    assert!(manager.native_os_id(process).is_none());
}

#[cfg(windows)]
#[test]
fn desktop_coordinator_starts_service_roles_and_origin_renderer() {
    let program = std::path::Path::new(env!("CARGO_BIN_EXE_ghita-browser-child"));
    let mut coordinator = BrowserProcessCoordinator::start(program).expect("start coordinator");
    assert_eq!(coordinator.native_process_count(), 3);
    let first = coordinator
        .attach_tab(11, "https://site-a.test/page")
        .expect("attach first tab");
    let second = coordinator
        .attach_tab(12, "https://site-a.test/other")
        .expect("reuse same-origin renderer");
    assert_eq!(first, second);
    let third = coordinator
        .attach_tab(13, "https://site-b.test/")
        .expect("separate cross-origin renderer");
    assert_ne!(first, third);
    assert_eq!(coordinator.native_process_count(), 5);
    assert!(coordinator.heartbeat_and_recover().is_empty());
}

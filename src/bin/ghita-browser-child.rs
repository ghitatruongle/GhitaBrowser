//! Generic restricted child-process endpoint for Phase 23 browser roles.

use std::io::{BufRead, Write};

use ghitabrowser::ipc::{IpcMessage, NativeIpcReply, IPC_VERSION, MAX_IPC_MESSAGE_BYTES};
use ghitabrowser::process_architecture::{GenerationId, ProcessId, ProcessRole};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    ghitabrowser::worker::apply_restricted_worker_token().map_err(|error| error.to_string())?;
    let mut arguments = std::env::args().skip(1);
    let process_id = arguments
        .next()
        .ok_or_else(|| "missing process id".to_string())?
        .parse::<u64>()
        .map(ProcessId)
        .map_err(|_| "invalid process id".to_string())?;
    let generation = arguments
        .next()
        .ok_or_else(|| "missing generation".to_string())?
        .parse::<u64>()
        .map(GenerationId)
        .map_err(|_| "invalid generation".to_string())?;
    let role: ProcessRole = serde_json::from_str(
        &arguments
            .next()
            .ok_or_else(|| "missing process role".to_string())?,
    )
    .map_err(|error| format!("invalid process role: {error}"))?;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.len() > MAX_IPC_MESSAGE_BYTES {
            return Err("IPC request exceeds its byte budget".to_string());
        }
        let message: IpcMessage =
            serde_json::from_str(&line).map_err(|error| format!("invalid IPC request: {error}"))?;
        let accepted = message.version == IPC_VERSION
            && message.target_id == process_id
            && message.target_generation == generation;
        let detail = if accepted {
            match &message.command {
                ghitabrowser::ipc::IpcCommand::Shutdown => "shutdown".to_string(),
                command => format!("{role}: accepted {command:?}"),
            }
        } else {
            "rejected version, process id or generation".to_string()
        };
        let should_shutdown = detail == "shutdown";
        let reply = NativeIpcReply {
            version: IPC_VERSION,
            process_id,
            generation,
            sequence: message.sequence,
            accepted,
            detail,
        };
        let encoded = serde_json::to_vec(&reply).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_IPC_MESSAGE_BYTES {
            return Err("IPC response exceeds its byte budget".to_string());
        }
        stdout
            .write_all(&encoded)
            .and_then(|_| stdout.write_all(b"\n"))
            .and_then(|_| stdout.flush())
            .map_err(|error| error.to_string())?;
        if should_shutdown {
            break;
        }
    }
    Ok(())
}

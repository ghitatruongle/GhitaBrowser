//! Versioned Bounded IPC Channels and Message Framing for GhitaBrowser (Phase 23).
//! Implements strict generation-checked message passing between browser and child processes.

use crate::process_architecture::{GenerationId, ProcessId};

pub const IPC_VERSION: u32 = 1;
pub const MAX_IPC_MESSAGE_BYTES: usize = 4 * 1024 * 1024; // 4 MB max message size

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IpcCommand {
    Navigate { url: String },
    RenderFrame { html: String },
    FetchResource { url: String, method: String },
    PlayMedia { src: String },
    Heartbeat,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IpcMessage {
    pub version: u32,
    pub sender_id: ProcessId,
    pub sender_generation: GenerationId,
    pub target_id: ProcessId,
    pub target_generation: GenerationId,
    pub sequence: u64,
    pub command: IpcCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NativeIpcReply {
    pub version: u32,
    pub process_id: ProcessId,
    pub generation: GenerationId,
    pub sequence: u64,
    pub accepted: bool,
    pub detail: String,
}

pub struct IpcChannel {
    pub channel_id: u64,
    pub sender_id: ProcessId,
    pub sender_generation: GenerationId,
    pub target_id: ProcessId,
    pub target_generation: GenerationId,
    pub sequence: u64,
    pub send_queue: Vec<IpcMessage>,
    pub receive_queue: Vec<IpcMessage>,
    pub closed: bool,
    last_received_sequence: u64,
}

impl IpcChannel {
    pub fn new(
        channel_id: u64,
        sender_id: ProcessId,
        sender_generation: GenerationId,
        target_id: ProcessId,
        target_generation: GenerationId,
    ) -> Self {
        Self {
            channel_id,
            sender_id,
            sender_generation,
            target_id,
            target_generation,
            sequence: 1,
            send_queue: Vec::new(),
            receive_queue: Vec::new(),
            closed: false,
            last_received_sequence: 0,
        }
    }

    pub fn send(&mut self, command: IpcCommand) -> Result<(), String> {
        if self.closed {
            return Err("IPC channel is closed".to_string());
        }

        let seq = self.sequence;
        self.sequence += 1;

        let message = IpcMessage {
            version: IPC_VERSION,
            sender_id: self.sender_id,
            sender_generation: self.sender_generation,
            target_id: self.target_id,
            target_generation: self.target_generation,
            sequence: seq,
            command,
        };

        let encoded = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_IPC_MESSAGE_BYTES {
            return Err("IPC message exceeds its byte budget".to_string());
        }
        if self.send_queue.len() >= 1000 {
            return Err("IPC send queue budget exceeded".to_string());
        }

        self.send_queue.push(message);
        Ok(())
    }

    pub fn deliver_incoming(&mut self, message: IpcMessage) -> Result<(), String> {
        if self.closed {
            return Err("IPC channel is closed".to_string());
        }

        // Validate version
        if message.version != IPC_VERSION {
            return Err(format!(
                "IPC version mismatch: expected {}, got {}",
                IPC_VERSION, message.version
            ));
        }

        // Validate target process and generation ID (drop stale messages from previous generations)
        if message.target_generation != self.target_generation {
            return Err("StaleGenerationMessageDropped".to_string());
        }
        if message.target_id != self.target_id
            || message.sender_id != self.sender_id
            || message.sender_generation != self.sender_generation
        {
            return Err("IPC endpoint identity mismatch".to_string());
        }
        if message.sequence <= self.last_received_sequence {
            return Err("IPC replay or out-of-order sequence rejected".to_string());
        }

        if self.receive_queue.len() >= 1000 {
            return Err("IPC receive queue budget exceeded".to_string());
        }

        self.last_received_sequence = message.sequence;
        self.receive_queue.push(message);
        Ok(())
    }

    pub fn receive(&mut self) -> Option<IpcMessage> {
        if self.closed || self.receive_queue.is_empty() {
            None
        } else {
            Some(self.receive_queue.remove(0))
        }
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.send_queue.clear();
        self.receive_queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_channel_version_and_generation_checking() {
        let mut ch = IpcChannel::new(
            1,
            ProcessId(1),
            GenerationId(1),
            ProcessId(2),
            GenerationId(1),
        );

        // Valid message sends cleanly
        ch.send(IpcCommand::Navigate {
            url: "https://example.com".to_string(),
        })
        .unwrap();

        let msg = ch.send_queue.remove(0);
        ch.deliver_incoming(msg).unwrap();

        let received = ch.receive().unwrap();
        assert_eq!(
            received.command,
            IpcCommand::Navigate {
                url: "https://example.com".to_string()
            }
        );

        // Message with stale generation is rejected
        let stale_msg = IpcMessage {
            version: IPC_VERSION,
            sender_id: ProcessId(1),
            sender_generation: GenerationId(1),
            target_id: ProcessId(2),
            target_generation: GenerationId(0), // Stale generation!
            sequence: 99,
            command: IpcCommand::Heartbeat,
        };

        let err = ch.deliver_incoming(stale_msg).unwrap_err();
        assert_eq!(err, "StaleGenerationMessageDropped");
    }
}

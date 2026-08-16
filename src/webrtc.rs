//! Bounded local WebRTC state machine.
//!
//! Transport, DTLS-SRTP and device backends are injected by platform code.
//! This browser-owned layer controls origin/permission checks, peer lifecycle,
//! candidate/data-channel limits and prompt teardown on revocation.

use crate::permissions::{PermissionState, PermissionStore, PermissionType};
use std::collections::{BTreeMap, VecDeque};

pub const MAX_PEERS_PER_ORIGIN: usize = 16;
pub const MAX_PEERS_TOTAL: usize = 128;
pub const MAX_ICE_CANDIDATES_PER_PEER: usize = 128;
pub const MAX_DATA_CHANNELS_PER_PEER: usize = 32;
pub const MAX_DATA_CHANNEL_MESSAGE_BYTES: usize = 256 * 1024;
pub const MAX_BUFFERED_DATA_BYTES_PER_PEER: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtcPeerState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtcTrackKind {
    Audio,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcTrack {
    pub id: String,
    pub kind: RtcTrackKind,
    pub enabled: bool,
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcDataChannel {
    pub id: u16,
    pub label: String,
    pub ordered: bool,
    pub open: bool,
    inbound: VecDeque<Vec<u8>>,
    outbound: VecDeque<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct RtcPeerConnection {
    pub id: u64,
    pub origin: String,
    pub state: RtcPeerState,
    pub local_description: Option<String>,
    pub remote_description: Option<String>,
    pub tracks: Vec<RtcTrack>,
    pub ice_candidates: Vec<String>,
    data_channels: BTreeMap<u16, RtcDataChannel>,
    next_data_channel_id: u16,
    buffered_data_bytes: usize,
}

#[derive(Debug, Default)]
pub struct RtcRegistry {
    next_peer_id: u64,
    peers: BTreeMap<u64, RtcPeerConnection>,
}

impl RtcRegistry {
    pub fn create_peer(&mut self, origin: &str) -> Result<u64, String> {
        let origin = canonical_origin(origin)?;
        if self.peers.len() >= MAX_PEERS_TOTAL {
            self.peers
                .retain(|_, peer| peer.state != RtcPeerState::Closed);
        }
        if self.peers.len() >= MAX_PEERS_TOTAL {
            return Err("QuotaExceededError: global WebRTC peer budget exceeded".to_string());
        }
        if self
            .peers
            .values()
            .filter(|peer| peer.origin == origin && peer.state != RtcPeerState::Closed)
            .count()
            >= MAX_PEERS_PER_ORIGIN
        {
            return Err("QuotaExceededError: WebRTC peer budget exceeded".to_string());
        }
        let id = self
            .next_peer_id
            .checked_add(1)
            .ok_or_else(|| "WebRTC peer id overflow".to_string())?;
        self.next_peer_id = id;
        self.peers.insert(
            id,
            RtcPeerConnection {
                id,
                origin,
                state: RtcPeerState::New,
                local_description: None,
                remote_description: None,
                tracks: Vec::new(),
                ice_candidates: Vec::new(),
                data_channels: BTreeMap::new(),
                next_data_channel_id: 0,
                buffered_data_bytes: 0,
            },
        );
        Ok(id)
    }

    pub fn peer(&self, id: u64) -> Option<&RtcPeerConnection> {
        self.peers.get(&id)
    }

    pub fn add_track(
        &mut self,
        peer_id: u64,
        permission_store: &PermissionStore,
        kind: RtcTrackKind,
    ) -> Result<String, String> {
        let peer = self.open_peer_mut(peer_id)?;
        let permission = match kind {
            RtcTrackKind::Audio => PermissionType::Microphone,
            RtcTrackKind::Video => PermissionType::Camera,
        };
        if permission_store.get_permission(&peer.origin, permission) != PermissionState::Granted {
            return Err("NotAllowedError: capture permission was not granted".to_string());
        }
        let index = peer.tracks.len().saturating_add(1);
        let id = format!("ghita-rtc-{}-{index}", peer.id);
        peer.tracks.push(RtcTrack {
            id: id.clone(),
            kind,
            enabled: true,
            stopped: false,
        });
        Ok(id)
    }

    pub fn set_description(&mut self, peer_id: u64, local: bool, sdp: &str) -> Result<(), String> {
        if sdp.is_empty() || sdp.len() > 128 * 1024 || !sdp.is_ascii() {
            return Err("TypeError: SDP exceeds the bounded WebRTC profile".to_string());
        }
        let peer = self.open_peer_mut(peer_id)?;
        if local {
            peer.local_description = Some(sdp.to_string());
        } else {
            peer.remote_description = Some(sdp.to_string());
        }
        if peer.local_description.is_some() && peer.remote_description.is_some() {
            peer.state = RtcPeerState::Connecting;
        }
        Ok(())
    }

    pub fn add_ice_candidate(&mut self, peer_id: u64, candidate: &str) -> Result<(), String> {
        if candidate.is_empty() || candidate.len() > 8 * 1024 {
            return Err("TypeError: invalid ICE candidate".to_string());
        }
        let peer = self.open_peer_mut(peer_id)?;
        if peer.ice_candidates.len() >= MAX_ICE_CANDIDATES_PER_PEER {
            return Err("QuotaExceededError: ICE candidate budget exceeded".to_string());
        }
        peer.ice_candidates.push(candidate.to_string());
        if peer.local_description.is_some() && peer.remote_description.is_some() {
            peer.state = RtcPeerState::Connected;
        }
        Ok(())
    }

    pub fn create_data_channel(
        &mut self,
        peer_id: u64,
        label: &str,
        ordered: bool,
    ) -> Result<u16, String> {
        if label.len() > 256 {
            return Err("TypeError: data-channel label exceeds budget".to_string());
        }
        let peer = self.open_peer_mut(peer_id)?;
        if peer.data_channels.len() >= MAX_DATA_CHANNELS_PER_PEER {
            return Err("QuotaExceededError: data-channel budget exceeded".to_string());
        }
        let id = peer.next_data_channel_id;
        peer.next_data_channel_id = peer.next_data_channel_id.saturating_add(1);
        peer.data_channels.insert(
            id,
            RtcDataChannel {
                id,
                label: label.to_string(),
                ordered,
                open: true,
                inbound: VecDeque::new(),
                outbound: VecDeque::new(),
            },
        );
        Ok(id)
    }

    pub fn send_data(
        &mut self,
        peer_id: u64,
        channel_id: u16,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        if bytes.len() > MAX_DATA_CHANNEL_MESSAGE_BYTES {
            return Err("QuotaExceededError: data-channel message exceeds budget".to_string());
        }
        let peer = self.open_peer_mut(peer_id)?;
        let projected_bytes = peer
            .buffered_data_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| "QuotaExceededError: data-channel byte budget overflow".to_string())?;
        if projected_bytes > MAX_BUFFERED_DATA_BYTES_PER_PEER {
            return Err("QuotaExceededError: data-channel byte budget exceeded".to_string());
        }
        let channel = peer
            .data_channels
            .get_mut(&channel_id)
            .ok_or_else(|| "InvalidStateError: data channel is detached".to_string())?;
        if !channel.open {
            return Err("InvalidStateError: data channel is closed".to_string());
        }
        channel.outbound.push_back(bytes);
        peer.buffered_data_bytes = projected_bytes;
        Ok(())
    }

    pub fn receive_data(
        &mut self,
        peer_id: u64,
        channel_id: u16,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        if bytes.len() > MAX_DATA_CHANNEL_MESSAGE_BYTES {
            return Err("QuotaExceededError: data-channel message exceeds budget".to_string());
        }
        let peer = self.open_peer_mut(peer_id)?;
        let projected_bytes = peer
            .buffered_data_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| "QuotaExceededError: data-channel byte budget overflow".to_string())?;
        if projected_bytes > MAX_BUFFERED_DATA_BYTES_PER_PEER {
            return Err("QuotaExceededError: data-channel byte budget exceeded".to_string());
        }
        let channel = peer
            .data_channels
            .get_mut(&channel_id)
            .ok_or_else(|| "InvalidStateError: data channel is detached".to_string())?;
        if !channel.open {
            return Err("InvalidStateError: data channel is closed".to_string());
        }
        channel.inbound.push_back(bytes);
        peer.buffered_data_bytes = projected_bytes;
        Ok(())
    }

    pub fn read_data(&mut self, peer_id: u64, channel_id: u16) -> Result<Option<Vec<u8>>, String> {
        let peer = self.open_peer_mut(peer_id)?;
        let channel = peer
            .data_channels
            .get_mut(&channel_id)
            .ok_or_else(|| "InvalidStateError: data channel is detached".to_string())?;
        let value = channel.inbound.pop_front();
        if let Some(bytes) = &value {
            peer.buffered_data_bytes = peer.buffered_data_bytes.saturating_sub(bytes.len());
        }
        Ok(value)
    }

    /// Drain one browser-to-network data-channel message and release the
    /// retained-byte charge once the platform transport accepts ownership.
    pub fn take_outbound_data(
        &mut self,
        peer_id: u64,
        channel_id: u16,
    ) -> Result<Option<Vec<u8>>, String> {
        let peer = self.open_peer_mut(peer_id)?;
        let channel = peer
            .data_channels
            .get_mut(&channel_id)
            .ok_or_else(|| "InvalidStateError: data channel is detached".to_string())?;
        let value = channel.outbound.pop_front();
        if let Some(bytes) = &value {
            peer.buffered_data_bytes = peer.buffered_data_bytes.saturating_sub(bytes.len());
        }
        Ok(value)
    }

    pub fn revoke_capture(&mut self, origin: &str, kind: RtcTrackKind) -> Result<usize, String> {
        let origin = canonical_origin(origin)?;
        let mut stopped = 0usize;
        for peer in self.peers.values_mut().filter(|peer| peer.origin == origin) {
            for track in peer
                .tracks
                .iter_mut()
                .filter(|track| track.kind == kind && !track.stopped)
            {
                track.enabled = false;
                track.stopped = true;
                stopped = stopped.saturating_add(1);
            }
        }
        Ok(stopped)
    }

    pub fn close(&mut self, peer_id: u64) -> bool {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.state = RtcPeerState::Closed;
            for track in &mut peer.tracks {
                track.enabled = false;
                track.stopped = true;
            }
            peer.data_channels.clear();
            peer.buffered_data_bytes = 0;
            peer.ice_candidates.clear();
            true
        } else {
            false
        }
    }

    fn open_peer_mut(&mut self, id: u64) -> Result<&mut RtcPeerConnection, String> {
        let peer = self
            .peers
            .get_mut(&id)
            .ok_or_else(|| "InvalidStateError: WebRTC peer is detached".to_string())?;
        if peer.state == RtcPeerState::Closed {
            return Err("InvalidStateError: WebRTC peer is closed".to_string());
        }
        Ok(peer)
    }
}

fn canonical_origin(value: &str) -> Result<String, String> {
    let url =
        url::Url::parse(value).map_err(|_| "SecurityError: invalid WebRTC origin".to_string())?;
    if url.scheme() != "https" && url.host_str() != Some("localhost") {
        return Err("SecurityError: WebRTC requires a secure origin".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

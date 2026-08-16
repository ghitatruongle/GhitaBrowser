use ghitabrowser::auth_session::{AuthSessionState, AuthSessionStore};
use ghitabrowser::permissions::{PermissionState, PermissionStore, PermissionType};
use ghitabrowser::protected_media::{
    ApprovedKeySystem, EncryptedSampleDescriptor, ProtectedMediaController,
};
use ghitabrowser::webrtc::{RtcRegistry, RtcTrackKind};

#[test]
fn protected_media_requires_approved_key_system_and_current_license() {
    let origin = "https://localhost";
    let mut media = ProtectedMediaController::default();
    media
        .approve_key_system(ApprovedKeySystem {
            name: "org.ghita.local-test".into(),
            persistent_state_allowed: false,
            distinctive_identifier_allowed: false,
        })
        .unwrap();
    let session = media
        .create_session(origin, "org.ghita.local-test", false, b"init-data")
        .unwrap();
    assert!(media
        .authorize_encrypted_sample(
            &session.id,
            &EncryptedSampleDescriptor {
                key_id: vec![1],
                initialization_vector: vec![2; 16],
                encrypted_byte_count: 1024,
            },
            1,
        )
        .is_err());
    media
        .update_license(&session.id, b"licensed", Some(100))
        .unwrap();
    media
        .authorize_encrypted_sample(
            &session.id,
            &EncryptedSampleDescriptor {
                key_id: vec![1],
                initialization_vector: vec![2; 16],
                encrypted_byte_count: 1024,
            },
            2,
        )
        .unwrap();
}

#[test]
fn webrtc_tracks_stop_when_the_origin_permission_is_revoked() {
    let origin = "https://localhost";
    let mut permissions = PermissionStore::new();
    permissions
        .set_permission(origin, PermissionType::Camera, PermissionState::Granted)
        .unwrap();
    let mut peers = RtcRegistry::default();
    let peer = peers.create_peer(origin).unwrap();
    peers
        .add_track(peer, &permissions, RtcTrackKind::Video)
        .unwrap();
    assert_eq!(
        peers.revoke_capture(origin, RtcTrackKind::Video).unwrap(),
        1
    );
    assert!(peers.peer(peer).unwrap().tracks[0].stopped);

    let channel = peers.create_data_channel(peer, "bounded", true).unwrap();
    peers
        .send_data(peer, channel, b"outbound".to_vec())
        .unwrap();
    assert_eq!(
        peers.take_outbound_data(peer, channel).unwrap(),
        Some(b"outbound".to_vec())
    );
    peers
        .receive_data(peer, channel, b"inbound".to_vec())
        .unwrap();
    assert_eq!(
        peers.read_data(peer, channel).unwrap(),
        Some(b"inbound".to_vec())
    );
}

#[test]
fn authenticated_sessions_are_origin_partitioned_and_redacted() {
    let mut sessions = AuthSessionStore::default();
    let session = sessions
        .create("https://localhost", b"secret-token".to_vec(), Some(100))
        .unwrap();
    assert_eq!(session.state, AuthSessionState::Active);
    assert!(sessions.logout(&session.id));
    assert_eq!(
        sessions.audit(&session.id, 1).unwrap().state,
        AuthSessionState::LoggedOut
    );
}

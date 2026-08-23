//! Signed, transactional GhitaBrowser update engine.
//!
//! Runtime trust decisions are Rust-owned. Update packages use a project-owned
//! canonical manifest signed with Ed25519; every file is SHA-256 checked before
//! the installation tree is touched. The user profile, updater state and
//! installation roots are distinct and validated before destructive work.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::package_crypto::{
    sha256_hex, validate_key_id, validate_package_path, CanonicalBytes, PackageCryptoError,
    PublisherTrustStore,
};

const MAX_UPDATE_FILES: usize = 4_096;
const MAX_UPDATE_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_UPDATE_PACKAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;

/// Release publisher keys pinned into the binary at build time. The profile
/// copy of `trusted_publishers.json` can no longer introduce trust on its
/// own — a user-writable file must never be the root of a code-installation
/// signature chain. Rotate by adding a new `(key_id, hex)` entry and signing
/// with `ghita-release-tool` (see tools/).
pub const PINNED_RELEASE_KEYS: &[(&str, &str)] = &[
    (
        "ghita-release-2026-08",
        "612897ca9842a77ddb162a138e62900cc4d7685570b289ba47af34a78aacaa0c",
    ),
];

/// Build the runtime trust store from the pinned keys only.
fn pinned_trust_store() -> PublisherTrustStore {
    let mut trust = PublisherTrustStore::new();
    for (key_id, key_hex) in PINNED_RELEASE_KEYS {
        match crate::package_crypto::decode_hex_exact::<32>(key_hex) {
            Ok(key) => {
                // A malformed pin is a build-time bug; skip rather than fail
                // the whole browser startup, but leave a loud trace.
                if let Err(error) = trust.insert_ed25519(*key_id, key) {
                    log::error!("pinned release key {key_id} rejected: {error:?}");
                }
            }
            Err(error) => log::error!("pinned release key {key_id} has bad hex: {error:?}"),
        }
    }
    trust
}

fn is_pinned_key_id(key_id: &str) -> bool {
    PINNED_RELEASE_KEYS.iter().any(|(id, _)| *id == key_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    InvalidManifest(String),
    InvalidSignature(String),
    DowngradeDisallowed(String),
    UnsupportedVersion(String),
    PayloadCorrupt(String),
    StorageError(String),
    Interrupted(String),
    DiskFull(String),
    UnsafePath(String),
    ConfirmationRequired(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidManifest(message) => write!(f, "invalid update manifest: {message}"),
            Self::InvalidSignature(message) => write!(f, "invalid update signature: {message}"),
            Self::DowngradeDisallowed(message) => write!(f, "downgrade disallowed: {message}"),
            Self::UnsupportedVersion(message) => write!(f, "unsupported version: {message}"),
            Self::PayloadCorrupt(message) => write!(f, "corrupt update payload: {message}"),
            Self::StorageError(message) => write!(f, "update storage error: {message}"),
            Self::Interrupted(message) => write!(f, "update interrupted: {message}"),
            Self::DiskFull(message) => write!(f, "insufficient disk space: {message}"),
            Self::UnsafePath(message) => write!(f, "unsafe update path: {message}"),
            Self::ConfirmationRequired(message) => write!(f, "confirmation required: {message}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<PackageCryptoError> for UpdateError {
    fn from(error: PackageCryptoError) -> Self {
        match error {
            PackageCryptoError::InvalidSignature(message)
            | PackageCryptoError::UnknownPublisher(message)
            | PackageCryptoError::InvalidPublicKey(message) => Self::InvalidSignature(message),
            PackageCryptoError::UnsafePath(path) => Self::UnsafePath(path),
            other => Self::InvalidManifest(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub min_supported_version: String,
    pub channel: String,
    pub download_url: String,
    pub release_notes: Option<String>,
    pub is_delta: bool,
    pub delta_base_version: Option<String>,
    pub publisher_key_id: String,
    /// SHA-256 per payload path. A delta lists only files it replaces.
    pub file_hashes: BTreeMap<String, String>,
    pub signature: String,
}

impl UpdateManifest {
    pub fn validate(&self) -> Result<(), UpdateError> {
        semver::Version::parse(&self.version)
            .map_err(|_| UpdateError::InvalidManifest("version is not valid semver".into()))?;
        semver::Version::parse(&self.min_supported_version).map_err(|_| {
            UpdateError::InvalidManifest("min_supported_version is not valid semver".into())
        })?;
        if self.channel.is_empty()
            || self.channel.len() > 32
            || !self
                .channel
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(UpdateError::InvalidManifest(
                "channel is not a bounded identifier".into(),
            ));
        }
        let download = url::Url::parse(&self.download_url)
            .map_err(|_| UpdateError::InvalidManifest("download_url is invalid".into()))?;
        let loopback_http = download.scheme() == "http"
            && matches!(
                download.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]")
            );
        if download.scheme() != "https" && !loopback_http {
            return Err(UpdateError::InvalidManifest(
                "download_url must use HTTPS outside loopback".into(),
            ));
        }
        if self
            .release_notes
            .as_ref()
            .is_some_and(|notes| notes.len() > 64 * 1024)
        {
            return Err(UpdateError::InvalidManifest(
                "release notes exceed 64 KiB".into(),
            ));
        }
        if self.is_delta {
            let base = self.delta_base_version.as_deref().ok_or_else(|| {
                UpdateError::InvalidManifest("delta update is missing its base version".into())
            })?;
            semver::Version::parse(base).map_err(|_| {
                UpdateError::InvalidManifest("delta base version is invalid".into())
            })?;
        } else if self.delta_base_version.is_some() {
            return Err(UpdateError::InvalidManifest(
                "full update cannot declare a delta base".into(),
            ));
        }
        validate_key_id(&self.publisher_key_id)?;
        if self.signature.len() != 128 {
            return Err(UpdateError::InvalidSignature(
                "Ed25519 signatures must contain 128 hexadecimal characters".into(),
            ));
        }
        if self.file_hashes.is_empty() || self.file_hashes.len() > MAX_UPDATE_FILES {
            return Err(UpdateError::InvalidManifest(format!(
                "file hash count must be 1..={MAX_UPDATE_FILES}"
            )));
        }
        for (path, digest) in &self.file_hashes {
            validate_package_path(path)?;
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(UpdateError::InvalidManifest(format!(
                    "invalid SHA-256 for {path}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateState {
    Idle,
    Available(String),
    Staging,
    Applying,
    RollingBack,
    RolledBack,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct UpdatePackage {
    pub manifest: UpdateManifest,
    pub files: HashMap<String, Vec<u8>>,
}

impl UpdatePackage {
    pub fn new(
        mut manifest: UpdateManifest,
        files: HashMap<String, Vec<u8>>,
    ) -> Result<Self, UpdateError> {
        manifest.file_hashes = files
            .iter()
            .map(|(path, bytes)| (path.clone(), sha256_hex(bytes)))
            .collect();
        let package = Self { manifest, files };
        package.validate_payload()?;
        Ok(package)
    }

    pub fn validate_payload(&self) -> Result<(), UpdateError> {
        self.manifest.validate()?;
        if self.files.len() != self.manifest.file_hashes.len() {
            return Err(UpdateError::PayloadCorrupt(
                "payload paths do not match the signed manifest".into(),
            ));
        }
        let mut total = 0u64;
        for (path, bytes) in &self.files {
            validate_package_path(path)?;
            let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if length > MAX_UPDATE_FILE_BYTES {
                return Err(UpdateError::PayloadCorrupt(format!(
                    "payload file exceeds {MAX_UPDATE_FILE_BYTES} bytes: {path}"
                )));
            }
            total = total
                .checked_add(length)
                .ok_or_else(|| UpdateError::PayloadCorrupt("payload size overflow".into()))?;
            let expected = self.manifest.file_hashes.get(path).ok_or_else(|| {
                UpdateError::PayloadCorrupt(format!("unsigned payload path: {path}"))
            })?;
            if !expected.eq_ignore_ascii_case(&sha256_hex(bytes)) {
                return Err(UpdateError::PayloadCorrupt(format!(
                    "SHA-256 mismatch for {path}"
                )));
            }
        }
        if total > MAX_UPDATE_PACKAGE_BYTES {
            return Err(UpdateError::PayloadCorrupt(format!(
                "package exceeds {MAX_UPDATE_PACKAGE_BYTES} bytes"
            )));
        }
        Ok(())
    }

    pub fn canonical_payload(&self) -> Result<Vec<u8>, UpdateError> {
        self.validate_payload()?;
        let manifest = &self.manifest;
        let mut bytes = CanonicalBytes::new(b"ghitabrowser-update-package-v1");
        bytes.push_str(&manifest.version);
        bytes.push_str(&manifest.min_supported_version);
        bytes.push_str(&manifest.channel);
        bytes.push_str(&manifest.download_url);
        bytes.push_str(manifest.release_notes.as_deref().unwrap_or(""));
        bytes.push_u64(u64::from(manifest.is_delta));
        bytes.push_str(manifest.delta_base_version.as_deref().unwrap_or(""));
        bytes.push_str(&manifest.publisher_key_id);
        bytes.push_u64(manifest.file_hashes.len() as u64);
        for (path, digest) in &manifest.file_hashes {
            bytes.push_str(path);
            bytes.push_str(&digest.to_ascii_lowercase());
        }
        Ok(bytes.finish())
    }

    pub fn compute_digest(&self) -> Result<String, UpdateError> {
        Ok(sha256_hex(&self.canonical_payload()?))
    }

    pub fn verify_signature(&self, trust: &PublisherTrustStore) -> Result<(), UpdateError> {
        let payload = self.canonical_payload()?;
        trust.verify(
            &self.manifest.publisher_key_id,
            &payload,
            &self.manifest.signature,
        )?;
        Ok(())
    }

    pub fn payload_bytes(&self) -> u64 {
        self.files
            .values()
            .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .sum()
    }
}

pub struct VersionComparer;

impl VersionComparer {
    pub fn parse_version(value: &str) -> Result<semver::Version, UpdateError> {
        semver::Version::parse(value)
            .map_err(|_| UpdateError::InvalidManifest(format!("invalid semver: {value}")))
    }

    pub fn is_newer(target: &str, current: &str) -> bool {
        match (Self::parse_version(target), Self::parse_version(current)) {
            (Ok(target), Ok(current)) => target > current,
            _ => false,
        }
    }

    pub fn is_supported(current: &str, minimum: &str) -> bool {
        match (Self::parse_version(current), Self::parse_version(minimum)) {
            (Ok(current), Ok(minimum)) => current >= minimum,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateFault {
    None,
    DiskFullBeforeStage,
    InterruptAfterStage,
    InterruptAfterBackup,
    InterruptDuringApply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum JournalPhase {
    Staged,
    BackedUp,
    Applying,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpdateJournal {
    from_version: String,
    to_version: String,
    phase: JournalPhase,
    is_delta: bool,
}

pub struct UpdateInstaller;

impl UpdateInstaller {
    pub fn stage_update(staging_dir: &Path, package: &UpdatePackage) -> Result<(), UpdateError> {
        package.validate_payload()?;
        recreate_directory(staging_dir, &[])?;
        for (relative, bytes) in &package.files {
            validate_package_path(relative)?;
            let target = staging_dir.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(storage_error)?;
            }
            fs::write(&target, bytes).map_err(storage_error)?;
        }
        verify_tree_files(staging_dir, &package.manifest.file_hashes)
    }

    pub fn create_backup(target_dir: &Path, backup_dir: &Path) -> Result<(), UpdateError> {
        recreate_directory(backup_dir, &[])?;
        if target_dir.exists() {
            copy_tree_bounded(target_dir, backup_dir)?;
        }
        Ok(())
    }

    pub fn apply_staged_update(
        staging_dir: &Path,
        target_dir: &Path,
        backup_dir: &Path,
        fault: UpdateFault,
    ) -> Result<(), UpdateError> {
        Self::create_backup(target_dir, backup_dir)?;
        if fault == UpdateFault::InterruptAfterBackup {
            return Err(UpdateError::Interrupted("fault after backup".into()));
        }
        recreate_directory(target_dir, &[backup_dir])?;
        if let Err(error) = copy_tree_bounded(staging_dir, target_dir) {
            let _ = Self::restore_backup(backup_dir, target_dir);
            return Err(error);
        }
        if fault == UpdateFault::InterruptDuringApply {
            Self::restore_backup(backup_dir, target_dir)?;
            return Err(UpdateError::Interrupted("fault during apply".into()));
        }
        Ok(())
    }

    pub fn restore_backup(backup_dir: &Path, target_dir: &Path) -> Result<(), UpdateError> {
        if !backup_dir.exists() {
            return Err(UpdateError::StorageError("backup is missing".into()));
        }
        recreate_directory(target_dir, &[backup_dir])?;
        copy_tree_bounded(backup_dir, target_dir)
    }
}

pub struct RepairEngine;

impl RepairEngine {
    pub fn repair(
        install_dir: &Path,
        verified_package: &UpdatePackage,
    ) -> Result<Vec<String>, UpdateError> {
        verified_package.validate_payload()?;
        let mut repaired = Vec::new();
        for (relative, bytes) in &verified_package.files {
            let target = install_dir.join(relative);
            let matches = fs::read(&target)
                .map(|installed| {
                    sha256_hex(&installed) == verified_package.manifest.file_hashes[relative]
                })
                .unwrap_or(false);
            if !matches {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(storage_error)?;
                }
                atomic_write(&target, bytes)?;
                repaired.push(relative.clone());
            }
        }
        verify_tree_files(install_dir, &verified_package.manifest.file_hashes)?;
        Ok(repaired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallChoice {
    KeepUserProfile,
    RemoveUserProfile { confirmed_path: PathBuf },
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedUpdaterState {
    current_version: String,
    state: UpdateState,
    installed_manifest: Option<UpdateManifest>,
}

pub struct UpdateManager {
    pub current_version: String,
    pub state: UpdateState,
    pub installed_manifest: Option<UpdateManifest>,
    trust: PublisherTrustStore,
    install_dir: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    user_profile_dir: Option<PathBuf>,
}

impl UpdateManager {
    pub fn new_in_memory(current_version: &str) -> Self {
        Self::new_in_memory_with_trust(current_version, PublisherTrustStore::new())
    }

    pub fn new_in_memory_with_trust(current_version: &str, trust: PublisherTrustStore) -> Self {
        Self {
            current_version: current_version.to_string(),
            state: UpdateState::Idle,
            installed_manifest: None,
            trust,
            install_dir: None,
            state_dir: None,
            user_profile_dir: None,
        }
    }

    /// Isolated constructor for deterministic tests and update helpers.
    pub fn new_with_paths(
        current_version: &str,
        install_dir: &Path,
        state_dir: &Path,
        user_profile_dir: &Path,
        mut trust: PublisherTrustStore,
    ) -> Result<Self, UpdateError> {
        VersionComparer::parse_version(current_version)?;
        let install_dir = absolute_path(install_dir)?;
        let state_dir = absolute_path(state_dir)?;
        let user_profile_dir = absolute_path(user_profile_dir)?;
        validate_distinct_roots(&install_dir, &state_dir, &user_profile_dir)?;
        fs::create_dir_all(&install_dir).map_err(storage_error)?;
        fs::create_dir_all(&state_dir).map_err(storage_error)?;
        fs::create_dir_all(&user_profile_dir).map_err(storage_error)?;
        let trust_path = state_dir.join("trusted_publishers.json");
        if trust_path.exists() {
            let persisted: BTreeMap<String, [u8; 32]> = read_bounded_json(&trust_path, 64 * 1024)?;
            let persisted = PublisherTrustStore::import_ed25519(persisted)?;
            for (key_id, key) in persisted.export_ed25519() {
                // The profile file is user-writable, so it can only re-state
                // keys that are already pinned — it can never introduce a
                // new trust root.
                if is_pinned_key_id(&key_id) {
                    trust.insert_ed25519(key_id, key)?;
                } else {
                    log::warn!(
                        "ignoring non-pinned publisher key {:?} from profile",
                        key_id
                    );
                }
            }
        }
        let mut manager = Self {
            current_version: current_version.to_string(),
            state: UpdateState::Idle,
            installed_manifest: None,
            trust,
            install_dir: Some(install_dir),
            state_dir: Some(state_dir),
            user_profile_dir: Some(user_profile_dir),
        };
        manager.load_state()?;
        manager.recover_interrupted_update()?;
        Ok(manager)
    }

    /// Product constructor. It points at the actual executable directory but
    /// does not modify it until a trusted package is explicitly applied.
    pub fn new_for_application(
        current_version: &str,
        profile_dir: &Path,
    ) -> Result<Self, UpdateError> {
        let executable = std::env::current_exe().map_err(storage_error)?;
        let install_dir = executable.parent().ok_or_else(|| {
            UpdateError::UnsafePath("current executable has no parent directory".into())
        })?;
        let mut manager = Self::new_with_paths(
            current_version,
            install_dir,
            &profile_dir.join("updater"),
            profile_dir,
            pinned_trust_store(),
        )?;
        manager.enforce_version_floor();
        Ok(manager)
    }

    /// The persisted `current_version` baseline lives in user-writable
    /// storage; rewinding it would let an attacker replay old (genuinely
    /// signed) manifests to downgrade the browser. The running binary's own
    /// compile-time version is a floor that cannot be edited from disk.
    fn enforce_version_floor(&mut self) {
        let running = crate::VERSION;
        let Ok(running_version) = VersionComparer::parse_version(running) else {
            return;
        };
        match VersionComparer::parse_version(&self.current_version) {
            Ok(persisted) if persisted < running_version => {
                log::warn!(
                    "updater baseline {} was below this build ({running}); lifting",
                    self.current_version
                );
                self.current_version = running.to_string();
            }
            _ => {}
        }
    }

    pub fn trust_publisher(
        &mut self,
        key_id: impl Into<String>,
        public_key: [u8; 32],
    ) -> Result<(), UpdateError> {
        self.trust.insert_ed25519(key_id, public_key)?;
        if let Some(state_dir) = &self.state_dir {
            atomic_json_write(
                &state_dir.join("trusted_publishers.json"),
                &self.trust.export_ed25519(),
            )?;
        }
        Ok(())
    }

    pub fn check_update(&mut self, manifest: &UpdateManifest) -> Result<bool, UpdateError> {
        manifest.validate()?;
        let current = VersionComparer::parse_version(&self.current_version)?;
        let target = VersionComparer::parse_version(&manifest.version)?;
        let minimum = VersionComparer::parse_version(&manifest.min_supported_version)?;
        if current < minimum {
            return Err(UpdateError::UnsupportedVersion(format!(
                "{} is older than required {}",
                self.current_version, manifest.min_supported_version
            )));
        }
        if target <= current {
            return Err(UpdateError::DowngradeDisallowed(format!(
                "target {} is not newer than {}",
                manifest.version, self.current_version
            )));
        }
        if manifest.is_delta
            && manifest.delta_base_version.as_deref() != Some(self.current_version.as_str())
        {
            return Err(UpdateError::PayloadCorrupt(format!(
                "delta base {:?} does not match {}",
                manifest.delta_base_version, self.current_version
            )));
        }
        self.state = UpdateState::Available(manifest.version.clone());
        Ok(true)
    }

    pub fn apply_update(&mut self, package: UpdatePackage) -> Result<String, UpdateError> {
        self.apply_update_with_fault(package, UpdateFault::None)
    }

    pub fn apply_update_with_fault(
        &mut self,
        package: UpdatePackage,
        fault: UpdateFault,
    ) -> Result<String, UpdateError> {
        package.verify_signature(&self.trust)?;
        self.check_update(&package.manifest)?;
        let target_version = package.manifest.version.clone();
        if self.install_dir.is_none() {
            self.current_version = target_version.clone();
            self.installed_manifest = Some(package.manifest);
            self.state = UpdateState::Idle;
            return Ok(target_version);
        }
        if fault == UpdateFault::DiskFullBeforeStage {
            return Err(UpdateError::DiskFull("injected preflight failure".into()));
        }
        self.preflight_space(package.payload_bytes())?;
        let state_dir = self.state_dir()?.to_path_buf();
        let install_dir = self.install_dir()?.to_path_buf();
        let staging = state_dir.join("staging");
        let candidate = state_dir.join("candidate");
        let backup = state_dir.join("backup");
        self.state = UpdateState::Staging;
        UpdateInstaller::stage_update(&staging, &package)?;
        if package.manifest.is_delta {
            recreate_directory(&candidate, &[&install_dir])?;
            copy_tree_bounded(&install_dir, &candidate)?;
            copy_tree_bounded(&staging, &candidate)?;
        } else {
            recreate_directory(&candidate, &[&install_dir])?;
            copy_tree_bounded(&staging, &candidate)?;
        }
        self.write_journal(UpdateJournal {
            from_version: self.current_version.clone(),
            to_version: target_version.clone(),
            phase: JournalPhase::Staged,
            is_delta: package.manifest.is_delta,
        })?;
        if fault == UpdateFault::InterruptAfterStage {
            self.state = UpdateState::Failed("interrupted after stage".into());
            return Err(UpdateError::Interrupted("fault after stage".into()));
        }

        UpdateInstaller::create_backup(&install_dir, &backup)?;
        self.update_journal_phase(JournalPhase::BackedUp)?;
        if fault == UpdateFault::InterruptAfterBackup {
            self.state = UpdateState::Failed("interrupted after backup".into());
            return Err(UpdateError::Interrupted("fault after backup".into()));
        }
        self.state = UpdateState::Applying;
        self.update_journal_phase(JournalPhase::Applying)?;
        let apply_result =
            UpdateInstaller::apply_staged_update(&candidate, &install_dir, &backup, fault);
        if let Err(error) = apply_result {
            let rollback = UpdateInstaller::restore_backup(&backup, &install_dir);
            self.state = match rollback {
                Ok(()) => UpdateState::RolledBack,
                Err(ref rollback_error) => UpdateState::Failed(rollback_error.to_string()),
            };
            self.persist_state()?;
            return Err(error);
        }
        verify_tree_files(&install_dir, &package.manifest.file_hashes)?;
        self.update_journal_phase(JournalPhase::Committed)?;
        self.current_version = target_version.clone();
        self.installed_manifest = Some(package.manifest);
        self.state = UpdateState::Idle;
        self.persist_state()?;
        self.clear_journal_and_staging()?;
        Ok(target_version)
    }

    pub fn recover_interrupted_update(&mut self) -> Result<bool, UpdateError> {
        let Some(state_dir) = self.state_dir.as_ref() else {
            return Ok(false);
        };
        let journal_path = state_dir.join("journal.json");
        if !journal_path.exists() {
            return Ok(false);
        }
        let journal: UpdateJournal = read_bounded_json(&journal_path, MAX_MANIFEST_BYTES)?;
        match journal.phase {
            JournalPhase::Staged => {
                self.state = UpdateState::Idle;
            }
            JournalPhase::BackedUp | JournalPhase::Applying => {
                let backup = state_dir.join("backup");
                let install = self.install_dir()?.to_path_buf();
                self.state = UpdateState::RollingBack;
                UpdateInstaller::restore_backup(&backup, &install)?;
                self.current_version = journal.from_version;
                self.state = UpdateState::RolledBack;
            }
            JournalPhase::Committed => {
                self.current_version = journal.to_version;
                self.state = UpdateState::Idle;
            }
        }
        self.persist_state()?;
        self.clear_journal_and_staging()?;
        Ok(true)
    }

    pub fn rollback(&mut self) -> Result<(), UpdateError> {
        let install = self.install_dir()?.to_path_buf();
        let state = self.state_dir()?.to_path_buf();
        self.state = UpdateState::RollingBack;
        UpdateInstaller::restore_backup(&state.join("backup"), &install)?;
        self.state = UpdateState::RolledBack;
        self.persist_state()
    }

    pub fn repair(&mut self, expected_package: &UpdatePackage) -> Result<Vec<String>, UpdateError> {
        expected_package.verify_signature(&self.trust)?;
        let install = self.install_dir()?.to_path_buf();
        let repaired = RepairEngine::repair(&install, expected_package)?;
        self.persist_state()?;
        Ok(repaired)
    }

    pub fn uninstall(&mut self, choice: UninstallChoice) -> Result<(), UpdateError> {
        let install = self.install_dir()?.to_path_buf();
        let state = self.state_dir()?.to_path_buf();
        let profile = self.user_profile_dir()?.to_path_buf();
        let remove_profile = match choice {
            UninstallChoice::KeepUserProfile => false,
            UninstallChoice::RemoveUserProfile { confirmed_path } => {
                let confirmed = absolute_path(&confirmed_path)?;
                if confirmed != profile {
                    return Err(UpdateError::ConfirmationRequired(
                        "confirmed profile path does not match updater ownership".into(),
                    ));
                }
                validate_destructive_target(&profile, &[])?;
                true
            }
        };
        validate_destructive_target(&install, &[&state, &profile])?;
        if install.exists() {
            fs::remove_dir_all(&install).map_err(storage_error)?;
        }
        if state.exists() {
            validate_destructive_target(&state, &[&profile])?;
            fs::remove_dir_all(&state).map_err(storage_error)?;
        }
        if remove_profile && profile.exists() {
            fs::remove_dir_all(profile).map_err(storage_error)?;
        }
        self.state = UpdateState::Idle;
        Ok(())
    }

    fn preflight_space(&self, payload_bytes: u64) -> Result<(), UpdateError> {
        let state_dir = self.state_dir()?;
        let available = fs2::available_space(state_dir).map_err(storage_error)?;
        let required = payload_bytes
            .saturating_mul(3)
            .saturating_add(32 * 1024 * 1024);
        if available < required {
            return Err(UpdateError::DiskFull(format!(
                "requires {required} bytes, only {available} available"
            )));
        }
        Ok(())
    }

    fn install_dir(&self) -> Result<&Path, UpdateError> {
        self.install_dir
            .as_deref()
            .ok_or_else(|| UpdateError::StorageError("no installation root configured".into()))
    }

    fn state_dir(&self) -> Result<&Path, UpdateError> {
        self.state_dir
            .as_deref()
            .ok_or_else(|| UpdateError::StorageError("no updater state root configured".into()))
    }

    fn user_profile_dir(&self) -> Result<&Path, UpdateError> {
        self.user_profile_dir
            .as_deref()
            .ok_or_else(|| UpdateError::StorageError("no user profile root configured".into()))
    }

    fn write_journal(&self, journal: UpdateJournal) -> Result<(), UpdateError> {
        atomic_json_write(&self.state_dir()?.join("journal.json"), &journal)
    }

    fn update_journal_phase(&self, phase: JournalPhase) -> Result<(), UpdateError> {
        let path = self.state_dir()?.join("journal.json");
        let mut journal: UpdateJournal = read_bounded_json(&path, MAX_MANIFEST_BYTES)?;
        journal.phase = phase;
        atomic_json_write(&path, &journal)
    }

    fn clear_journal_and_staging(&self) -> Result<(), UpdateError> {
        let state = self.state_dir()?;
        for path in [
            state.join("journal.json"),
            state.join("staging"),
            state.join("candidate"),
        ] {
            if path.is_dir() {
                fs::remove_dir_all(path).map_err(storage_error)?;
            } else if path.exists() {
                fs::remove_file(path).map_err(storage_error)?;
            }
        }
        Ok(())
    }

    fn persist_state(&self) -> Result<(), UpdateError> {
        let Some(state_dir) = &self.state_dir else {
            return Ok(());
        };
        fs::create_dir_all(state_dir).map_err(storage_error)?;
        atomic_json_write(
            &state_dir.join("state.json"),
            &PersistedUpdaterState {
                current_version: self.current_version.clone(),
                state: self.state.clone(),
                installed_manifest: self.installed_manifest.clone(),
            },
        )
    }

    fn load_state(&mut self) -> Result<(), UpdateError> {
        let Some(state_dir) = &self.state_dir else {
            return Ok(());
        };
        let path = state_dir.join("state.json");
        if !path.exists() {
            return Ok(());
        }
        let persisted: PersistedUpdaterState = read_bounded_json(&path, MAX_MANIFEST_BYTES)?;
        VersionComparer::parse_version(&persisted.current_version)?;
        self.current_version = persisted.current_version;
        self.state = persisted.state;
        self.installed_manifest = persisted.installed_manifest;
        Ok(())
    }
}

fn verify_tree_files(root: &Path, expected: &BTreeMap<String, String>) -> Result<(), UpdateError> {
    for (relative, digest) in expected {
        validate_package_path(relative)?;
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(storage_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(UpdateError::PayloadCorrupt(format!(
                "payload target is not a regular file: {relative}"
            )));
        }
        if metadata.len() > MAX_UPDATE_FILE_BYTES {
            return Err(UpdateError::PayloadCorrupt(format!(
                "payload target exceeds size limit: {relative}"
            )));
        }
        let bytes = fs::read(&path).map_err(storage_error)?;
        if !digest.eq_ignore_ascii_case(&sha256_hex(&bytes)) {
            return Err(UpdateError::PayloadCorrupt(format!(
                "installed SHA-256 mismatch: {relative}"
            )));
        }
    }
    Ok(())
}

fn copy_tree_bounded(source: &Path, destination: &Path) -> Result<(), UpdateError> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(destination).map_err(storage_error)?;
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    let mut files = 0usize;
    let mut bytes = 0u64;
    while let Some((from, to)) = stack.pop() {
        for entry in fs::read_dir(&from).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(storage_error)?;
            if metadata.file_type().is_symlink() {
                return Err(UpdateError::UnsafePath(format!(
                    "symbolic links are forbidden in update trees: {}",
                    entry.path().display()
                )));
            }
            let target = to.join(entry.file_name());
            if metadata.is_dir() {
                fs::create_dir_all(&target).map_err(storage_error)?;
                stack.push((entry.path(), target));
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes.saturating_add(metadata.len());
                if files > MAX_UPDATE_FILES || bytes > MAX_UPDATE_PACKAGE_BYTES {
                    return Err(UpdateError::PayloadCorrupt(
                        "update tree exceeds file or byte limit".into(),
                    ));
                }
                fs::copy(entry.path(), target).map_err(storage_error)?;
            }
        }
    }
    Ok(())
}

fn validate_distinct_roots(
    install: &Path,
    state: &Path,
    profile: &Path,
) -> Result<(), UpdateError> {
    if install == state
        || install == profile
        || state == profile
        || state.starts_with(install)
        || profile.starts_with(install)
        || install.starts_with(state)
    {
        return Err(UpdateError::UnsafePath(
            "install, updater-state and profile ownership roots overlap unsafely".into(),
        ));
    }
    if !state.starts_with(profile) {
        return Err(UpdateError::UnsafePath(
            "updater state must be owned by the selected user profile".into(),
        ));
    }
    Ok(())
}

fn validate_destructive_target(target: &Path, protected: &[&Path]) -> Result<(), UpdateError> {
    if !target.is_absolute()
        || target.parent().is_none()
        || target.parent().is_some_and(Path::is_root)
    {
        return Err(UpdateError::UnsafePath(format!(
            "refusing broad destructive target: {}",
            target.display()
        )));
    }
    if protected
        .iter()
        .any(|path| *path == target || path.starts_with(target))
    {
        return Err(UpdateError::UnsafePath(format!(
            "target contains protected data: {}",
            target.display()
        )));
    }
    Ok(())
}

trait PathRoot {
    fn is_root(&self) -> bool;
}

impl PathRoot for Path {
    fn is_root(&self) -> bool {
        self.parent().is_none()
    }
}

fn recreate_directory(path: &Path, protected: &[&Path]) -> Result<(), UpdateError> {
    if path.exists() {
        validate_destructive_target(path, protected)?;
        fs::remove_dir_all(path).map_err(storage_error)?;
    }
    fs::create_dir_all(path).map_err(storage_error)
}

fn absolute_path(path: &Path) -> Result<PathBuf, UpdateError> {
    if path.as_os_str().is_empty() {
        return Err(UpdateError::UnsafePath("path is empty".into()));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(storage_error)
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), UpdateError> {
    fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(storage_error)?;
    crate::fs_atomic::atomic_write_bytes(path, bytes).map_err(storage_error)
}

fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<(), UpdateError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(storage_error)?;
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| UpdateError::StorageError(error.to_string()))?;
    atomic_write(path, &bytes)
}

fn read_bounded_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    maximum_bytes: u64,
) -> Result<T, UpdateError> {
    let metadata = fs::metadata(path).map_err(storage_error)?;
    if metadata.len() > maximum_bytes {
        return Err(UpdateError::PayloadCorrupt(format!(
            "JSON evidence exceeds {maximum_bytes} bytes: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(storage_error)?;
    serde_json::from_slice(&bytes).map_err(|error| UpdateError::StorageError(error.to_string()))
}

fn storage_error(error: std::io::Error) -> UpdateError {
    UpdateError::StorageError(error.to_string())
}

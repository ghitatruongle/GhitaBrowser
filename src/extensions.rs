//! GhitaBrowser-owned extension package and capability subsystem.
//!
//! The manifest, review flow and capability model are original to this
//! project. Package authenticity is Ed25519 over a canonical byte stream;
//! content and storage are bounded and profile-owned paths never use an
//! untrusted identifier before it has passed strict validation.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::package_crypto::{
    sha256_hex, validate_key_id, validate_package_path, CanonicalBytes, PackageCryptoError,
    PublisherTrustStore,
};

pub const MAX_EXTENSION_FILES: usize = 128;
pub const MAX_EXTENSION_PACKAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_EXTENSION_SCRIPT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_EXTENSION_STORAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EXTENSION_STORAGE_ENTRIES: usize = 1_024;
pub const EXTENSION_WORKER_STEP_BUDGET: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    InvalidManifest(String),
    InvalidSignature(String),
    PermissionDenied(String),
    ExecutionFailed(String),
    NotFound(String),
    StorageError(String),
    AlreadyInstalled(String),
    ResourceLimit(String),
    ReviewRequired(String),
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidManifest(message) => write!(f, "invalid manifest: {message}"),
            Self::InvalidSignature(message) => write!(f, "invalid signature: {message}"),
            Self::PermissionDenied(message) => write!(f, "permission denied: {message}"),
            Self::ExecutionFailed(message) => write!(f, "execution failed: {message}"),
            Self::NotFound(message) => write!(f, "extension not found: {message}"),
            Self::StorageError(message) => write!(f, "storage error: {message}"),
            Self::AlreadyInstalled(message) => write!(f, "extension already installed: {message}"),
            Self::ResourceLimit(message) => write!(f, "resource limit: {message}"),
            Self::ReviewRequired(message) => write!(f, "permission review required: {message}"),
        }
    }
}

impl std::error::Error for ExtensionError {}

impl From<PackageCryptoError> for ExtensionError {
    fn from(error: PackageCryptoError) -> Self {
        match error {
            PackageCryptoError::InvalidSignature(message)
            | PackageCryptoError::UnknownPublisher(message)
            | PackageCryptoError::InvalidPublicKey(message) => Self::InvalidSignature(message),
            other => Self::InvalidManifest(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPermission {
    Network,
    Storage,
    Tabs,
    ContentScript,
    Custom(String),
}

impl ExtensionPermission {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Network => "network",
            Self::Storage => "storage",
            Self::Tabs => "tabs",
            Self::ContentScript => "content_script",
            Self::Custom(value) => value,
        }
    }

    fn validate(&self) -> Result<(), ExtensionError> {
        if let Self::Custom(value) = self {
            if value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err(ExtensionError::InvalidManifest(
                    "custom permission is not a bounded capability identifier".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentScriptConfig {
    pub matches: Vec<String>,
    pub script_path: String,
}

impl ContentScriptConfig {
    pub fn validate(&self) -> Result<(), ExtensionError> {
        validate_package_path(&self.script_path)?;
        if self.matches.is_empty() || self.matches.len() > 64 {
            return Err(ExtensionError::InvalidManifest(
                "a content script needs 1..=64 URL patterns".into(),
            ));
        }
        for pattern in &self.matches {
            ParsedMatchPattern::parse(pattern)?;
        }
        Ok(())
    }

    pub fn matches_url(&self, url: &str) -> bool {
        self.matches.iter().any(|pattern| {
            ParsedMatchPattern::parse(pattern)
                .map(|parsed| parsed.matches(url))
                .unwrap_or(false)
        })
    }
}

#[derive(Debug, Clone)]
struct ParsedMatchPattern {
    any_scheme: bool,
    scheme: String,
    host: String,
    include_subdomains: bool,
    path_prefix: String,
}

impl ParsedMatchPattern {
    fn parse(pattern: &str) -> Result<Self, ExtensionError> {
        if pattern == "<all_urls>" {
            return Ok(Self {
                any_scheme: true,
                scheme: String::new(),
                host: "*".into(),
                include_subdomains: true,
                path_prefix: "/".into(),
            });
        }
        if pattern.len() > 512 || pattern == "*" {
            return Err(ExtensionError::InvalidManifest(format!(
                "invalid content-script match pattern: {pattern}"
            )));
        }
        let (scheme, remainder) = pattern.split_once("://").ok_or_else(|| {
            ExtensionError::InvalidManifest(format!("match pattern lacks scheme: {pattern}"))
        })?;
        if !matches!(scheme, "http" | "https" | "*") {
            return Err(ExtensionError::InvalidManifest(
                "content scripts are restricted to HTTP(S) origins".into(),
            ));
        }
        let (host_pattern, path_pattern) = remainder.split_once('/').ok_or_else(|| {
            ExtensionError::InvalidManifest(format!("match pattern lacks path: {pattern}"))
        })?;
        let (host, include_subdomains) = if host_pattern == "*" {
            ("*", true)
        } else if let Some(host) = host_pattern.strip_prefix("*.") {
            (host, true)
        } else {
            (host_pattern, false)
        };
        if host != "*"
            && (host.is_empty()
                || host.len() > 253
                || !host
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
        {
            return Err(ExtensionError::InvalidManifest(format!(
                "invalid match-pattern host: {host_pattern}"
            )));
        }
        if path_pattern.contains('*') && !path_pattern.ends_with('*') {
            return Err(ExtensionError::InvalidManifest(
                "only a trailing path wildcard is supported".into(),
            ));
        }
        let path_prefix = format!(
            "/{}",
            path_pattern.strip_suffix('*').unwrap_or(path_pattern)
        );
        Ok(Self {
            any_scheme: scheme == "*",
            scheme: scheme.to_string(),
            host: host.to_ascii_lowercase(),
            include_subdomains,
            path_prefix,
        })
    }

    fn matches(&self, candidate: &str) -> bool {
        let Ok(url) = url::Url::parse(candidate) else {
            return false;
        };
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        if !self.any_scheme && url.scheme() != self.scheme {
            return false;
        }
        let Some(candidate_host) = url.host_str().map(str::to_ascii_lowercase) else {
            return false;
        };
        let host_matches = self.host == "*"
            || candidate_host == self.host
            || (self.include_subdomains
                && candidate_host
                    .strip_suffix(&self.host)
                    .is_some_and(|prefix| prefix.ends_with('.')));
        host_matches && url.path().starts_with(&self.path_prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GhitaExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub permissions: Vec<ExtensionPermission>,
    /// Explicit network origins. Wildcard entries use the same bounded grammar
    /// as content-script patterns and are ignored unless Network is approved.
    #[serde(default)]
    pub network_origins: Vec<String>,
    pub background_script: Option<String>,
    pub content_scripts: Vec<ContentScriptConfig>,
    pub publisher_key_id: String,
    pub signature: String,
}

impl GhitaExtensionManifest {
    pub fn validate(&self) -> Result<(), ExtensionError> {
        validate_component_id(&self.id, "extension id")?;
        validate_text(&self.name, 1, 128, "name")?;
        semver::Version::parse(&self.version).map_err(|_| {
            ExtensionError::InvalidManifest("version must be semantic versioning".into())
        })?;
        if let Some(description) = &self.description {
            validate_text(description, 0, 4_096, "description")?;
        }
        if let Some(author) = &self.author {
            validate_text(author, 0, 256, "author")?;
        }
        validate_key_id(&self.publisher_key_id)?;
        if self.signature.len() != 128 {
            return Err(ExtensionError::InvalidSignature(
                "Ed25519 signatures must contain 128 hexadecimal characters".into(),
            ));
        }
        if self.permissions.len() > 32 {
            return Err(ExtensionError::ResourceLimit(
                "at most 32 permissions may be declared".into(),
            ));
        }
        let mut unique = HashSet::new();
        for permission in &self.permissions {
            permission.validate()?;
            if !unique.insert(permission) {
                return Err(ExtensionError::InvalidManifest(
                    "duplicate permission declaration".into(),
                ));
            }
        }
        if self.network_origins.len() > 64 {
            return Err(ExtensionError::ResourceLimit(
                "at most 64 network origins may be declared".into(),
            ));
        }
        if !self.network_origins.is_empty() && !unique.contains(&ExtensionPermission::Network) {
            return Err(ExtensionError::InvalidManifest(
                "network origins require the network permission".into(),
            ));
        }
        for pattern in &self.network_origins {
            ParsedMatchPattern::parse(pattern)?;
        }
        if let Some(path) = &self.background_script {
            validate_package_path(path)?;
        }
        if self.content_scripts.len() > 64 {
            return Err(ExtensionError::ResourceLimit(
                "at most 64 content scripts may be declared".into(),
            ));
        }
        if !self.content_scripts.is_empty() && !unique.contains(&ExtensionPermission::ContentScript)
        {
            return Err(ExtensionError::InvalidManifest(
                "content scripts require the content_script permission".into(),
            ));
        }
        for content_script in &self.content_scripts {
            content_script.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ExtensionPackage {
    pub manifest: GhitaExtensionManifest,
    pub files: HashMap<String, String>,
}

impl ExtensionPackage {
    pub fn validate(&self) -> Result<(), ExtensionError> {
        self.manifest.validate()?;
        if self.files.is_empty() || self.files.len() > MAX_EXTENSION_FILES {
            return Err(ExtensionError::ResourceLimit(format!(
                "package file count must be 1..={MAX_EXTENSION_FILES}"
            )));
        }
        let mut total = 0usize;
        for (path, content) in &self.files {
            validate_package_path(path)?;
            if content.len() > MAX_EXTENSION_SCRIPT_BYTES {
                return Err(ExtensionError::ResourceLimit(format!(
                    "package file {path} exceeds {MAX_EXTENSION_SCRIPT_BYTES} bytes"
                )));
            }
            total = total
                .checked_add(path.len() + content.len())
                .ok_or_else(|| ExtensionError::ResourceLimit("package size overflow".into()))?;
        }
        if total > MAX_EXTENSION_PACKAGE_BYTES {
            return Err(ExtensionError::ResourceLimit(format!(
                "package exceeds {MAX_EXTENSION_PACKAGE_BYTES} bytes"
            )));
        }
        if let Some(background) = &self.manifest.background_script {
            if !self.files.contains_key(background) {
                return Err(ExtensionError::InvalidManifest(format!(
                    "background script is missing: {background}"
                )));
            }
        }
        for content_script in &self.manifest.content_scripts {
            if !self.files.contains_key(&content_script.script_path) {
                return Err(ExtensionError::InvalidManifest(format!(
                    "content script is missing: {}",
                    content_script.script_path
                )));
            }
        }
        Ok(())
    }

    pub fn canonical_payload(&self) -> Result<Vec<u8>, ExtensionError> {
        self.validate()?;
        let manifest = &self.manifest;
        let mut bytes = CanonicalBytes::new(b"ghitabrowser-extension-package-v1");
        bytes.push_str(&manifest.id);
        bytes.push_str(&manifest.name);
        bytes.push_str(&manifest.version);
        bytes.push_str(manifest.description.as_deref().unwrap_or(""));
        bytes.push_str(manifest.author.as_deref().unwrap_or(""));
        bytes.push_str(&manifest.publisher_key_id);

        let mut permissions: Vec<_> = manifest
            .permissions
            .iter()
            .map(|value| value.as_str())
            .collect();
        permissions.sort_unstable();
        bytes.push_u64(permissions.len() as u64);
        for permission in permissions {
            bytes.push_str(permission);
        }
        let mut origins = manifest.network_origins.clone();
        origins.sort();
        bytes.push_u64(origins.len() as u64);
        for origin in origins {
            bytes.push_str(&origin);
        }
        bytes.push_str(manifest.background_script.as_deref().unwrap_or(""));
        bytes.push_u64(manifest.content_scripts.len() as u64);
        for script in &manifest.content_scripts {
            bytes.push_str(&script.script_path);
            bytes.push_u64(script.matches.len() as u64);
            for pattern in &script.matches {
                bytes.push_str(pattern);
            }
        }
        let mut file_paths: Vec<_> = self.files.keys().collect();
        file_paths.sort();
        bytes.push_u64(file_paths.len() as u64);
        for path in file_paths {
            bytes.push_str(path);
            bytes.push_bytes(self.files[path].as_bytes());
        }
        Ok(bytes.finish())
    }

    pub fn compute_digest(&self) -> Result<String, ExtensionError> {
        Ok(sha256_hex(&self.canonical_payload()?))
    }

    pub fn verify_signature(&self, trust: &PublisherTrustStore) -> Result<(), ExtensionError> {
        let payload = self.canonical_payload()?;
        trust.verify(
            &self.manifest.publisher_key_id,
            &payload,
            &self.manifest.signature,
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionStatus {
    Enabled,
    Disabled,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtensionStorage {
    data: HashMap<String, String>,
}

impl ExtensionStorage {
    pub fn get(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }

    pub fn set(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), ExtensionError> {
        let key = key.into();
        let value = value.into();
        if key.is_empty() || key.len() > 256 || value.len() > 64 * 1024 {
            return Err(ExtensionError::ResourceLimit(
                "storage key/value exceeds its per-entry limit".into(),
            ));
        }
        if !self.data.contains_key(&key) && self.data.len() >= MAX_EXTENSION_STORAGE_ENTRIES {
            return Err(ExtensionError::ResourceLimit(
                "extension storage entry limit exceeded".into(),
            ));
        }
        let existing = self.data.get(&key).map_or(0, |item| key.len() + item.len());
        let current: usize = self
            .data
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum();
        let next = current
            .saturating_sub(existing)
            .saturating_add(key.len() + value.len());
        if next > MAX_EXTENSION_STORAGE_BYTES {
            return Err(ExtensionError::ResourceLimit(
                "extension storage byte limit exceeded".into(),
            ));
        }
        self.data.insert(key, value);
        Ok(())
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.data.remove(key)
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[derive(Debug)]
pub struct ExtensionWorker {
    pub extension_id: String,
    pub permissions: HashSet<ExtensionPermission>,
    pub storage: ExtensionStorage,
    pub is_cancelled: bool,
    pub remaining_executions: u32,
}

impl ExtensionWorker {
    pub fn new(extension_id: impl Into<String>, permissions: Vec<ExtensionPermission>) -> Self {
        Self {
            extension_id: extension_id.into(),
            permissions: permissions.into_iter().collect(),
            storage: ExtensionStorage::default(),
            is_cancelled: false,
            remaining_executions: 1,
        }
    }

    pub fn has_permission(&self, permission: &ExtensionPermission) -> bool {
        self.permissions.contains(permission)
    }

    pub fn check_permission(&self, permission: &ExtensionPermission) -> Result<(), ExtensionError> {
        if self.has_permission(permission) {
            Ok(())
        } else {
            Err(ExtensionError::PermissionDenied(format!(
                "extension '{}' lacks required permission: {permission:?}",
                self.extension_id
            )))
        }
    }

    pub fn cancel(&mut self) {
        self.is_cancelled = true;
    }

    pub fn execute_script(
        &mut self,
        script_name: &str,
        script_code: &str,
        engine: &mut crate::javascript::JsvEngine,
    ) -> Result<String, ExtensionError> {
        if self.is_cancelled || self.remaining_executions == 0 {
            return Err(ExtensionError::ExecutionFailed(
                "worker was cancelled or already consumed".into(),
            ));
        }
        if script_code.len() > MAX_EXTENSION_SCRIPT_BYTES {
            return Err(ExtensionError::ResourceLimit(
                "background script exceeds byte budget".into(),
            ));
        }
        self.remaining_executions -= 1;
        engine
            .eval_with_step_limit(script_code, EXTENSION_WORKER_STEP_BUDGET)
            .map(|value| format!("{value:?}"))
            .map_err(|error| {
                ExtensionError::ExecutionFailed(format!("script '{script_name}' failed: {error}"))
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionRecord {
    pub manifest: GhitaExtensionManifest,
    pub status: ExtensionStatus,
    pub granted_permissions: HashSet<ExtensionPermission>,
    pub granted_network_origins: BTreeSet<String>,
    pub package_digest: String,
    pub installed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionPermissionReview {
    pub extension_id: String,
    pub publisher_key_id: String,
    pub package_digest: String,
    pub requested_permissions: BTreeSet<ExtensionPermission>,
    pub requested_network_origins: BTreeSet<String>,
    pub content_script_patterns: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct ExtensionApproval {
    pub package_digest: String,
    pub approved_permissions: BTreeSet<ExtensionPermission>,
    pub approved_network_origins: BTreeSet<String>,
    pub user_confirmed: bool,
}

impl ExtensionApproval {
    pub fn approve_all(review: &ExtensionPermissionReview) -> Self {
        Self {
            package_digest: review.package_digest.clone(),
            approved_permissions: review.requested_permissions.clone(),
            approved_network_origins: review.requested_network_origins.clone(),
            user_confirmed: true,
        }
    }
}

pub struct ExtensionManager {
    profile_dir: Option<PathBuf>,
    trust: PublisherTrustStore,
    extensions: HashMap<String, ExtensionRecord>,
    storages: HashMap<String, ExtensionStorage>,
    files: HashMap<String, HashMap<String, String>>,
}

impl ExtensionManager {
    pub fn new_in_memory() -> Self {
        Self::new_in_memory_with_trust(PublisherTrustStore::new())
    }

    pub fn new_in_memory_with_trust(trust: PublisherTrustStore) -> Self {
        Self {
            profile_dir: None,
            trust,
            extensions: HashMap::new(),
            storages: HashMap::new(),
            files: HashMap::new(),
        }
    }

    pub fn new_with_profile(profile_dir: &Path) -> Result<Self, ExtensionError> {
        Self::new_with_profile_and_trust(profile_dir, PublisherTrustStore::new())
    }

    pub fn new_with_profile_and_trust(
        profile_dir: &Path,
        mut trust: PublisherTrustStore,
    ) -> Result<Self, ExtensionError> {
        let profile_dir = absolute_path(profile_dir)?;
        let extensions_dir = profile_dir.join("extensions");
        fs::create_dir_all(&extensions_dir).map_err(storage_error)?;
        let trust_path = extensions_dir.join("trusted_publishers.json");
        if trust_path.exists() {
            let persisted: std::collections::BTreeMap<String, [u8; 32]> =
                read_bounded_json(&trust_path, 64 * 1024)?;
            let persisted = PublisherTrustStore::import_ed25519(persisted)?;
            for (key_id, key) in persisted.export_ed25519() {
                trust.insert_ed25519(key_id, key)?;
            }
        }
        let mut manager = Self {
            profile_dir: Some(profile_dir),
            trust,
            extensions: HashMap::new(),
            storages: HashMap::new(),
            files: HashMap::new(),
        };
        manager.load_installed()?;
        Ok(manager)
    }

    pub fn trust_publisher(
        &mut self,
        key_id: impl Into<String>,
        public_key: [u8; 32],
    ) -> Result<(), ExtensionError> {
        self.trust.insert_ed25519(key_id, public_key)?;
        if let Some(profile_dir) = &self.profile_dir {
            atomic_json_write(
                &profile_dir
                    .join("extensions")
                    .join("trusted_publishers.json"),
                &self.trust.export_ed25519(),
            )?;
        }
        Ok(())
    }

    pub fn list_extensions(&self) -> Vec<&ExtensionRecord> {
        self.extensions.values().collect()
    }

    pub fn get_extension(&self, id: &str) -> Option<&ExtensionRecord> {
        self.extensions.get(id)
    }

    pub fn review_package(
        &self,
        package: &ExtensionPackage,
    ) -> Result<ExtensionPermissionReview, ExtensionError> {
        package.verify_signature(&self.trust)?;
        let content_script_patterns = package
            .manifest
            .content_scripts
            .iter()
            .flat_map(|script| script.matches.iter().cloned())
            .collect();
        Ok(ExtensionPermissionReview {
            extension_id: package.manifest.id.clone(),
            publisher_key_id: package.manifest.publisher_key_id.clone(),
            package_digest: package.compute_digest()?,
            requested_permissions: package.manifest.permissions.iter().cloned().collect(),
            requested_network_origins: package.manifest.network_origins.iter().cloned().collect(),
            content_script_patterns,
        })
    }

    pub fn install_reviewed_package(
        &mut self,
        package: ExtensionPackage,
        approval: ExtensionApproval,
    ) -> Result<String, ExtensionError> {
        let review = self.review_package(&package)?;
        if !approval.user_confirmed || approval.package_digest != review.package_digest {
            return Err(ExtensionError::ReviewRequired(
                "approval is absent or belongs to different package bytes".into(),
            ));
        }
        if !approval
            .approved_permissions
            .is_subset(&review.requested_permissions)
            || !approval
                .approved_network_origins
                .is_subset(&review.requested_network_origins)
        {
            return Err(ExtensionError::PermissionDenied(
                "approval contains an undeclared capability".into(),
            ));
        }
        if !approval.approved_network_origins.is_empty()
            && !approval
                .approved_permissions
                .contains(&ExtensionPermission::Network)
        {
            return Err(ExtensionError::PermissionDenied(
                "network origins cannot be approved without network capability".into(),
            ));
        }

        let id = package.manifest.id.clone();
        if self.extensions.contains_key(&id) {
            return Err(ExtensionError::AlreadyInstalled(id));
        }
        let record = ExtensionRecord {
            manifest: package.manifest,
            status: ExtensionStatus::Enabled,
            granted_permissions: approval.approved_permissions.into_iter().collect(),
            granted_network_origins: approval.approved_network_origins,
            package_digest: review.package_digest,
            installed_at: unix_seconds(),
        };
        self.extensions.insert(id.clone(), record);
        self.storages
            .insert(id.clone(), ExtensionStorage::default());
        self.files.insert(id.clone(), package.files);
        if let Err(error) = self.persist_extension(&id) {
            self.extensions.remove(&id);
            self.storages.remove(&id);
            self.files.remove(&id);
            return Err(error);
        }
        Ok(id)
    }

    pub fn set_status(&mut self, id: &str, status: ExtensionStatus) -> Result<(), ExtensionError> {
        let record = self
            .extensions
            .get_mut(id)
            .ok_or_else(|| ExtensionError::NotFound(id.to_string()))?;
        record.status = status;
        self.persist_extension(id)
    }

    pub fn revoke_permission(
        &mut self,
        id: &str,
        permission: &ExtensionPermission,
    ) -> Result<(), ExtensionError> {
        let record = self
            .extensions
            .get_mut(id)
            .ok_or_else(|| ExtensionError::NotFound(id.to_string()))?;
        record.granted_permissions.remove(permission);
        if permission == &ExtensionPermission::Network {
            record.granted_network_origins.clear();
        }
        if record.granted_permissions.is_empty() {
            record.status = ExtensionStatus::Revoked;
        }
        self.persist_extension(id)
    }

    pub fn authorize_network_request(&self, id: &str, target: &str) -> Result<(), ExtensionError> {
        let record = self
            .extensions
            .get(id)
            .ok_or_else(|| ExtensionError::NotFound(id.to_string()))?;
        if record.status != ExtensionStatus::Enabled
            || !record
                .granted_permissions
                .contains(&ExtensionPermission::Network)
        {
            return Err(ExtensionError::PermissionDenied(
                "network capability is not active".into(),
            ));
        }
        let allowed = record.granted_network_origins.iter().any(|pattern| {
            ParsedMatchPattern::parse(pattern)
                .map(|parsed| parsed.matches(target))
                .unwrap_or(false)
        });
        if allowed {
            Ok(())
        } else {
            Err(ExtensionError::PermissionDenied(format!(
                "target origin was not approved: {target}"
            )))
        }
    }

    pub fn storage_get(&self, id: &str, key: &str) -> Result<Option<&String>, ExtensionError> {
        self.require_permission(id, &ExtensionPermission::Storage)?;
        Ok(self.storages.get(id).and_then(|storage| storage.get(key)))
    }

    pub fn storage_set(
        &mut self,
        id: &str,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), ExtensionError> {
        self.require_permission(id, &ExtensionPermission::Storage)?;
        self.storages
            .get_mut(id)
            .ok_or_else(|| ExtensionError::NotFound(id.to_string()))?
            .set(key, value)?;
        self.persist_extension(id)
    }

    pub fn execute_background(
        &mut self,
        id: &str,
        engine: &mut crate::javascript::JsvEngine,
    ) -> Result<String, ExtensionError> {
        let record = self
            .extensions
            .get(id)
            .ok_or_else(|| ExtensionError::NotFound(id.to_string()))?;
        if record.status != ExtensionStatus::Enabled {
            return Err(ExtensionError::PermissionDenied(
                "extension is not enabled".into(),
            ));
        }
        let script_path = record.manifest.background_script.as_ref().ok_or_else(|| {
            ExtensionError::ExecutionFailed("no background script declared".into())
        })?;
        let code = self
            .files
            .get(id)
            .and_then(|files| files.get(script_path))
            .ok_or_else(|| ExtensionError::ExecutionFailed("background script is missing".into()))?
            .clone();
        let permissions = record.granted_permissions.iter().cloned().collect();
        let mut worker = ExtensionWorker::new(id, permissions);
        worker.storage = self.storages.get(id).cloned().unwrap_or_default();
        let result = worker.execute_script(script_path, &code, engine)?;
        self.storages.insert(id.to_string(), worker.storage);
        self.persist_extension(id)?;
        Ok(result)
    }

    pub fn get_content_scripts_for_url(&self, url: &str) -> Vec<(String, String, String)> {
        let mut scripts = Vec::new();
        for (id, record) in &self.extensions {
            if record.status != ExtensionStatus::Enabled
                || !record
                    .granted_permissions
                    .contains(&ExtensionPermission::ContentScript)
            {
                continue;
            }
            for config in &record.manifest.content_scripts {
                if config.matches_url(url) {
                    if let Some(code) = self
                        .files
                        .get(id)
                        .and_then(|files| files.get(&config.script_path))
                    {
                        scripts.push((id.clone(), config.script_path.clone(), code.clone()));
                    }
                }
            }
        }
        scripts
    }

    pub fn execute_content_scripts(
        &self,
        url: &str,
        runtime: &mut crate::web_runtime::PageRuntime,
    ) -> Vec<(String, String, Result<String, String>)> {
        self.get_content_scripts_for_url(url)
            .into_iter()
            .map(|(id, path, code)| {
                let result = runtime
                    .execute_script(&code)
                    .map(|value| value.to_display_string());
                (id, path, result)
            })
            .collect()
    }

    pub fn uninstall_extension(&mut self, id: &str) -> Result<(), ExtensionError> {
        validate_component_id(id, "extension id")?;
        if !self.extensions.contains_key(id) {
            return Err(ExtensionError::NotFound(id.to_string()));
        }
        if let Some(profile_dir) = &self.profile_dir {
            let path = profile_dir.join("extensions").join(id);
            if path.exists() {
                fs::remove_dir_all(&path).map_err(storage_error)?;
            }
        }
        self.extensions.remove(id);
        self.storages.remove(id);
        self.files.remove(id);
        Ok(())
    }

    fn require_permission(
        &self,
        id: &str,
        permission: &ExtensionPermission,
    ) -> Result<(), ExtensionError> {
        let record = self
            .extensions
            .get(id)
            .ok_or_else(|| ExtensionError::NotFound(id.to_string()))?;
        if record.status == ExtensionStatus::Enabled
            && record.granted_permissions.contains(permission)
        {
            Ok(())
        } else {
            Err(ExtensionError::PermissionDenied(format!(
                "extension {id} lacks active {permission:?} capability"
            )))
        }
    }

    fn persist_extension(&self, id: &str) -> Result<(), ExtensionError> {
        let Some(profile_dir) = &self.profile_dir else {
            return Ok(());
        };
        validate_component_id(id, "extension id")?;
        let directory = profile_dir.join("extensions").join(id);
        fs::create_dir_all(&directory).map_err(storage_error)?;
        let record = self
            .extensions
            .get(id)
            .ok_or_else(|| ExtensionError::NotFound(id.to_string()))?;
        let storage = self
            .storages
            .get(id)
            .ok_or_else(|| ExtensionError::StorageError("storage record is missing".into()))?;
        let files = self
            .files
            .get(id)
            .ok_or_else(|| ExtensionError::StorageError("package files are missing".into()))?;
        atomic_json_write(&directory.join("record.json"), record)?;
        atomic_json_write(&directory.join("storage.json"), storage)?;
        atomic_json_write(&directory.join("files.json"), files)?;
        Ok(())
    }

    fn load_installed(&mut self) -> Result<(), ExtensionError> {
        let Some(profile_dir) = &self.profile_dir else {
            return Ok(());
        };
        let root = profile_dir.join("extensions");
        for entry in fs::read_dir(&root).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            let file_type = entry.file_type().map_err(storage_error)?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let directory_name = entry.file_name().to_string_lossy().into_owned();
            validate_component_id(&directory_name, "extension directory")?;
            let record: ExtensionRecord =
                read_bounded_json(&entry.path().join("record.json"), 256 * 1024)?;
            let storage: ExtensionStorage = read_bounded_json(
                &entry.path().join("storage.json"),
                MAX_EXTENSION_STORAGE_BYTES + 512 * 1024,
            )?;
            let files: HashMap<String, String> = read_bounded_json(
                &entry.path().join("files.json"),
                MAX_EXTENSION_PACKAGE_BYTES + 512 * 1024,
            )?;
            if record.manifest.id != directory_name {
                return Err(ExtensionError::StorageError(
                    "extension record id does not match its directory".into(),
                ));
            }
            let package = ExtensionPackage {
                manifest: record.manifest.clone(),
                files: files.clone(),
            };
            package.verify_signature(&self.trust)?;
            if package.compute_digest()? != record.package_digest {
                return Err(ExtensionError::InvalidSignature(
                    "persisted extension digest does not match signed package".into(),
                ));
            }
            self.extensions.insert(directory_name.clone(), record);
            self.storages.insert(directory_name.clone(), storage);
            self.files.insert(directory_name, files);
        }
        Ok(())
    }
}

fn validate_component_id(value: &str, field: &str) -> Result<(), ExtensionError> {
    if value.is_empty()
        || value.len() > 96
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || value == "."
        || value == ".."
    {
        return Err(ExtensionError::InvalidManifest(format!(
            "{field} is not a safe bounded identifier"
        )));
    }
    Ok(())
}

fn validate_text(
    value: &str,
    minimum: usize,
    maximum: usize,
    field: &str,
) -> Result<(), ExtensionError> {
    let length = value.len();
    if length < minimum || length > maximum || value.chars().any(char::is_control) {
        return Err(ExtensionError::InvalidManifest(format!(
            "{field} length/content is invalid"
        )));
    }
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn storage_error(error: std::io::Error) -> ExtensionError {
    ExtensionError::StorageError(error.to_string())
}

fn absolute_path(path: &Path) -> Result<PathBuf, ExtensionError> {
    if path.as_os_str().is_empty() {
        return Err(ExtensionError::StorageError("profile path is empty".into()));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(storage_error)
    }
}

fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<(), ExtensionError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| ExtensionError::StorageError(error.to_string()))?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes).map_err(storage_error)?;
    if path.exists() {
        fs::remove_file(path).map_err(storage_error)?;
    }
    fs::rename(&temporary, path).map_err(storage_error)
}

fn read_bounded_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    maximum_bytes: usize,
) -> Result<T, ExtensionError> {
    let metadata = fs::metadata(path).map_err(storage_error)?;
    if metadata.len() > maximum_bytes as u64 {
        return Err(ExtensionError::ResourceLimit(format!(
            "persisted file exceeds {maximum_bytes} bytes: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(storage_error)?;
    serde_json::from_slice(&bytes).map_err(|error| ExtensionError::StorageError(error.to_string()))
}

//! Installed web applications with explicit review and origin-scoped data.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_APP_ICONS: usize = 16;
const MAX_APP_PERMISSIONS: usize = 32;
const MAX_APP_RECORD_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    InvalidManifest(String),
    PermissionDenied(String),
    NotFound(String),
    StorageError(String),
    AlreadyInstalled(String),
    LaunchFailed(String),
    ReviewRequired(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidManifest(message) => write!(f, "invalid app manifest: {message}"),
            Self::PermissionDenied(message) => write!(f, "permission denied: {message}"),
            Self::NotFound(message) => write!(f, "app not found: {message}"),
            Self::StorageError(message) => write!(f, "storage error: {message}"),
            Self::AlreadyInstalled(message) => write!(f, "app already installed: {message}"),
            Self::LaunchFailed(message) => write!(f, "failed to launch app: {message}"),
            Self::ReviewRequired(message) => write!(f, "installation review required: {message}"),
        }
    }
}

impl std::error::Error for AppError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDisplayMode {
    #[default]
    Standalone,
    MinimalUi,
    Browser,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppIconConfig {
    pub src: String,
    pub sizes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledAppManifest {
    pub id: String,
    pub name: String,
    pub start_url: String,
    pub scope_url: String,
    pub display_mode: AppDisplayMode,
    pub icons: Vec<AppIconConfig>,
    pub permissions: Vec<String>,
}

impl InstalledAppManifest {
    pub fn validate(&self) -> Result<(), AppError> {
        validate_app_id(&self.id)?;
        if self.name.is_empty() || self.name.len() > 128 || self.name.chars().any(char::is_control)
        {
            return Err(AppError::InvalidManifest(
                "name must contain 1..=128 safe characters".into(),
            ));
        }
        let start = secure_http_url(&self.start_url, "start_url")?;
        let scope = secure_http_url(&self.scope_url, "scope_url")?;
        if origin_tuple(&start) != origin_tuple(&scope) {
            return Err(AppError::InvalidManifest(
                "start_url and scope_url must share an origin".into(),
            ));
        }
        if !start.path().starts_with(scope.path()) {
            return Err(AppError::InvalidManifest(
                "start_url must be contained by scope_url".into(),
            ));
        }
        if self.icons.len() > MAX_APP_ICONS {
            return Err(AppError::InvalidManifest(format!(
                "at most {MAX_APP_ICONS} icons may be declared"
            )));
        }
        for icon in &self.icons {
            if icon.src.is_empty() || icon.src.len() > 2_048 || icon.sizes.len() > 64 {
                return Err(AppError::InvalidManifest("invalid icon metadata".into()));
            }
            let resolved = scope
                .join(&icon.src)
                .map_err(|_| AppError::InvalidManifest("invalid icon URL".into()))?;
            if origin_tuple(&resolved) != origin_tuple(&scope) {
                return Err(AppError::InvalidManifest(
                    "app icons must remain on the app origin".into(),
                ));
            }
        }
        if self.permissions.len() > MAX_APP_PERMISSIONS {
            return Err(AppError::InvalidManifest(format!(
                "at most {MAX_APP_PERMISSIONS} permissions may be declared"
            )));
        }
        let mut unique = BTreeSet::new();
        for permission in &self.permissions {
            if permission.is_empty()
                || permission.len() > 64
                || !permission
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
                || !unique.insert(permission)
            {
                return Err(AppError::InvalidManifest(
                    "app permissions must be unique bounded capability identifiers".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn origin(&self) -> Result<String, AppError> {
        let url = secure_http_url(&self.scope_url, "scope_url")?;
        Ok(url.origin().ascii_serialization())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAppReview {
    pub app_id: String,
    pub name: String,
    pub origin: String,
    pub scope_url: String,
    pub requested_permissions: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAppApproval {
    pub app_id: String,
    pub origin: String,
    pub approved_permissions: BTreeSet<String>,
    pub user_confirmed: bool,
}

impl InstalledAppApproval {
    pub fn approve_all(review: &InstalledAppReview) -> Self {
        Self {
            app_id: review.app_id.clone(),
            origin: review.origin.clone(),
            approved_permissions: review.requested_permissions.clone(),
            user_confirmed: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledAppShortcut {
    pub app_id: String,
    pub name: String,
    pub arguments: Vec<String>,
    pub icon_url: Option<String>,
    pub start_url: String,
}

#[derive(Debug, Clone)]
pub struct InstalledAppWindow {
    pub app_id: String,
    pub window_id: u64,
    pub title: String,
    pub current_url: String,
    pub is_active: bool,
    pub sw_scope: String,
    pub storage_partition: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledAppRecord {
    pub manifest: InstalledAppManifest,
    pub origin: String,
    pub approved_permissions: BTreeSet<String>,
    pub installed_at: u64,
    pub shortcut: InstalledAppShortcut,
}

pub struct InstalledAppManager {
    profile_dir: Option<PathBuf>,
    apps: HashMap<String, InstalledAppRecord>,
    active_windows: HashMap<u64, InstalledAppWindow>,
    next_window_id: u64,
}

impl InstalledAppManager {
    pub fn new_in_memory() -> Self {
        Self {
            profile_dir: None,
            apps: HashMap::new(),
            active_windows: HashMap::new(),
            next_window_id: 1,
        }
    }

    pub fn new_with_profile(profile_dir: &Path) -> Result<Self, AppError> {
        let profile_dir = absolute_path(profile_dir)?;
        fs::create_dir_all(profile_dir.join("apps")).map_err(storage_error)?;
        let mut manager = Self {
            profile_dir: Some(profile_dir),
            apps: HashMap::new(),
            active_windows: HashMap::new(),
            next_window_id: 1,
        };
        manager.load_installed()?;
        Ok(manager)
    }

    pub fn list_apps(&self) -> Vec<&InstalledAppRecord> {
        self.apps.values().collect()
    }

    pub fn get_app(&self, app_id: &str) -> Option<&InstalledAppRecord> {
        self.apps.get(app_id)
    }

    pub fn review_manifest(
        &self,
        manifest: &InstalledAppManifest,
        source_document_url: &str,
    ) -> Result<InstalledAppReview, AppError> {
        manifest.validate()?;
        let source = secure_http_url(source_document_url, "source document URL")?;
        let origin = manifest.origin()?;
        if source.origin().ascii_serialization() != origin {
            return Err(AppError::PermissionDenied(
                "an app can only be installed by a same-origin document".into(),
            ));
        }
        Ok(InstalledAppReview {
            app_id: manifest.id.clone(),
            name: manifest.name.clone(),
            origin,
            scope_url: manifest.scope_url.clone(),
            requested_permissions: manifest.permissions.iter().cloned().collect(),
        })
    }

    pub fn install_reviewed_app(
        &mut self,
        manifest: InstalledAppManifest,
        source_document_url: &str,
        approval: InstalledAppApproval,
    ) -> Result<String, AppError> {
        let review = self.review_manifest(&manifest, source_document_url)?;
        if !approval.user_confirmed
            || approval.app_id != review.app_id
            || approval.origin != review.origin
        {
            return Err(AppError::ReviewRequired(
                "approval does not bind the reviewed app and origin".into(),
            ));
        }
        if !approval
            .approved_permissions
            .is_subset(&review.requested_permissions)
        {
            return Err(AppError::PermissionDenied(
                "approval includes an undeclared app capability".into(),
            ));
        }
        let id = manifest.id.clone();
        if self.apps.contains_key(&id) {
            return Err(AppError::AlreadyInstalled(id));
        }
        let shortcut = InstalledAppShortcut {
            app_id: id.clone(),
            name: manifest.name.clone(),
            arguments: vec![format!("--app={id}")],
            icon_url: manifest.icons.first().map(|icon| icon.src.clone()),
            start_url: manifest.start_url.clone(),
        };
        let record = InstalledAppRecord {
            manifest,
            origin: review.origin,
            approved_permissions: approval.approved_permissions,
            installed_at: unix_seconds(),
            shortcut,
        };
        self.apps.insert(id.clone(), record);
        if let Err(error) = self.persist_app(&id) {
            self.apps.remove(&id);
            return Err(error);
        }
        Ok(id)
    }

    pub fn launch_app(&mut self, app_id: &str) -> Result<InstalledAppWindow, AppError> {
        validate_app_id(app_id)?;
        let record = self
            .apps
            .get(app_id)
            .ok_or_else(|| AppError::NotFound(app_id.to_string()))?;
        let window_id = self.next_window_id;
        self.next_window_id = self
            .next_window_id
            .checked_add(1)
            .ok_or_else(|| AppError::LaunchFailed("window identifier exhausted".into()))?;
        let window = InstalledAppWindow {
            app_id: app_id.to_string(),
            window_id,
            title: record.manifest.name.clone(),
            current_url: record.manifest.start_url.clone(),
            is_active: true,
            sw_scope: record.manifest.scope_url.clone(),
            storage_partition: self.app_data_dir(app_id),
        };
        self.active_windows.insert(window_id, window.clone());
        Ok(window)
    }

    /// Construct a PageRuntime whose IndexedDB, Cache API and ServiceWorker
    /// data live exclusively under this installed application's partition.
    pub fn create_runtime(
        &self,
        app_id: &str,
        html: &str,
        viewport_width: u32,
    ) -> Result<crate::web_runtime::PageRuntime, AppError> {
        let record = self
            .apps
            .get(app_id)
            .ok_or_else(|| AppError::NotFound(app_id.to_string()))?;
        crate::web_runtime::PageRuntime::from_html_with_storage_dir(
            html,
            Vec::new(),
            viewport_width,
            &record.manifest.start_url,
            self.app_data_dir(app_id),
        )
        .map_err(AppError::LaunchFailed)
    }

    pub fn close_window(&mut self, window_id: u64) -> Option<InstalledAppWindow> {
        self.active_windows.remove(&window_id)
    }

    pub fn active_windows_count(&self) -> usize {
        self.active_windows.len()
    }

    pub fn uninstall_app(&mut self, app_id: &str) -> Result<(), AppError> {
        validate_app_id(app_id)?;
        if !self.apps.contains_key(app_id) {
            return Err(AppError::NotFound(app_id.to_string()));
        }
        if let Some(profile_dir) = &self.profile_dir {
            let app_path = profile_dir.join("apps").join(app_id);
            if app_path.exists() {
                fs::remove_dir_all(&app_path).map_err(storage_error)?;
            }
        }
        self.apps.remove(app_id);
        self.active_windows
            .retain(|_, window| window.app_id != app_id);
        Ok(())
    }

    fn app_data_dir(&self, app_id: &str) -> Option<PathBuf> {
        self.profile_dir
            .as_ref()
            .map(|profile| profile.join("apps").join(app_id).join("web-platform"))
    }

    fn persist_app(&self, app_id: &str) -> Result<(), AppError> {
        let Some(profile_dir) = &self.profile_dir else {
            return Ok(());
        };
        validate_app_id(app_id)?;
        let app_dir = profile_dir.join("apps").join(app_id);
        fs::create_dir_all(app_dir.join("web-platform")).map_err(storage_error)?;
        let record = self
            .apps
            .get(app_id)
            .ok_or_else(|| AppError::NotFound(app_id.to_string()))?;
        atomic_json_write(&app_dir.join("record.json"), record)?;
        atomic_json_write(&app_dir.join("shortcut.json"), &record.shortcut)?;
        Ok(())
    }

    fn load_installed(&mut self) -> Result<(), AppError> {
        let Some(profile_dir) = &self.profile_dir else {
            return Ok(());
        };
        for entry in fs::read_dir(profile_dir.join("apps")).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            let file_type = entry.file_type().map_err(storage_error)?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let directory_name = entry.file_name().to_string_lossy().into_owned();
            validate_app_id(&directory_name)?;
            let record_path = entry.path().join("record.json");
            let metadata = fs::metadata(&record_path).map_err(storage_error)?;
            if metadata.len() > MAX_APP_RECORD_BYTES as u64 {
                return Err(AppError::StorageError(
                    "persisted app record exceeds byte limit".into(),
                ));
            }
            let bytes = fs::read(record_path).map_err(storage_error)?;
            let record: InstalledAppRecord = serde_json::from_slice(&bytes)
                .map_err(|error| AppError::StorageError(error.to_string()))?;
            record.manifest.validate()?;
            if record.manifest.id != directory_name || record.origin != record.manifest.origin()? {
                return Err(AppError::StorageError(
                    "persisted app identity/origin does not match its partition".into(),
                ));
            }
            fs::create_dir_all(entry.path().join("web-platform")).map_err(storage_error)?;
            self.apps.insert(directory_name, record);
        }
        Ok(())
    }
}

fn validate_app_id(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 96
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AppError::InvalidManifest(
            "app id is not a safe bounded identifier".into(),
        ));
    }
    Ok(())
}

fn secure_http_url(value: &str, field: &str) -> Result<url::Url, AppError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| AppError::InvalidManifest(format!("{field} is not a valid URL")))?;
    if !matches!(parsed.scheme(), "https" | "http") || parsed.host_str().is_none() {
        return Err(AppError::InvalidManifest(format!(
            "{field} must be an absolute HTTP(S) URL"
        )));
    }
    if parsed.scheme() == "http"
        && !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    {
        return Err(AppError::InvalidManifest(format!(
            "{field} must use HTTPS outside loopback"
        )));
    }
    Ok(parsed)
}

fn origin_tuple(url: &url::Url) -> (String, String, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().unwrap_or_default().to_ascii_lowercase(),
        url.port_or_known_default(),
    )
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn storage_error(error: std::io::Error) -> AppError {
    AppError::StorageError(error.to_string())
}

fn absolute_path(path: &Path) -> Result<PathBuf, AppError> {
    if path.as_os_str().is_empty() {
        return Err(AppError::StorageError("profile path is empty".into()));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(storage_error)
    }
}

fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| AppError::StorageError(error.to_string()))?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes).map_err(storage_error)?;
    if path.exists() {
        fs::remove_file(path).map_err(storage_error)?;
    }
    fs::rename(temporary, path).map_err(storage_error)
}

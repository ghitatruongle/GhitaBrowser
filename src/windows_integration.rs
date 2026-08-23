//! Per-user Windows integration and consent-owned notification state.
//!
//! Registry mutation is explicit, reversible and gated by Authenticode
//! verification of the executable being registered. GhitaBrowser registers
//! capabilities but never writes protected `UserChoice` hashes; Windows keeps
//! the final default-browser choice under user control.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_NOTIFICATIONS: usize = 100;
const MAX_NOTIFICATION_TEXT_BYTES: usize = 4_096;
const MAX_PERSISTED_STATE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashReportConsent {
    Granted,
    Denied,
    #[default]
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAssociation {
    pub extension: String,
    pub prog_id: String,
    pub icon_index: i32,
    pub content_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolHandler {
    pub scheme: String,
    pub prog_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValuePlan {
    pub subkey: String,
    pub value_name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserNotification {
    pub id: u64,
    pub title: String,
    pub message: String,
    pub category: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    OpenUrl(String),
    LaunchApp(String),
    Update,
    Rollback,
    Repair,
    Uninstall,
    NoAction,
}

pub struct WindowsIntegration {
    profile_dir: Option<PathBuf>,
    pub file_associations: Vec<FileAssociation>,
    pub protocol_handlers: Vec<ProtocolHandler>,
    pub crash_consent: CrashReportConsent,
    notifications: VecDeque<BrowserNotification>,
    next_notification_id: u64,
}

impl WindowsIntegration {
    pub fn new_in_memory() -> Self {
        let mut manager = Self {
            profile_dir: None,
            file_associations: Vec::new(),
            protocol_handlers: Vec::new(),
            crash_consent: CrashReportConsent::Prompt,
            notifications: VecDeque::new(),
            next_notification_id: 1,
        };
        manager.init_default_associations();
        manager
    }

    pub fn new_with_profile(profile_dir: &Path) -> Result<Self, String> {
        let profile_dir = absolute_path(profile_dir)?;
        let mut manager = Self {
            profile_dir: Some(profile_dir),
            file_associations: Vec::new(),
            protocol_handlers: Vec::new(),
            crash_consent: CrashReportConsent::Prompt,
            notifications: VecDeque::new(),
            next_notification_id: 1,
        };
        manager.init_default_associations();
        manager.load_state()?;
        Ok(manager)
    }

    fn init_default_associations(&mut self) {
        self.file_associations = vec![
            FileAssociation {
                extension: ".html".into(),
                prog_id: "GhitaBrowser.HTML".into(),
                icon_index: 0,
                content_type: "text/html".into(),
            },
            FileAssociation {
                extension: ".htm".into(),
                prog_id: "GhitaBrowser.HTML".into(),
                icon_index: 0,
                content_type: "text/html".into(),
            },
            FileAssociation {
                extension: ".pdf".into(),
                prog_id: "GhitaBrowser.PDF".into(),
                icon_index: 1,
                content_type: "application/pdf".into(),
            },
            FileAssociation {
                extension: ".txt".into(),
                prog_id: "GhitaBrowser.Text".into(),
                icon_index: 2,
                content_type: "text/plain".into(),
            },
        ];
        self.protocol_handlers = vec![
            ProtocolHandler {
                scheme: "http".into(),
                prog_id: "GhitaBrowser.HTTP".into(),
            },
            ProtocolHandler {
                scheme: "https".into(),
                prog_id: "GhitaBrowser.HTTPS".into(),
            },
        ];
    }

    pub fn parse_cli_args(args: &[String]) -> Result<CliAction, String> {
        let mut action: Option<CliAction> = None;
        for argument in args.iter().skip(1) {
            let candidate = if let Some(app_id) = argument.strip_prefix("--app=") {
                validate_component_id(app_id, "app id")?;
                CliAction::LaunchApp(app_id.to_string())
            } else if argument == "--update" {
                CliAction::Update
            } else if argument == "--rollback" {
                CliAction::Rollback
            } else if argument == "--repair" {
                CliAction::Repair
            } else if argument == "--uninstall" {
                CliAction::Uninstall
            } else if !argument.starts_with('-') {
                if argument.len() > 32 * 1024 {
                    return Err("activation argument exceeds 32 KiB".into());
                }
                if let Ok(url) = url::Url::parse(argument) {
                    if !matches!(url.scheme(), "http" | "https" | "file") {
                        return Err("activation URL uses an unsupported scheme".into());
                    }
                }
                CliAction::OpenUrl(argument.clone())
            } else {
                return Err(format!("unknown activation option: {argument}"));
            };
            if action.replace(candidate).is_some() {
                return Err("multiple activation actions are not allowed".into());
            }
        }
        Ok(action.unwrap_or(CliAction::NoAction))
    }

    /// Produce the complete per-user HKCU registration plan. This method is
    /// pure and is used for review/tests before any Windows state is changed.
    pub fn registration_plan(&self, executable: &Path) -> Result<Vec<RegistryValuePlan>, String> {
        let executable = absolute_path(executable)?;
        let command = format!("\"{}\" \"%1\"", executable.display());
        let icon = format!("{},0", executable.display());
        let mut plan = Vec::new();
        let clients = "Software\\Clients\\StartMenuInternet\\GhitaBrowser";
        plan.push(registry_value(clients, "", "GhitaBrowser"));
        plan.push(registry_value(
            &format!("{clients}\\Capabilities"),
            "ApplicationName",
            "GhitaBrowser",
        ));
        plan.push(registry_value(
            &format!("{clients}\\Capabilities"),
            "ApplicationDescription",
            "Independent GhitaBrowser web browser",
        ));
        for association in &self.file_associations {
            plan.push(registry_value(
                &format!("{clients}\\Capabilities\\FileAssociations"),
                &association.extension,
                &association.prog_id,
            ));
            let class_key = format!("Software\\Classes\\{}", association.prog_id);
            plan.push(registry_value(&class_key, "", "GhitaBrowser document"));
            plan.push(registry_value(
                &format!("{class_key}\\DefaultIcon"),
                "",
                &format!("{},{}", executable.display(), association.icon_index),
            ));
            plan.push(registry_value(
                &format!("{class_key}\\shell\\open\\command"),
                "",
                &command,
            ));
        }
        for protocol in &self.protocol_handlers {
            plan.push(registry_value(
                &format!("{clients}\\Capabilities\\URLAssociations"),
                &protocol.scheme,
                &protocol.prog_id,
            ));
            let class_key = format!("Software\\Classes\\{}", protocol.prog_id);
            plan.push(registry_value(&class_key, "", "URL:GhitaBrowser Protocol"));
            plan.push(registry_value(&class_key, "URL Protocol", ""));
            plan.push(registry_value(
                &format!("{class_key}\\DefaultIcon"),
                "",
                &icon,
            ));
            plan.push(registry_value(
                &format!("{class_key}\\shell\\open\\command"),
                "",
                &command,
            ));
        }
        plan.push(registry_value(
            "Software\\RegisteredApplications",
            "GhitaBrowser",
            &format!("{clients}\\Capabilities"),
        ));
        Ok(plan)
    }

    /// Register the signed executable for the current user. This never chooses
    /// defaults on the user's behalf and never requires administrator rights.
    #[cfg(target_os = "windows")]
    pub fn register_for_current_user(
        &self,
        executable: &Path,
        expected_sha256: &str,
    ) -> Result<usize, String> {
        let actual_sha256 = crate::acceptance::AcceptanceAuditor::sha256_file(executable)?;
        if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
            return Err("registered executable does not match the signed update manifest".into());
        }
        verify_authenticode(executable)?;
        let plan = self.registration_plan(executable)?;
        for value in &plan {
            if let Err(error) = set_hkcu_string(&value.subkey, &value.value_name, &value.value) {
                let _ = self.unregister_for_current_user();
                return Err(error);
            }
        }
        notify_association_change();
        Ok(plan.len())
    }

    #[cfg(not(target_os = "windows"))]
    pub fn register_for_current_user(
        &self,
        _executable: &Path,
        _expected_sha256: &str,
    ) -> Result<usize, String> {
        Err("Windows integration is only available on Windows".into())
    }

    #[cfg(target_os = "windows")]
    pub fn unregister_for_current_user(&self) -> Result<(), String> {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};

        for path in [
            "Software\\Clients\\StartMenuInternet\\GhitaBrowser",
            "Software\\Classes\\GhitaBrowser.HTML",
            "Software\\Classes\\GhitaBrowser.PDF",
            "Software\\Classes\\GhitaBrowser.Text",
            "Software\\Classes\\GhitaBrowser.HTTP",
            "Software\\Classes\\GhitaBrowser.HTTPS",
        ] {
            let wide = wide_null(path);
            // Missing keys are harmless during idempotent uninstall.
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr())) };
        }
        // Remove the named RegisteredApplications value without deleting any
        // value owned by another product.
        delete_hkcu_value("Software\\RegisteredApplications", "GhitaBrowser")?;
        notify_association_change();
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    pub fn unregister_for_current_user(&self) -> Result<(), String> {
        Err("Windows integration is only available on Windows".into())
    }

    pub fn set_crash_consent(&mut self, consent: CrashReportConsent) -> Result<(), String> {
        self.crash_consent = consent;
        self.persist_state()
    }

    pub fn crash_upload_allowed(&self) -> bool {
        self.crash_consent == CrashReportConsent::Granted
    }

    pub fn push_notification(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        category: impl Into<String>,
    ) -> Result<u64, String> {
        let title = title.into();
        let message = message.into();
        let category = category.into();
        if title.is_empty()
            || title.len() + message.len() + category.len() > MAX_NOTIFICATION_TEXT_BYTES
            || category.len() > 64
        {
            return Err("notification text exceeds its safety budget".into());
        }
        let id = self.next_notification_id;
        self.next_notification_id = self
            .next_notification_id
            .checked_add(1)
            .ok_or_else(|| "notification identifier exhausted".to_string())?;
        self.notifications.push_back(BrowserNotification {
            id,
            title,
            message,
            category,
            created_at: unix_seconds(),
        });
        while self.notifications.len() > MAX_NOTIFICATIONS {
            self.notifications.pop_front();
        }
        self.persist_state()?;
        Ok(id)
    }

    pub fn list_notifications(&self) -> Vec<&BrowserNotification> {
        self.notifications.iter().collect()
    }

    pub fn clear_notifications(&mut self) -> Result<(), String> {
        self.notifications.clear();
        self.persist_state()
    }

    fn persist_state(&self) -> Result<(), String> {
        let Some(profile_dir) = &self.profile_dir else {
            return Ok(());
        };
        let directory = profile_dir.join("windows");
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        atomic_json_write(&directory.join("crash_consent.json"), &self.crash_consent)?;
        atomic_json_write(&directory.join("notifications.json"), &self.notifications)?;
        Ok(())
    }

    fn load_state(&mut self) -> Result<(), String> {
        let Some(profile_dir) = &self.profile_dir else {
            return Ok(());
        };
        let directory = profile_dir.join("windows");
        let consent = directory.join("crash_consent.json");
        if consent.exists() {
            self.crash_consent = read_bounded_json(&consent)?;
        }
        let notifications = directory.join("notifications.json");
        if notifications.exists() {
            let list: VecDeque<BrowserNotification> = read_bounded_json(&notifications)?;
            if list.len() > MAX_NOTIFICATIONS
                || list.iter().any(|notification| {
                    notification.title.len()
                        + notification.message.len()
                        + notification.category.len()
                        > MAX_NOTIFICATION_TEXT_BYTES
                })
            {
                return Err("persisted notification state exceeds limits".into());
            }
            self.next_notification_id = list
                .iter()
                .map(|notification| notification.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| "notification identifier exhausted".to_string())?;
            self.notifications = list;
        }
        Ok(())
    }
}

fn registry_value(subkey: &str, value_name: &str, value: &str) -> RegistryValuePlan {
    RegistryValuePlan {
        subkey: subkey.to_string(),
        value_name: value_name.to_string(),
        value: value.to_string(),
    }
}

fn validate_component_id(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 96
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(format!("{field} is not a safe bounded identifier"));
    }
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("path is empty".into());
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| error.to_string())
    }
}

fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PERSISTED_STATE_BYTES {
        return Err("Windows integration state exceeds 1 MiB".into());
    }
    crate::fs_atomic::atomic_write_bytes(path, &bytes).map_err(|error| error.to_string())
}

fn read_bounded_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    if fs::metadata(path).map_err(|error| error.to_string())?.len() > MAX_PERSISTED_STATE_BYTES {
        return Err("Windows integration state exceeds 1 MiB".into());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn set_hkcu_string(subkey: &str, name: &str, value: &str) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let key_wide = wide_null(subkey);
    let name_wide = wide_null(name);
    let value_wide = wide_null(value);
    let bytes = unsafe {
        std::slice::from_raw_parts(
            value_wide.as_ptr().cast::<u8>(),
            value_wide.len() * std::mem::size_of::<u16>(),
        )
    };
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_wide.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .map_err(|error| error.to_string())?;
        let result = RegSetValueExW(key, PCWSTR(name_wide.as_ptr()), 0, REG_SZ, Some(bytes))
            .map_err(|error| error.to_string());
        let _ = RegCloseKey(key);
        result
    }
}

#[cfg(target_os = "windows")]
fn delete_hkcu_value(subkey: &str, name: &str) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    };

    let subkey = wide_null(subkey);
    let name = wide_null(name);
    let mut key = HKEY::default();
    unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_WRITE,
            &mut key,
        )
        .map_err(|error| error.to_string())?;
        let result = RegDeleteValueW(key, PCWSTR(name.as_ptr())).map_err(|error| error.to_string());
        let _ = RegCloseKey(key);
        result
    }
}

#[cfg(target_os = "windows")]
fn notify_association_change() {
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

#[cfg(target_os = "windows")]
fn verify_authenticode(path: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    let path_wide = wide_null(&path.to_string_lossy());
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(path_wide.as_ptr()),
        hFile: HANDLE::default(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        let _ = WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
    }
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "Authenticode verification failed for {} (0x{:08x})",
            path.display(),
            status as u32
        ))
    }
}

/// Identity of the certificate that signed an executable, extracted from its
/// embedded PKCS#7 signature. This is the cryptographic anchor for release
/// acceptance: a valid signature from any other certificate must fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerIdentity {
    /// Simple display subject (typically the `CN` value) of the certificate.
    pub subject: String,
    /// Lowercase hex SHA-256 thumbprint of the certificate.
    pub thumbprint_sha256: String,
}

/// Bind an extracted signer identity to the exact user-approved certificate.
///
/// Fails closed: a missing or malformed approved identity, an unapproved
/// subject or an unapproved thumbprint all reject the signer. The thumbprint
/// comparison is the cryptographic binding; the subject comparison is a
/// human-readable confirmation that both must satisfy.
pub fn validate_signer_identity(
    actual: &SignerIdentity,
    expected_subject: &str,
    expected_thumbprint_sha256: &str,
) -> Result<(), String> {
    let expected_subject = expected_subject.trim();
    let expected_thumbprint = expected_thumbprint_sha256.trim();
    if expected_subject.is_empty() {
        return Err("approved certificate subject is missing".into());
    }
    if !is_sha256_hex(expected_thumbprint) {
        return Err(
            "approved certificate thumbprint must be a 64-character SHA-256 hex digest".into(),
        );
    }
    if !actual.subject.trim().eq_ignore_ascii_case(expected_subject) {
        return Err(format!(
            "signed certificate subject '{}' does not match the approved '{}'",
            actual.subject, expected_subject
        ));
    }
    if !actual
        .thumbprint_sha256
        .eq_ignore_ascii_case(expected_thumbprint)
    {
        return Err(format!(
            "signed certificate thumbprint '{}' does not match the approved '{}'",
            actual.thumbprint_sha256, expected_thumbprint
        ));
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(target_os = "windows")]
fn signed_executable_signer(path: &Path) -> Result<SignerIdentity, String> {
    use std::ffi::c_void;

    use windows::Win32::Security::Cryptography::{
        CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext,
        CertGetCertificateContextProperty, CertGetNameStringW, CryptMsgClose, CryptMsgGetParam,
        CryptQueryObject, CERT_CONTEXT, CERT_FIND_FLAGS, CERT_NAME_SIMPLE_DISPLAY_TYPE,
        CERT_QUERY_CONTENT_FLAG_ALL, CERT_QUERY_CONTENT_TYPE, CERT_QUERY_ENCODING_TYPE,
        CERT_QUERY_FORMAT_FLAG_ALL, CERT_QUERY_FORMAT_TYPE, CERT_QUERY_OBJECT_FILE,
        CERT_SHA256_HASH_PROP_ID, CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM, CRYPT_INTEGER_BLOB,
        HCERTSTORE,
    };

    // CERT_FIND_ISSUER_SERIAL_NUMBER is not exported by the 0.52 metadata.
    const CERT_FIND_ISSUER_SERIAL_NUMBER: CERT_FIND_FLAGS = CERT_FIND_FLAGS(0x0004_0004);

    #[repr(C)]
    struct CertFindIssuerSerial {
        issuer: CRYPT_INTEGER_BLOB,
        serial_number: CRYPT_INTEGER_BLOB,
    }

    struct SignatureHandles {
        store: HCERTSTORE,
        message: *mut c_void,
        cert: *mut CERT_CONTEXT,
    }
    impl Drop for SignatureHandles {
        fn drop(&mut self) {
            if !self.cert.is_null() {
                unsafe { CertFreeCertificateContext(Some(self.cert)) };
            }
            if !self.message.is_null() {
                unsafe {
                    let _ = CryptMsgClose(Some(self.message));
                }
            }
            if !self.store.0.is_null() {
                unsafe {
                    let _ = CertCloseStore(self.store, 0);
                }
            }
        }
    }

    let path_wide = wide_null(&path.to_string_lossy());
    let mut encoding = CERT_QUERY_ENCODING_TYPE::default();
    let mut content_type = CERT_QUERY_CONTENT_TYPE::default();
    let mut format_type = CERT_QUERY_FORMAT_TYPE::default();
    let mut store = HCERTSTORE::default();
    let mut message: *mut c_void = std::ptr::null_mut();
    unsafe {
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            path_wide.as_ptr().cast(),
            CERT_QUERY_CONTENT_FLAG_ALL,
            CERT_QUERY_FORMAT_FLAG_ALL,
            0,
            Some(&mut encoding),
            Some(&mut content_type),
            Some(&mut format_type),
            Some(&mut store),
            Some(&mut message),
            None,
        )
        .map_err(|error| format!("cannot query the signed file: {error}"))?;
    }
    let mut handles = SignatureHandles {
        store,
        message,
        cert: std::ptr::null_mut(),
    };
    if message.is_null() {
        return Err("signed executable has no embedded PKCS#7 signature message".into());
    }

    let mut size = 0u32;
    unsafe { CryptMsgGetParam(message, CMSG_SIGNER_INFO_PARAM, 0, None, &mut size) }
        .map_err(|error| format!("cannot query signer info size: {error}"))?;
    if size == 0 || size > 16 * 1024 {
        return Err("signer info has an unexpected size".into());
    }
    let mut signer_bytes = vec![0u8; size as usize];
    unsafe {
        CryptMsgGetParam(
            message,
            CMSG_SIGNER_INFO_PARAM,
            0,
            Some(signer_bytes.as_mut_ptr().cast()),
            &mut size,
        )
        .map_err(|error| format!("cannot read signer info: {error}"))?;
    }
    let signer = unsafe { &*(signer_bytes.as_ptr().cast::<CMSG_SIGNER_INFO>()) };
    let find = CertFindIssuerSerial {
        issuer: signer.Issuer,
        serial_number: signer.SerialNumber,
    };
    let cert = unsafe {
        CertFindCertificateInStore(
            store,
            encoding,
            0,
            CERT_FIND_ISSUER_SERIAL_NUMBER,
            Some((&find as *const CertFindIssuerSerial).cast()),
            None,
        )
    };
    if cert.is_null() {
        return Err("signer certificate is not present in the signature store".into());
    }
    handles.cert = cert;

    let mut subject_buffer = [0u16; 512];
    let length = unsafe {
        CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            None,
            Some(&mut subject_buffer),
        )
    };
    if length == 0 {
        return Err("cannot read the signer certificate subject".into());
    }
    // CertGetNameStringW reports the required character count (including the
    // terminator) and leaves the buffer untouched when it is too small, so
    // clamp instead of indexing past 512 entries.
    let subject_len = (length as usize - 1).min(subject_buffer.len());
    let subject = String::from_utf16_lossy(&subject_buffer[..subject_len]);

    let mut thumbprint = [0u8; 64];
    let mut thumbprint_size = thumbprint.len() as u32;
    unsafe {
        CertGetCertificateContextProperty(
            cert,
            CERT_SHA256_HASH_PROP_ID,
            Some(thumbprint.as_mut_ptr().cast()),
            &mut thumbprint_size,
        )
        .map_err(|error| format!("cannot read the certificate thumbprint: {error}"))?;
    }
    let thumbprint_sha256 =
        crate::package_crypto::encode_hex(&thumbprint[..thumbprint_size as usize]);

    Ok(SignerIdentity {
        subject,
        thumbprint_sha256,
    })
}

#[cfg(target_os = "windows")]
pub fn verify_signed_executable(
    path: &Path,
    expected_subject: &str,
    expected_thumbprint_sha256: &str,
) -> Result<(), String> {
    verify_authenticode(path)?;
    let signer = signed_executable_signer(path)?;
    validate_signer_identity(&signer, expected_subject, expected_thumbprint_sha256)
}

#[cfg(not(target_os = "windows"))]
pub fn verify_signed_executable(
    _path: &Path,
    _expected_subject: &str,
    _expected_thumbprint_sha256: &str,
) -> Result<(), String> {
    Err("Authenticode verification is only available on Windows".into())
}

// Password manager store

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCredential {
    pub username: String,
    pub password: String,
}

/// Narrow adapter around Windows Credential Manager. Password bytes are never
/// persisted in GhitaBrowser's JSON profile; Windows owns encryption and user
/// account access control.
pub struct WindowsCredentialStore;

impl WindowsCredentialStore {
    #[cfg(windows)]
    pub fn save(profile: &str, origin: &str, username: &str, password: &str) -> Result<(), String> {
        use windows::core::PWSTR;
        use windows::Win32::Security::Credentials::{
            CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
        };

        let target = credential_target(profile, origin, username)?;
        let mut target_wide = wide_null(&target);
        let mut username_wide = wide_null(username);
        let mut password_bytes = password.as_bytes().to_vec();
        if password_bytes.len() > 5 * 512 {
            return Err("Credential password exceeds Windows generic credential limit".to_string());
        }
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target_wide.as_mut_ptr()),
            CredentialBlobSize: password_bytes.len() as u32,
            CredentialBlob: password_bytes.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(username_wide.as_mut_ptr()),
            ..Default::default()
        };
        unsafe { CredWriteW(&credential, 0) }
            .map_err(|error| format!("Windows Credential Manager write failed: {error}"))
    }

    #[cfg(not(windows))]
    pub fn save(
        _profile: &str,
        _origin: &str,
        _username: &str,
        _password: &str,
    ) -> Result<(), String> {
        Err("Windows Credential Manager is unavailable".to_string())
    }

    #[cfg(windows)]
    pub fn read(profile: &str, origin: &str, username: &str) -> Result<SystemCredential, String> {
        use windows::core::PCWSTR;
        use windows::Win32::Security::Credentials::{
            CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
        };

        let target = wide_null(&credential_target(profile, origin, username)?);
        let mut raw: *mut CREDENTIALW = std::ptr::null_mut();
        unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0, &mut raw) }
            .map_err(|error| format!("Windows Credential Manager read failed: {error}"))?;
        if raw.is_null() {
            return Err("Windows Credential Manager returned an empty credential".to_string());
        }
        let credential = unsafe { &*raw };
        let stored_username = unsafe { credential.UserName.to_string() };
        let password_bytes = if credential.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            unsafe {
                std::slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
                .to_vec()
            }
        };
        unsafe { CredFree(raw.cast()) };
        let stored_username =
            stored_username.map_err(|error| format!("Credential username is invalid: {error}"))?;
        let password = String::from_utf8(password_bytes)
            .map_err(|_| "Credential password is not valid UTF-8".to_string())?;
        Ok(SystemCredential {
            username: stored_username,
            password,
        })
    }

    #[cfg(not(windows))]
    pub fn read(
        _profile: &str,
        _origin: &str,
        _username: &str,
    ) -> Result<SystemCredential, String> {
        Err("Windows Credential Manager is unavailable".to_string())
    }

    #[cfg(windows)]
    pub fn delete(profile: &str, origin: &str, username: &str) -> Result<(), String> {
        use windows::core::PCWSTR;
        use windows::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};

        let target = wide_null(&credential_target(profile, origin, username)?);
        unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0) }
            .map_err(|error| format!("Windows Credential Manager delete failed: {error}"))
    }

    #[cfg(not(windows))]
    pub fn delete(_profile: &str, _origin: &str, _username: &str) -> Result<(), String> {
        Err("Windows Credential Manager is unavailable".to_string())
    }
}

/// Windows WebAuthn platform capability used by the future navigator.credentials
/// prompt path. This exposes capability only; credential requests still require
/// an explicit user gesture and a browser-window handle.
pub struct WindowsPasskeyPlatform;

impl WindowsPasskeyPlatform {
    pub fn api_version() -> u32 {
        #[cfg(windows)]
        unsafe {
            windows::Win32::Networking::WindowsWebServices::WebAuthNGetApiVersionNumber()
        }
        #[cfg(not(windows))]
        {
            0
        }
    }

    pub fn is_available() -> bool {
        Self::api_version() > 0
    }
}

fn credential_target(profile: &str, origin: &str, username: &str) -> Result<String, String> {
    let profile = profile.trim();
    let username = username.trim();
    if profile.is_empty() || profile.len() > 64 || username.is_empty() || username.len() > 512 {
        return Err("Credential profile or username is invalid".to_string());
    }
    let parsed = url::Url::parse(origin).map_err(|_| "Credential origin is invalid".to_string())?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err("Credentials require an HTTPS origin".to_string());
    }
    let origin = parsed.origin().ascii_serialization();
    Ok(format!("GhitaBrowser/{profile}/{origin}/{username}"))
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPassword {
    pub id: String,
    pub domain: String,
    pub username: String,
    /// Obfuscated (NOT hashed — reversible for autofill) password bytes.
    /// See `obfuscate`; the plaintext is never stored verbatim.
    pub password_hash: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PasswordStore {
    pub entries: Vec<SavedPassword>,
    /// Monotonic id counter so two saves within the same millisecond can't
    /// collide on the same id (delete/pin operations would act on the wrong
    /// entry).
    #[serde(default)]
    next_id: u64,
}

impl PasswordStore {
    pub fn add_password(
        &mut self,
        domain: String,
        username: String,
        password_plain: &str,
    ) -> String {
        let id = format!(
            "pwd-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            self.next_id
        );
        self.next_id += 1;
        let created_at = chrono::Utc::now().to_rfc3339();

        // Obfuscate so the plaintext never sits verbatim in memory/on disk.
        // Note: reversible by design (autofill needs the real password) — a
        // production password manager would use a KDF or the OS keychain.
        let password_hash = obfuscate(password_plain, chrono::Utc::now().timestamp_millis() as u64);

        // Update if existing domain+username exists
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.domain == domain && e.username == username)
        {
            existing.password_hash = password_hash;
            return existing.id.clone();
        }

        self.entries.push(SavedPassword {
            id: id.clone(),
            domain,
            username,
            password_hash,
            created_at,
        });

        id
    }

    /// Find saved credentials for a domain. Matches the exact host or a
    /// subdomain of the stored domain (like cookie matching) — never bare
    /// substring containment, which would match "evil-example.com" for
    /// "example.com" and offer credentials to attacker-controlled hosts.
    pub fn find_for_domain(&self, domain: &str) -> Vec<&SavedPassword> {
        let domain = domain.trim_start_matches('.').to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                let stored = e.domain.trim_start_matches('.').to_ascii_lowercase();
                domain == stored || domain.ends_with(&format!(".{}", stored))
            })
            .collect()
    }

    /// Recover the plaintext password for an entry (for autofill).
    pub fn decoded_password(&self, entry: &SavedPassword) -> String {
        deobfuscate(&entry.password_hash)
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let initial_len = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() < initial_len
    }
}

/// Reversibly obfuscate a password: XOR each byte with a keystream derived
/// from an entry-specific nonce, hex-encoded with the nonce as prefix.
///
/// NOT encryption — this keeps the plaintext out of memory dumps and text
/// scans but is trivially reversible by design. A real manager would use
/// argon2/bcrypt for verification and the OS keychain for retrieval.
fn obfuscate(plain: &str, nonce: u64) -> String {
    let bytes = plain.as_bytes();
    let mut out = String::with_capacity(bytes.len() * 2 + 16);
    out.push_str(&format!("{:016x}", nonce));
    let mut state = nonce;
    for (i, &b) in bytes.iter().enumerate() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let key = (state >> 33) as u8 ^ (i as u8).wrapping_mul(31);
        out.push_str(&format!("{:02x}", b ^ key));
    }
    out
}

/// Reverse `obfuscate`. Returns an empty string for malformed input.
fn deobfuscate(encoded: &str) -> String {
    if encoded.len() < 16 {
        return String::new();
    }
    let (nonce_hex, hex) = encoded.split_at(16);
    let nonce = u64::from_str_radix(nonce_hex, 16).unwrap_or(0);

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let hb = hex.as_bytes();
    let mut i = 0;
    while i + 1 < hb.len() {
        let hi = hex_val(hb[i]);
        let lo = hex_val(hb[i + 1]);
        if let (Some(h), Some(l)) = (hi, lo) {
            bytes.push(h << 4 | l);
        }
        i += 2;
    }

    let mut state = nonce;
    for (i, b) in bytes.iter_mut().enumerate() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let key = (state >> 33) as u8 ^ (i as u8).wrapping_mul(31);
        *b ^= key;
    }

    String::from_utf8(bytes).unwrap_or_default()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_store() {
        let mut store = PasswordStore::default();
        let id = store.add_password("example.com".to_string(), "user1".to_string(), "secret123");
        assert_eq!(store.entries.len(), 1);

        let matches = store.find_for_domain("example.com");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].username, "user1");

        assert!(store.delete(&id));
        assert_eq!(store.entries.len(), 0);
    }

    #[test]
    fn test_password_not_stored_in_plaintext() {
        let mut store = PasswordStore::default();
        store.add_password("example.com".to_string(), "u".to_string(), "hunter2secret");
        let stored = &store.entries[0].password_hash;
        assert!(
            !stored.contains("hunter2secret"),
            "plaintext must not be stored verbatim"
        );
        // Round-trip must recover the original
        assert_eq!(store.decoded_password(&store.entries[0]), "hunter2secret");
    }

    #[test]
    fn test_find_for_domain_exact_and_subdomain_not_substring() {
        let mut store = PasswordStore::default();
        store.add_password("example.com".to_string(), "u".to_string(), "p");
        store.add_password("api.example.com".to_string(), "v".to_string(), "q");

        // Exact and subdomain hits
        assert_eq!(store.find_for_domain("example.com").len(), 1);
        assert_eq!(store.find_for_domain("www.example.com").len(), 1);
        assert_eq!(store.find_for_domain("api.example.com").len(), 2);

        // Substring lookalikes must NOT match
        assert_eq!(store.find_for_domain("badexample.com").len(), 0);
        assert_eq!(store.find_for_domain("example.com.evil.net").len(), 0);
        assert_eq!(store.find_for_domain("my-example.com").len(), 0);
    }

    #[test]
    fn system_credential_targets_require_exact_https_origins() {
        let target = credential_target("Work", "https://example.com/login", "alice").unwrap();
        assert_eq!(target, "GhitaBrowser/Work/https://example.com/alice");
        assert!(credential_target("Work", "http://example.com", "alice").is_err());
        assert!(credential_target("Work", "https://example.com", "").is_err());
    }
}

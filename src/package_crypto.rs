//! Shared package-authenticity and path-safety primitives.
//!
//! GhitaBrowser owns the canonical byte formats and publisher trust policy.
//! Ed25519 and SHA-256 are used only as general-purpose cryptographic
//! primitives; no other browser package format or trust store is embedded.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub const MAX_KEY_ID_BYTES: usize = 128;
pub const MAX_PACKAGE_PATH_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageCryptoError {
    InvalidKeyId(String),
    InvalidPublicKey(String),
    InvalidSignature(String),
    UnknownPublisher(String),
    UnsafePath(String),
}

impl std::fmt::Display for PackageCryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidKeyId(message) => write!(f, "invalid key id: {message}"),
            Self::InvalidPublicKey(message) => write!(f, "invalid public key: {message}"),
            Self::InvalidSignature(message) => write!(f, "invalid signature: {message}"),
            Self::UnknownPublisher(key_id) => write!(f, "unknown publisher: {key_id}"),
            Self::UnsafePath(path) => write!(f, "unsafe package path: {path}"),
        }
    }
}

impl std::error::Error for PackageCryptoError {}

/// Explicit publisher keys trusted by the current browser profile/channel.
#[derive(Debug, Clone, Default)]
pub struct PublisherTrustStore {
    keys: BTreeMap<String, VerifyingKey>,
}

impl PublisherTrustStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_ed25519(
        &mut self,
        key_id: impl Into<String>,
        public_key: [u8; 32],
    ) -> Result<(), PackageCryptoError> {
        let key_id = key_id.into();
        validate_key_id(&key_id)?;
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|error| PackageCryptoError::InvalidPublicKey(error.to_string()))?;
        self.keys.insert(key_id, verifying_key);
        Ok(())
    }

    pub fn remove(&mut self, key_id: &str) -> bool {
        self.keys.remove(key_id).is_some()
    }

    pub fn contains(&self, key_id: &str) -> bool {
        self.keys.contains_key(key_id)
    }

    pub fn export_ed25519(&self) -> BTreeMap<String, [u8; 32]> {
        self.keys
            .iter()
            .map(|(key_id, key)| (key_id.clone(), key.to_bytes()))
            .collect()
    }

    pub fn import_ed25519(keys: BTreeMap<String, [u8; 32]>) -> Result<Self, PackageCryptoError> {
        let mut store = Self::new();
        for (key_id, key) in keys {
            store.insert_ed25519(key_id, key)?;
        }
        Ok(store)
    }

    pub fn verify(
        &self,
        key_id: &str,
        payload: &[u8],
        signature_hex: &str,
    ) -> Result<(), PackageCryptoError> {
        validate_key_id(key_id)?;
        let key = self
            .keys
            .get(key_id)
            .ok_or_else(|| PackageCryptoError::UnknownPublisher(key_id.to_string()))?;
        let signature_bytes = decode_hex_exact::<64>(signature_hex)?;
        let signature = Signature::from_bytes(&signature_bytes);
        key.verify(payload, &signature)
            .map_err(|_| PackageCryptoError::InvalidSignature("Ed25519 verification failed".into()))
    }
}

pub fn validate_key_id(key_id: &str) -> Result<(), PackageCryptoError> {
    if key_id.is_empty() || key_id.len() > MAX_KEY_ID_BYTES {
        return Err(PackageCryptoError::InvalidKeyId(
            "length must be between 1 and 128 bytes".into(),
        ));
    }
    if !key_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(PackageCryptoError::InvalidKeyId(
            "only ASCII letters, digits, dot, underscore and hyphen are allowed".into(),
        ));
    }
    Ok(())
}

/// Accept only a relative sequence of normal components. This rejects parent
/// traversal, drive/prefix paths, absolute paths and empty package paths.
pub fn validate_package_path(path: &str) -> Result<(), PackageCryptoError> {
    if path.is_empty() || path.len() > MAX_PACKAGE_PATH_BYTES {
        return Err(PackageCryptoError::UnsafePath(path.to_string()));
    }
    let parsed = Path::new(path);
    if parsed.is_absolute() {
        return Err(PackageCryptoError::UnsafePath(path.to_string()));
    }
    let mut components = 0usize;
    for component in parsed.components() {
        match component {
            Component::Normal(part) if !part.is_empty() => components += 1,
            _ => return Err(PackageCryptoError::UnsafePath(path.to_string())),
        }
    }
    if components == 0 {
        return Err(PackageCryptoError::UnsafePath(path.to_string()));
    }
    Ok(())
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    encode_hex(&sha256(bytes))
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn decode_hex_exact<const N: usize>(value: &str) -> Result<[u8; N], PackageCryptoError> {
    if value.len() != N * 2 || !value.is_ascii() {
        return Err(PackageCryptoError::InvalidSignature(format!(
            "expected {} hexadecimal characters",
            N * 2
        )));
    }
    let mut output = [0u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_nibble(pair[0])?;
        let low = decode_nibble(pair[1])?;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

fn decode_nibble(byte: u8) -> Result<u8, PackageCryptoError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(PackageCryptoError::InvalidSignature(
            "signature contains a non-hexadecimal character".into(),
        )),
    }
}

/// Length-prefix every field to remove delimiter ambiguity from signed
/// payloads. Integers are encoded big-endian for cross-machine stability.
#[derive(Debug, Default)]
pub struct CanonicalBytes {
    bytes: Vec<u8>,
}

impl CanonicalBytes {
    pub fn new(domain: &[u8]) -> Self {
        let mut value = Self::default();
        value.push_bytes(domain);
        value
    }

    pub fn push_str(&mut self, value: &str) {
        self.push_bytes(value.as_bytes());
    }

    pub fn push_bytes(&mut self, value: &[u8]) {
        self.bytes
            .extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
        self.bytes.extend_from_slice(value);
    }

    pub fn push_u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_paths_reject_escape_and_absolute_forms() {
        for path in ["../escape", "a/../../escape", "/rooted", "C:\\escape", ""] {
            assert!(validate_package_path(path).is_err(), "accepted {path:?}");
        }
        assert!(validate_package_path("scripts/background.js").is_ok());
    }

    #[test]
    fn hex_decoder_is_strict() {
        assert_eq!(decode_hex_exact::<2>("00ff").unwrap(), [0, 255]);
        assert!(decode_hex_exact::<2>("0xff").is_err());
        assert!(decode_hex_exact::<2>("abc").is_err());
    }
}

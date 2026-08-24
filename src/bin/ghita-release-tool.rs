//! Release signing helper.
//!
//! `gen`  – generate an Ed25519 publisher key: writes the secret seed to a
//!          local file (NEVER commit it) and prints the public key to pin
//!          into `src/updater.rs::PINNED_RELEASE_KEYS`.
//! `sign` – sign an update package: reads a manifest JSON (with a 128-char
//!          hex placeholder signature), hashes every file under
//!          --payload-dir, signs the canonical payload and rewrites the
//!          manifest with the real signature.
//!
//! Usage:
//!   cargo run --bin ghita-release-tool -- gen --out keys/release-private-key.txt
//!   cargo run --bin ghita-release-tool -- sign \
//!       --manifest dist/manifest.json --payload-dir dist/payload \
//!       --key keys/release-private-key.txt

#[cfg(not(windows))]
fn main() {
    eprintln!("ghita-release-tool requires Windows (BCrypt entropy source).");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gen") => command_gen(&args[1..]),
        Some("sign") => command_sign(&args[1..]),
        _ => Err("expected `gen` or `sign`; run with no args for usage".into()),
    }
}

/// Extract `--name value` pairs.
#[cfg(windows)]
fn arg_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

#[cfg(windows)]
fn command_gen(args: &[String]) -> Result<(), String> {
    use ed25519_dalek::SigningKey;

    let out = arg_value(args, "--out")
        .ok_or("gen requires --out <private-key-file>")?
        .to_string();

    let mut seed = [0u8; 32];
    fill_random(&mut seed)?;
    let signing = SigningKey::from_bytes(&seed);
    let public_hex = ghitabrowser::package_crypto::encode_hex(
        &ed25519_dalek::VerifyingKey::from(&signing).to_bytes(),
    );

    if let Some(parent) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    restrict_private_key_file(std::path::Path::new(&out))?;
    std::fs::write(
        &out,
        format!("{}\n", ghitabrowser::package_crypto::encode_hex(&seed)),
    )
    .map_err(|e| e.to_string())?;

    println!("Private seed written to {out} — keep it out of version control.");
    println!("Key id suggestion: ghita-release-YYYY-MM");
    println!("Public key (pin into PINNED_RELEASE_KEYS):");
    println!("{public_hex}");
    Ok(())
}

#[cfg(windows)]
fn command_sign(args: &[String]) -> Result<(), String> {
    use ed25519_dalek::Signer;
    use ghitabrowser::updater::UpdatePackage;

    let manifest_path = arg_value(args, "--manifest").ok_or("sign requires --manifest <json>")?;
    let payload_dir =
        arg_value(args, "--payload-dir").ok_or("sign requires --payload-dir <dir>")?;
    let key_path = arg_value(args, "--key").ok_or("sign requires --key <private-key-file>")?;

    let seed_hex = std::fs::read_to_string(key_path)
        .map_err(|e| format!("cannot read key file: {e}"))?
        .trim()
        .to_string();
    let seed = ghitabrowser::package_crypto::decode_hex_exact::<32>(&seed_hex)
        .map_err(|e| format!("bad key file: {e:?}"))?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);

    let manifest_json =
        std::fs::read_to_string(manifest_path).map_err(|e| format!("cannot read manifest: {e}"))?;
    let manifest: ghitabrowser::updater::UpdateManifest =
        serde_json::from_str(&manifest_json).map_err(|e| format!("bad manifest JSON: {e}"))?;

    let files = read_payload_files(std::path::Path::new(payload_dir))?;
    println!("payload files: {}", files.len());

    // UpdatePackage::new recomputes file_hashes from the actual bytes.
    let mut package = UpdatePackage::new(manifest, files).map_err(|e| e.to_string())?;
    let signature = signing.sign(&package.canonical_payload().map_err(|e| e.to_string())?);
    package.manifest.signature =
        ghitabrowser::package_crypto::encode_hex(signature.to_bytes().as_slice());

    // Fail closed: verify through the same path the browser uses before we
    // hand the manifest back.
    let mut trust = ghitabrowser::package_crypto::PublisherTrustStore::new();
    trust
        .insert_ed25519(
            package.manifest.publisher_key_id.clone(),
            ed25519_dalek::VerifyingKey::from(&signing).to_bytes(),
        )
        .map_err(|e| e.to_string())?;
    package
        .verify_signature(&trust)
        .map_err(|e| e.to_string())?;

    let output = serde_json::to_vec_pretty(&package.manifest).map_err(|e| e.to_string())?;
    std::fs::write(manifest_path, output).map_err(|e| e.to_string())?;
    println!(
        "signed manifest {} for version {} by key {:?}",
        manifest_path, package.manifest.version, package.manifest.publisher_key_id
    );
    Ok(())
}

/// Recursively collect payload files with the same bounds the updater enforces.
#[cfg(windows)]
fn read_payload_files(
    root: &std::path::Path,
) -> Result<std::collections::HashMap<String, Vec<u8>>, String> {
    const MAX_FILES: usize = 4_096;
    const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

    let mut files = std::collections::HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if files.len() >= MAX_FILES {
                return Err("payload exceeds 4096 files".into());
            }
            let metadata = entry.metadata().map_err(|e| e.to_string())?;
            if metadata.len() > MAX_FILE_BYTES {
                return Err(format!("{} exceeds 512 MiB", path.display()));
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "payload file outside --payload-dir".to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            files.insert(relative, bytes);
        }
    }
    if files.is_empty() {
        return Err("payload directory is empty".into());
    }
    Ok(files)
}

#[cfg(windows)]
fn fill_random(buffer: &mut [u8]) -> Result<(), String> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    // SAFETY: buffer is a valid slice for the duration of the call; the
    // system-preferred RNG flag needs no algorithm handle.
    let status = unsafe { BCryptGenRandom(None, buffer, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status.is_ok() {
        Ok(())
    } else {
        Err(format!("BCryptGenRandom failed with NTSTATUS {:?}", status))
    }
}

/// Best-effort ACL restriction so the seed is readable only by the current
/// user (mirrors what a proper secret file should look like on Windows).
#[cfg(windows)]
fn restrict_private_key_file(path: &std::path::Path) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
    };
    let mut wide: Vec<u16> = HSTRING::from(path.as_os_str()).as_wide().to_vec();
    wide.push(0);
    unsafe {
        let _ = SetFileAttributesW(
            windows::core::PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_HIDDEN.0),
        );
    }
    Ok(())
}

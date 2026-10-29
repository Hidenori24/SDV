use sdv_fota::{
    encode_package, validate_signed_manifest, Ecu, TrustKey, ValidationContext, MAX_MANIFEST_BYTES,
};
use sdv_sec::{ed25519_public_key, ed25519_sign, Sha256Context};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const MAX_SEED_FILE_BYTES: usize = 128;

struct Seed([u8; 32]);

impl Drop for Seed {
    fn drop(&mut self) {
        zeroize(&mut self.0);
    }
}

struct SeedText(Vec<u8>);

impl Drop for SeedText {
    fn drop(&mut self) {
        zeroize(&mut self.0);
    }
}

#[derive(Debug, PartialEq)]
struct SignOptions {
    seed_file: PathBuf,
    key_id: String,
    target: String,
    version: String,
    security_version: u64,
    trial_ms: u64,
    config_schema: u64,
    signal_if_version: u64,
    image_path: PathBuf,
    output_path: PathBuf,
}

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("sdv-fota-sign: {error}");
        std::process::exit(2);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let options = parse_options(args)?;
    let seed = read_seed(&options.seed_file)?;
    let (ecu, swc) = match options.target.as_str() {
        "engine" => (Ecu::Engine, "EngineSWC"),
        "brake" => (Ecu::Brake, "BrakeSWC"),
        "steering" => (Ecu::Steering, "SteeringSWC"),
        "vehicle" => (Ecu::Vehicle, "VehicleControlApp"),
        _ => return Err("target must be engine, brake, steering, or vehicle".into()),
    };
    let version = &options.version;
    let security_version = options.security_version;
    let trial_ms = options.trial_ms;
    let config_schema = options.config_schema;
    let signal_if_version = options.signal_if_version;
    let image_path = options.image_path;
    let output_path = options.output_path;

    let config_schema = u32::try_from(config_schema)
        .map_err(|_| "config schema must fit an unsigned 32-bit integer".to_owned())?;
    let signal_if_version = u32::try_from(signal_if_version)
        .map_err(|_| "signal interface version must fit an unsigned 32-bit integer".to_owned())?;

    let (payload_size, payload_digest) = hash_file(&image_path)?;
    if payload_size == 0 || payload_size > u64::from(u32::MAX) {
        return Err("image must be between 1 byte and 4 GiB - 1".into());
    }
    if trial_ms == 0 || trial_ms > u64::from(u32::MAX) {
        return Err("trial window must be a nonzero 32-bit number of milliseconds".into());
    }
    if config_schema == 0 || signal_if_version == 0 {
        return Err("config schema and signal interface version must be nonzero".into());
    }
    let manifest = serde_json::json!({
        "manifest_schema": 1,
        "image": {
            "target_ecu": ecu_name(ecu),
            "swc": swc,
            "version": version,
            "security_version": security_version,
            "size_bytes": payload_size,
            "sha256": hex(&payload_digest)
        },
        "requires": {
            "signal_if_version": signal_if_version,
            "config_schema": [config_schema]
        },
        "policy": {"allow_downgrade": false, "trial_ms": trial_ms}
    });
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err("encoded manifest exceeds the package limit".into());
    }
    let signature = ed25519_sign(&seed.0, &manifest_bytes);
    let public_key = ed25519_public_key(&seed.0);
    let validation_key = TrustKey {
        key_id: &options.key_id,
        public_key: &public_key,
    };
    let validation_context = ValidationContext {
        target_ecu: ecu,
        current_version: "0.0.0",
        highest_security_version: 0,
        config_schema,
        signal_if_version,
    };
    validate_signed_manifest(
        &options.key_id,
        &signature,
        &manifest_bytes,
        payload_size,
        &[validation_key],
        &validation_context,
    )
    .map_err(|error| format!("release metadata is invalid: {error:?}"))?;
    let result = write_package(
        &options.key_id,
        &signature,
        &manifest_bytes,
        payload_size,
        &payload_digest,
        &image_path,
        &output_path,
    );
    result?;
    println!("package: {}", output_path.display());
    println!("key_id: {}", options.key_id);
    println!("public_key: {}", hex(&public_key));
    Ok(())
}

fn parse_options(args: Vec<String>) -> Result<SignOptions, String> {
    let mut seed_file = None;
    let mut key_id = None;
    let mut target = None;
    let mut version = None;
    let mut security_version = None;
    let mut trial_ms = None;
    let mut config_schema = None;
    let mut signal_if_version = None;
    let mut image_path = None;
    let mut output_path = None;

    let mut args = args.into_iter();
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {option}\n{}", usage()))?;
        match option.as_str() {
            "--seed" => set_once(&mut seed_file, PathBuf::from(value), &option)?,
            "--key-id" => set_once(&mut key_id, value, &option)?,
            "--target" => set_once(&mut target, value, &option)?,
            "--version" => set_once(&mut version, value, &option)?,
            "--security-version" => set_once(&mut security_version, value, &option)?,
            "--trial-ms" => set_once(&mut trial_ms, value, &option)?,
            "--config-schema" => set_once(&mut config_schema, value, &option)?,
            "--signal-if-version" => set_once(&mut signal_if_version, value, &option)?,
            "--image" => set_once(&mut image_path, PathBuf::from(value), &option)?,
            "--output" => set_once(&mut output_path, PathBuf::from(value), &option)?,
            _ => return Err(format!("unknown option: {option}\n{}", usage())),
        }
    }

    Ok(SignOptions {
        seed_file: required(seed_file, "--seed")?,
        key_id: required(key_id, "--key-id")?,
        target: required(target, "--target")?,
        version: required(version, "--version")?,
        security_version: parse_number(
            &required(security_version, "--security-version")?,
            "security version",
        )?,
        trial_ms: parse_number(&required(trial_ms, "--trial-ms")?, "trial window")?,
        config_schema: parse_number(
            &required(config_schema, "--config-schema")?,
            "config schema",
        )?,
        signal_if_version: parse_number(
            &required(signal_if_version, "--signal-if-version")?,
            "signal interface version",
        )?,
        image_path: required(image_path, "--image")?,
        output_path: required(output_path, "--output")?,
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("option supplied more than once: {option}"));
    }
    Ok(())
}

fn required<T>(value: Option<T>, option: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("missing required option {option}\n{}", usage()))
}

fn write_package(
    key_id: &str,
    signature: &[u8; 64],
    manifest: &[u8],
    payload_size: u64,
    expected_digest: &[u8; 32],
    image_path: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let mut prefix = encode_package(key_id, signature, manifest, &[])
        .map_err(|error| format!("invalid package metadata: {error:?}"))?;
    prefix[10..18].copy_from_slice(&payload_size.to_be_bytes());
    let temporary = output_path.with_extension("sdvf.tmp");
    let result = (|| {
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        output
            .write_all(&prefix)
            .map_err(|error| error.to_string())?;
        let mut input = File::open(image_path).map_err(|error| error.to_string())?;
        let mut digest = Sha256Context::new();
        let mut copied = 0u64;
        let mut buffer = [0u8; 4096];
        loop {
            let read = input.read(&mut buffer).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(|error| error.to_string())?;
            digest.update(&buffer[..read]);
            copied = copied
                .checked_add(read as u64)
                .ok_or_else(|| "image size overflow".to_owned())?;
        }
        if copied != payload_size || &digest.finalize() != expected_digest {
            return Err("image changed while the package was being created".into());
        }
        output.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, output_path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn hash_file(path: &Path) -> Result<(u64, [u8; 32]), String> {
    let mut input = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256Context::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 4096];
    loop {
        let read = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| "image size overflow".to_owned())?;
        digest.update(&buffer[..read]);
    }
    Ok((size, digest.finalize()))
}

fn read_seed(path: &Path) -> Result<Seed, String> {
    let path_metadata =
        fs::symlink_metadata(path).map_err(|error| format!("cannot inspect seed file: {error}"))?;
    if !path_metadata.file_type().is_file() {
        return Err("seed path must be a regular file".into());
    }
    let file = File::open(path).map_err(|error| format!("cannot read seed file: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect seed file: {error}"))?;
    if !metadata.is_file() {
        return Err("seed path must be a regular file".into());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("seed file must not be accessible by group or other users".into());
    }

    let mut text = SeedText(Vec::with_capacity(MAX_SEED_FILE_BYTES + 1));
    file.take((MAX_SEED_FILE_BYTES + 1) as u64)
        .read_to_end(&mut text.0)
        .map_err(|error| format!("cannot read seed file: {error}"))?;
    if text.0.len() > MAX_SEED_FILE_BYTES {
        return Err("seed file is too large".into());
    }
    let start = text
        .0
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(text.0.len());
    let end = text
        .0
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    let bytes = &text.0[start..end];
    if bytes.len() != 64 {
        return Err("seed file must contain exactly 64 hexadecimal characters".into());
    }
    let mut seed = Seed([0u8; 32]);
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        seed.0[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(seed)
}

fn zeroize(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: each reference points to a writable byte owned by the secret buffer.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("seed contains a non-hexadecimal character".into()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_number(value: &str, label: &str) -> Result<u64, String> {
    value.parse().map_err(|_| format!("invalid {label}"))
}

fn ecu_name(ecu: Ecu) -> &'static str {
    match ecu {
        Ecu::Engine => "engine",
        Ecu::Brake => "brake",
        Ecu::Steering => "steering",
        Ecu::Vehicle => "vehicle",
    }
}

fn usage() -> String {
    "usage: sdv-fota-sign --seed <file> --key-id <id> --target <ecu> --version <version> --security-version <n> --trial-ms <n> --config-schema <n> --signal-if-version <n> --image <file> --output <package.sdvf>".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_options_independent_of_order() {
        let options = parse_options(vec![
            "--image".into(),
            "image.bin".into(),
            "--target".into(),
            "vehicle".into(),
            "--seed".into(),
            "release.seed".into(),
            "--key-id".into(),
            "release-1".into(),
            "--version".into(),
            "1.2.0".into(),
            "--security-version".into(),
            "3".into(),
            "--trial-ms".into(),
            "5000".into(),
            "--config-schema".into(),
            "1".into(),
            "--signal-if-version".into(),
            "1".into(),
            "--output".into(),
            "update.sdvf".into(),
        ])
        .unwrap();
        assert_eq!(options.target, "vehicle");
        assert_eq!(options.security_version, 3);
        assert_eq!(options.image_path, PathBuf::from("image.bin"));
    }

    #[test]
    fn rejects_duplicate_unknown_and_positional_arguments() {
        assert!(parse_options(vec!["image.bin".into()]).is_err());
        assert!(parse_options(vec!["--unknown".into(), "value".into()]).is_err());
        assert!(parse_options(vec![
            "--seed".into(),
            "a".into(),
            "--seed".into(),
            "b".into()
        ])
        .is_err());
    }
}

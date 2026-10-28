use sdv_fota::{encode_package, Ecu, MAX_MANIFEST_BYTES};
use sdv_sec::{ed25519_public_key, ed25519_sign, Sha256Context};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

struct Seed([u8; 32]);

impl Drop for Seed {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("sdv-fota-sign: {error}");
        std::process::exit(2);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.len() != 10 {
        return Err(usage());
    }
    let seed = Seed(read_seed(&args[0])?);
    let key_id = &args[1];
    let (ecu, swc) = match args[2].as_str() {
        "engine" => (Ecu::Engine, "EngineSWC"),
        "brake" => (Ecu::Brake, "BrakeSWC"),
        "steering" => (Ecu::Steering, "SteeringSWC"),
        "vehicle" => (Ecu::Vehicle, "VehicleControlApp"),
        _ => return Err("ECU must be engine, brake, steering, or vehicle".into()),
    };
    let version = &args[3];
    let security_version = parse_number(&args[4], "security version")?;
    let trial_ms = parse_number(&args[5], "trial window")?;
    let config_schema = parse_number(&args[6], "config schema")?;
    let signal_if_version = parse_number(&args[7], "signal interface version")?;
    let image_path = PathBuf::from(&args[8]);
    let output_path = PathBuf::from(&args[9]);

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
    let result = write_package(
        key_id,
        &signature,
        &manifest_bytes,
        payload_size,
        &payload_digest,
        &image_path,
        &output_path,
    );
    result?;
    println!("package: {}", output_path.display());
    println!("key_id: {key_id}");
    println!("public_key: {}", hex(&public_key));
    Ok(())
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

fn read_seed(path: &str) -> Result<[u8; 32], String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("cannot read seed file: {error}"))?;
    let text = text.trim();
    if text.len() != 64 {
        return Err("seed file must contain exactly 64 hexadecimal characters".into());
    }
    let mut seed = [0u8; 32];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        seed[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(seed)
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
    "usage: sdv-fota-sign <seed-file> <key-id> <ecu> <version> <security-version> <trial-ms> <config-schema> <signal-if-version> <image-file> <output.sdvf>".into()
}

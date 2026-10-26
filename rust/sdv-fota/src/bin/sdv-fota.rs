use sdv_fota::linux::{FileBootControl, FilePackageSource, FileSlotStorage};
use sdv_fota::{BootControl, Ecu, FotaInstaller, TrustKey, ValidationContext, MAX_MANIFEST_BYTES};
use std::ffi::OsString;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1).collect()) {
        eprintln!("sdv-fota: {error}");
        std::process::exit(2);
    }
}

fn run(args: Vec<OsString>) -> Result<(), String> {
    match args.first().and_then(|value| value.to_str()) {
        Some("init") => initialize(&args[1..]),
        Some("install") => install(&args[1..]),
        _ => Err(usage()),
    }
}

fn initialize(args: &[OsString]) -> Result<(), String> {
    if args.len() != 4 {
        return Err(usage());
    }
    let root = PathBuf::from(&args[0]);
    let slot = match required_str(&args[1], "active slot")? {
        "a" => sdv_fota::Slot::A,
        "b" => sdv_fota::Slot::B,
        _ => return Err("active slot must be `a` or `b`".into()),
    };
    let active_version = required_str(&args[2], "active version")?.to_owned();
    let security_version = parse_number(&args[3], "security version")?;
    FileBootControl::new(root)
        .initialize(slot, active_version, security_version)
        .map_err(|error| format!("boot metadata initialization failed: {error:?}"))
}

fn install(args: &[OsString]) -> Result<(), String> {
    if args.len() < 7 || args.len() > 8 {
        return Err(usage());
    }
    let root = PathBuf::from(&args[0]);
    let package_path = PathBuf::from(&args[1]);
    let key_id = required_str(&args[2], "key ID")?;
    let public_key = parse_public_key(required_str(&args[3], "public key")?)?;
    let target_ecu = match required_str(&args[4], "target ECU")? {
        "engine" => Ecu::Engine,
        "brake" => Ecu::Brake,
        "steering" => Ecu::Steering,
        _ => return Err("target ECU must be engine, brake, or steering".into()),
    };
    let config_schema = u32::try_from(parse_number(&args[5], "config schema")?)
        .map_err(|_| "config schema is out of range".to_owned())?;
    let signal_if_version = u32::try_from(parse_number(&args[6], "signal interface version")?)
        .map_err(|_| "signal interface version is out of range".to_owned())?;
    let trial_boots = if let Some(value) = args.get(7) {
        u32::try_from(parse_number(value, "trial boot count")?)
            .map_err(|_| "trial boot count is out of range".to_owned())?
    } else {
        3
    };

    let boot_control = FileBootControl::new(&root);
    let state = boot_control
        .state()
        .map_err(|error| format!("cannot read boot metadata: {error:?}"))?;
    let context = ValidationContext {
        target_ecu,
        current_version: &state.active_version,
        highest_security_version: state.highest_security_version,
        config_schema,
        signal_if_version,
    };
    let trust_key = TrustKey {
        key_id,
        public_key: &public_key,
    };
    let mut installer = FotaInstaller {
        storage: FileSlotStorage::new(&root),
        boot_control,
        trial_boots,
    };
    let mut source = FilePackageSource::open(package_path)
        .map_err(|error| format!("cannot open package: {error:?}"))?;
    let mut manifest_buffer = vec![0u8; MAX_MANIFEST_BYTES];
    let slot = installer
        .stage_package_from_source(&mut source, &mut manifest_buffer, &[trust_key], &context)
        .map_err(|error| format!("package installation failed: {error:?}"))?;
    println!("staged trial image in slot {}", slot_name(slot));
    Ok(())
}

fn parse_public_key(text: &str) -> Result<[u8; sdv_sec::ED25519_PUBLIC_KEY_LEN], String> {
    if text.len() != sdv_sec::ED25519_PUBLIC_KEY_LEN * 2 {
        return Err("public key must contain exactly 64 hexadecimal characters".into());
    }
    let mut key = [0u8; sdv_sec::ED25519_PUBLIC_KEY_LEN];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        key[index] = (hex(pair[0])? << 4) | hex(pair[1])?;
    }
    Ok(key)
}

fn hex(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("public key contains a non-hexadecimal character".into()),
    }
}

fn parse_number(value: &OsString, label: &str) -> Result<u64, String> {
    required_str(value, label)?
        .parse()
        .map_err(|_| format!("invalid {label}"))
}

fn required_str<'a>(value: &'a OsString, label: &str) -> Result<&'a str, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{label} must be valid UTF-8"))
}

fn slot_name(slot: sdv_fota::Slot) -> &'static str {
    match slot {
        sdv_fota::Slot::A => "A",
        sdv_fota::Slot::B => "B",
    }
}

fn usage() -> String {
    "usage:\n  sdv-fota init <state-root> <active-slot:a|b> <active-version> <security-version>\n  sdv-fota install <state-root> <package-file> <key-id> <public-key-hex> <ecu> <config-schema> <signal-if-version> [trial-boots]".into()
}

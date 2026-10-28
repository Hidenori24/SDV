use sdv_fota::linux::{FileBootControl, FilePackageSource, FileSlotStorage, HttpsPackageSource};
use sdv_fota::{
    BootControl, Ecu, FotaInstaller, PackageSource, ValidationContext, MAX_MANIFEST_BYTES,
};
use std::ffi::OsString;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1).collect()) {
        eprintln!("sdv-fota: {error}");
        std::process::exit(2);
    }
}

fn run(args: Vec<OsString>) -> Result<(), String> {
    let Some((command, command_args)) = args.split_first() else {
        return Err(usage());
    };
    match command.to_str() {
        Some("init") => initialize(command_args),
        Some("install") => install(command_args),
        Some("install-url") => install_url(command_args),
        Some("status") => status(command_args),
        _ => Err(usage()),
    }
}

fn status(args: &[OsString]) -> Result<(), String> {
    let [state_root] = args else {
        return Err(usage());
    };
    let state = FileBootControl::new(PathBuf::from(state_root))
        .state()
        .map_err(|error| format!("cannot read boot metadata: {error:?}"))?;
    println!("active_slot: {}", slot_name(state.active));
    println!("active_version: {}", state.active_version);
    println!(
        "highest_security_version: {}",
        state.highest_security_version
    );
    if let Some(trial) = state.trial {
        println!("trial_slot: {}", slot_name(trial.slot));
        println!("trial_version: {}", trial.version);
        println!("trial_boots_left: {}", trial.boots_left);
        println!("trial_window_ms: {}", trial.trial_window_ms);
    } else {
        println!("trial: none");
    }
    Ok(())
}

fn initialize(args: &[OsString]) -> Result<(), String> {
    let [state_root, active_slot, active_version, security_version] = args else {
        return Err(usage());
    };
    let root = PathBuf::from(state_root);
    let slot = match required_str(active_slot, "active slot")? {
        "a" => sdv_fota::Slot::A,
        "b" => sdv_fota::Slot::B,
        _ => return Err("active slot must be `a` or `b`".into()),
    };
    let active_version = required_str(active_version, "active version")?.to_owned();
    let security_version = parse_number(security_version, "security version")?;
    FileBootControl::new(root)
        .initialize(slot, &active_version, security_version)
        .map_err(|error| format!("boot metadata initialization failed: {error:?}"))
}

fn install(args: &[OsString]) -> Result<(), String> {
    let package_path = match args {
        [_, package_path, _, _, _] | [_, package_path, _, _, _, _] => package_path,
        _ => return Err(usage()),
    };
    let package_path = PathBuf::from(package_path);
    let source = FilePackageSource::open(package_path)
        .map_err(|error| format!("cannot open package: {error:?}"))?;
    install_from_source(args, source)
}

fn install_url(args: &[OsString]) -> Result<(), String> {
    let url = match args {
        [_, url, _, _, _] | [_, url, _, _, _, _] => url,
        _ => return Err(usage()),
    };
    let url = required_str(url, "HTTPS package URL")?;
    let source = HttpsPackageSource::open(url)
        .map_err(|error| format!("cannot start HTTPS package download: {error:?}"))?;
    install_from_source(args, source)
}

fn install_from_source<P: PackageSource<Error = sdv_fota::linux::Error>>(
    args: &[OsString],
    mut source: P,
) -> Result<(), String> {
    let (state_root, target_ecu, config_schema, signal_if_version, trial_boots) = match args {
        [state_root, _, target_ecu, config_schema, signal_if_version] => (
            state_root,
            target_ecu,
            config_schema,
            signal_if_version,
            None,
        ),
        [state_root, _, target_ecu, config_schema, signal_if_version, trial_boots] => (
            state_root,
            target_ecu,
            config_schema,
            signal_if_version,
            Some(trial_boots),
        ),
        _ => return Err(usage()),
    };
    let root = PathBuf::from(state_root);
    let target_ecu = match required_str(target_ecu, "target ECU")? {
        "engine" => Ecu::Engine,
        "brake" => Ecu::Brake,
        "steering" => Ecu::Steering,
        _ => return Err("target ECU must be engine, brake, or steering".into()),
    };
    let config_schema = u32::try_from(parse_number(config_schema, "config schema")?)
        .map_err(|_| "config schema is out of range".to_owned())?;
    let signal_if_version =
        u32::try_from(parse_number(signal_if_version, "signal interface version")?)
            .map_err(|_| "signal interface version is out of range".to_owned())?;
    let trial_boots = if let Some(value) = trial_boots {
        u32::try_from(parse_number(value, "trial boot count")?)
            .map_err(|_| "trial boot count is out of range".to_owned())?
    } else {
        3
    };
    let trusted_keys = load_trust_keys()?;
    let validation_keys: Vec<_> = trusted_keys.iter().map(|key| key.as_trust_key()).collect();

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
    let mut installer = FotaInstaller {
        storage: FileSlotStorage::new(&root),
        boot_control,
        trial_boots,
    };
    let mut manifest_buffer = vec![0u8; MAX_MANIFEST_BYTES];
    let slot = installer
        .stage_package_from_source(
            &mut source,
            &mut manifest_buffer,
            &validation_keys,
            &context,
        )
        .map_err(|error| format!("package installation failed: {error:?}"))?;
    println!("staged trial image in slot {}", slot_name(slot));
    Ok(())
}

fn load_trust_keys() -> Result<Vec<sdv_fota::trust_store::TrustedKey>, String> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("SDV_FOTA_TEST_TRUST_STORE") {
        return sdv_fota::trust_store::load_for_development(path)
            .map_err(|error| format!("cannot load trust store: {error:?}"));
    }
    sdv_fota::trust_store::load(sdv_fota::trust_store::TRUST_STORE_PATH)
        .map_err(|error| format!("cannot load trust store: {error:?}"))
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
    "usage:\n  sdv-fota init <state-root> <active-slot:a|b> <active-version> <security-version>\n  sdv-fota install <state-root> <package-file> <ecu> <config-schema> <signal-if-version> [trial-boots]\n  sdv-fota install-url <state-root> <https-url> <ecu> <config-schema> <signal-if-version> [trial-boots]\n  sdv-fota status <state-root>".into()
}

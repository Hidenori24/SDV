use sdv_fota::linux::{FileBootControl, FilePackageSource, FileSlotStorage, HttpsPackageSource};
use sdv_fota::{
    BootControl, Ecu, FotaInstaller, PackageSource, Slot, ValidationContext, MAX_MANIFEST_BYTES,
};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

const DEFAULT_TRIAL_BOOTS: u32 = 3;

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
    let mut options = parse_named_options(args, &["--state-root"])?;
    let state_root = PathBuf::from(required_option(&mut options, "--state-root")?);
    let state = FileBootControl::new(state_root)
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
    let mut options = parse_named_options(
        args,
        &[
            "--state-root",
            "--active-slot",
            "--active-version",
            "--security-version",
        ],
    )?;
    let root = PathBuf::from(required_option(&mut options, "--state-root")?);
    let active_slot = required_option(&mut options, "--active-slot")?;
    let slot = match required_str(&active_slot, "active slot")? {
        "a" => Slot::A,
        "b" => Slot::B,
        _ => return Err("active slot must be `a` or `b`".into()),
    };
    let active_version = required_option(&mut options, "--active-version")?;
    let active_version = required_str(&active_version, "active version")?;
    let security_version = required_number(&mut options, "--security-version", "security version")?;
    FileBootControl::new(root)
        .initialize(slot, active_version, security_version)
        .map_err(|error| format!("boot metadata initialization failed: {error:?}"))
}

fn install(args: &[OsString]) -> Result<(), String> {
    install_from_options(args, PackageLocation::File)
}

fn install_url(args: &[OsString]) -> Result<(), String> {
    install_from_options(args, PackageLocation::Https)
}

#[derive(Clone, Copy)]
enum PackageLocation {
    File,
    Https,
}

fn install_from_options(
    args: &[OsString],
    package_location: PackageLocation,
) -> Result<(), String> {
    let source_option = match package_location {
        PackageLocation::File => "--package",
        PackageLocation::Https => "--url",
    };
    let accepted_options = match package_location {
        PackageLocation::File => &[
            "--state-root",
            "--package",
            "--target",
            "--config-schema",
            "--signal-if-version",
            "--trial-boots",
        ][..],
        PackageLocation::Https => &[
            "--state-root",
            "--url",
            "--target",
            "--config-schema",
            "--signal-if-version",
            "--trial-boots",
        ][..],
    };
    let mut options = parse_named_options(args, accepted_options)?;
    let state_root = PathBuf::from(required_option(&mut options, "--state-root")?);
    let source_value = required_option(&mut options, source_option)?;
    let target_ecu = required_option(&mut options, "--target")?;
    let target_ecu = parse_vehicle_target(&target_ecu)?;
    let config_schema = required_u32(&mut options, "--config-schema", "config schema")?;
    let signal_if_version = required_u32(
        &mut options,
        "--signal-if-version",
        "signal interface version",
    )?;
    let trial_boots = optional_u32(&mut options, "--trial-boots", "trial boot count")?
        .unwrap_or(DEFAULT_TRIAL_BOOTS);

    match package_location {
        PackageLocation::File => {
            let source = FilePackageSource::open(PathBuf::from(source_value))
                .map_err(|error| format!("cannot open package: {error:?}"))?;
            install_source(
                state_root,
                target_ecu,
                config_schema,
                signal_if_version,
                trial_boots,
                source,
            )
        }
        PackageLocation::Https => {
            let url = required_str(&source_value, "HTTPS package URL")?;
            let source = HttpsPackageSource::open(url)
                .map_err(|error| format!("cannot start HTTPS package download: {error:?}"))?;
            install_source(
                state_root,
                target_ecu,
                config_schema,
                signal_if_version,
                trial_boots,
                source,
            )
        }
    }
}

fn install_source<P: PackageSource<Error = sdv_fota::linux::Error>>(
    state_root: PathBuf,
    target_ecu: Ecu,
    config_schema: u32,
    signal_if_version: u32,
    trial_boots: u32,
    mut source: P,
) -> Result<(), String> {
    let trusted_keys = load_trust_keys()?;
    let validation_keys: Vec<_> = trusted_keys.iter().map(|key| key.as_trust_key()).collect();

    let boot_control = FileBootControl::new(&state_root);
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
        storage: FileSlotStorage::new(&state_root),
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

fn parse_named_options(
    args: &[OsString],
    accepted_options: &[&str],
) -> Result<BTreeMap<String, OsString>, String> {
    let mut options = BTreeMap::new();
    let mut args = args.iter();
    while let Some(option) = args.next() {
        let option_name = option
            .to_str()
            .ok_or_else(|| format!("option name must be valid UTF-8\n{}", usage()))?;
        if !accepted_options.contains(&option_name) {
            return Err(format!("unknown option: {option_name}\n{}", usage()));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {option_name}\n{}", usage()))?;
        if options
            .insert(option_name.to_owned(), value.to_owned())
            .is_some()
        {
            return Err(format!("option supplied more than once: {option_name}"));
        }
    }
    Ok(options)
}

fn required_option(
    options: &mut BTreeMap<String, OsString>,
    name: &str,
) -> Result<OsString, String> {
    options
        .remove(name)
        .ok_or_else(|| format!("missing required option {name}\n{}", usage()))
}

fn optional_option(options: &mut BTreeMap<String, OsString>, name: &str) -> Option<OsString> {
    options.remove(name)
}

fn required_number(
    options: &mut BTreeMap<String, OsString>,
    option_name: &str,
    label: &str,
) -> Result<u64, String> {
    let value = required_option(options, option_name)?;
    parse_number(&value, label)
}

fn required_u32(
    options: &mut BTreeMap<String, OsString>,
    option_name: &str,
    label: &str,
) -> Result<u32, String> {
    u32::try_from(required_number(options, option_name, label)?)
        .map_err(|_| format!("{label} is out of range"))
}

fn optional_u32(
    options: &mut BTreeMap<String, OsString>,
    option_name: &str,
    label: &str,
) -> Result<Option<u32>, String> {
    optional_option(options, option_name)
        .map(|value| {
            u32::try_from(parse_number(&value, label)?)
                .map_err(|_| format!("{label} is out of range"))
        })
        .transpose()
}

fn parse_vehicle_target(value: &OsStr) -> Result<Ecu, String> {
    match required_str(value, "target ECU")? {
        "vehicle" => Ok(Ecu::Vehicle),
        "engine" | "brake" | "steering" => Err(
            "Pi Linux A/B installer accepts only the complete `vehicle` image; per-ECU slots are not configured".into(),
        ),
        _ => Err("target must be vehicle".into()),
    }
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

fn parse_number(value: &OsStr, label: &str) -> Result<u64, String> {
    required_str(value, label)?
        .parse()
        .map_err(|_| format!("invalid {label}"))
}

fn required_str<'a>(value: &'a OsStr, label: &str) -> Result<&'a str, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{label} must be valid UTF-8"))
}

fn slot_name(slot: Slot) -> &'static str {
    match slot {
        Slot::A => "A",
        Slot::B => "B",
    }
}

fn usage() -> String {
    "usage:\n  sdv-fota init --state-root <dir> --active-slot <a|b> --active-version <version> --security-version <n>\n  sdv-fota install --state-root <dir> --package <file> --target vehicle --config-schema <n> --signal-if-version <n> [--trial-boots <n>]\n  sdv-fota install-url --state-root <dir> --url <https-url> --target vehicle --config-schema <n> --signal-if-version <n> [--trial-boots <n>]\n  sdv-fota status --state-root <dir>".into()
}

#[cfg(test)]
mod tests {
    use super::parse_named_options;
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    #[test]
    fn named_options_are_order_independent_and_preserve_path_values() {
        let parsed = parse_named_options(
            &[
                "--package".into(),
                "/tmp/пакет.sdvf".into(),
                "--state-root".into(),
                "/var/lib/sdv-fota".into(),
            ],
            &["--state-root", "--package"],
        )
        .unwrap();
        let expected = BTreeMap::from([
            ("--package".to_owned(), OsString::from("/tmp/пакет.sdvf")),
            (
                "--state-root".to_owned(),
                OsString::from("/var/lib/sdv-fota"),
            ),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn named_options_reject_duplicates_unknowns_positionals_and_missing_values() {
        let accepted = &["--state-root"][..];
        assert!(parse_named_options(
            &[
                "--state-root".into(),
                "a".into(),
                "--state-root".into(),
                "b".into()
            ],
            accepted
        )
        .is_err());
        assert!(parse_named_options(&["--other".into(), "a".into()], accepted).is_err());
        assert!(parse_named_options(&["positional".into()], accepted).is_err());
        assert!(parse_named_options(&["--state-root".into()], accepted).is_err());
    }
}

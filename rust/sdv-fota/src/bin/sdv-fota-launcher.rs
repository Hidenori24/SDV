use sdv_fota::linux::{
    run_active_target_image_as_with_groups_and_app_args, run_selected_image_as_with_groups,
    run_selected_target_image_as_with_groups_and_app_args,
    run_trial_candidate_target_image_as_with_groups_and_app_args,
};
use sdv_fota::Ecu;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

enum Invocation {
    Positional {
        state_root: PathBuf,
        health_program: PathBuf,
        health_args: Vec<OsString>,
    },
    Flags {
        base_root: PathBuf,
        target: Ecu,
        health_program: Option<PathBuf>,
        health_args: Vec<OsString>,
        app_args: Vec<OsString>,
        trial_candidate: bool,
    },
}

enum LauncherError {
    Usage(String),
    Launch(String),
}

fn main() {
    match run(std::env::args_os().skip(1)) {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(LauncherError::Usage(error)) => {
            eprintln!("sdv-fota-launcher: {error}");
            std::process::exit(2);
        }
        Err(LauncherError::Launch(error)) => {
            eprintln!("sdv-fota-launcher: {error}");
            std::process::exit(1);
        }
    }
}

fn run(
    args: impl IntoIterator<Item = OsString>,
) -> Result<std::process::ExitStatus, LauncherError> {
    let invocation = parse_invocation(args).map_err(LauncherError::Usage)?;
    let target = match &invocation {
        Invocation::Positional { .. } => Ecu::Vehicle,
        Invocation::Flags { target, .. } => *target,
    };
    let (uid, gid, groups) = configured_identity(target).map_err(LauncherError::Usage)?;

    let status = match invocation {
        Invocation::Positional {
            state_root,
            health_program,
            health_args,
        } => run_selected_image_as_with_groups(
            state_root,
            &health_program,
            &health_args,
            uid,
            gid,
            &groups,
        ),
        Invocation::Flags {
            base_root,
            target,
            health_program,
            health_args,
            app_args,
            trial_candidate,
        } => match target {
            Ecu::Vehicle if !trial_candidate => {
                run_selected_target_image_as_with_groups_and_app_args(
                    base_root,
                    target,
                    health_program
                        .as_deref()
                        .expect("vehicle invocation requires a health program"),
                    &health_args,
                    uid,
                    gid,
                    &groups,
                    &app_args,
                )
            }
            Ecu::Vehicle => Err(sdv_fota::linux::Error::InvalidTargetLaunchMode),
            Ecu::Engine | Ecu::Brake | Ecu::Steering if trial_candidate => {
                run_trial_candidate_target_image_as_with_groups_and_app_args(
                    base_root, target, uid, gid, &groups, &app_args,
                )
            }
            Ecu::Engine | Ecu::Brake | Ecu::Steering => {
                run_active_target_image_as_with_groups_and_app_args(
                    base_root, target, uid, gid, &groups, &app_args,
                )
            }
        },
    }
    .map_err(|error| LauncherError::Launch(format!("{error:?}")))?;
    Ok(status)
}

fn parse_invocation(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    let args: Vec<_> = args.into_iter().collect();
    if args
        .first()
        .is_some_and(|value| value.to_str().is_some_and(|value| value.starts_with("--")))
    {
        parse_flag_invocation(&args)
    } else {
        parse_positional_invocation(&args)
    }
}

fn parse_positional_invocation(args: &[OsString]) -> Result<Invocation, String> {
    if args.len() < 2 {
        return Err(usage());
    }
    Ok(Invocation::Positional {
        state_root: PathBuf::from(&args[0]),
        health_program: PathBuf::from(&args[1]),
        health_args: args[2..].to_vec(),
    })
}

fn parse_flag_invocation(args: &[OsString]) -> Result<Invocation, String> {
    let mut state_root = None;
    let mut target = None;
    let mut health_program = None;
    let mut health_args = Vec::new();
    let mut app_args = Vec::new();
    let mut trial_candidate = false;
    let mut args = args.iter();

    while let Some(option) = args.next() {
        let option = option
            .to_str()
            .ok_or_else(|| format!("option must be valid UTF-8\n{}", usage()))?;
        if option == "--trial-candidate" {
            if trial_candidate {
                return Err(format!(
                    "option supplied more than once: {option}\n{}",
                    usage()
                ));
            }
            trial_candidate = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {option}\n{}", usage()))?;
        match option {
            "--state-root" => set_once(&mut state_root, value.clone(), option)?,
            "--target" => set_once(&mut target, parse_target(value)?, option)?,
            "--health-program" => set_once(&mut health_program, value.clone(), option)?,
            "--health-arg" => health_args.push(value.clone()),
            "--app-arg" => app_args.push(value.clone()),
            _ => return Err(format!("unknown option: {option}\n{}", usage())),
        }
    }

    let target = required(target, "--target")?;
    let health_program = health_program.map(PathBuf::from);
    match target {
        Ecu::Vehicle if health_program.is_none() => {
            return Err(format!(
                "vehicle target requires --health-program\n{}",
                usage()
            ));
        }
        Ecu::Engine | Ecu::Brake | Ecu::Steering
            if health_program.is_some() || !health_args.is_empty() =>
        {
            return Err(format!(
                "ECU targets are active-only and do not accept health-check options\n{}",
                usage()
            ));
        }
        _ => {}
    }
    if trial_candidate && target == Ecu::Vehicle {
        return Err(format!(
            "--trial-candidate requires an ECU target\n{}",
            usage()
        ));
    }
    Ok(Invocation::Flags {
        base_root: PathBuf::from(required(state_root, "--state-root")?),
        target,
        health_program,
        health_args,
        app_args,
        trial_candidate,
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!(
            "option supplied more than once: {name}\n{}",
            usage()
        ));
    }
    Ok(())
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("missing required option {name}\n{}", usage()))
}

fn parse_target(value: &OsStr) -> Result<Ecu, String> {
    match value.to_str() {
        Some("vehicle") => Ok(Ecu::Vehicle),
        Some("engine") => Ok(Ecu::Engine),
        Some("brake") => Ok(Ecu::Brake),
        Some("steering") => Ok(Ecu::Steering),
        _ => Err(format!(
            "--target must be one of vehicle, engine, brake, or steering\n{}",
            usage()
        )),
    }
}

fn identity_env_names(target: Ecu) -> (&'static str, &'static str, &'static str) {
    match target {
        Ecu::Vehicle => (
            "SDV_FOTA_APP_UID",
            "SDV_FOTA_APP_GID",
            "SDV_FOTA_APP_GROUPS",
        ),
        Ecu::Engine | Ecu::Brake | Ecu::Steering => (
            "SDV_FOTA_ECU_APP_UID",
            "SDV_FOTA_ECU_APP_GID",
            "SDV_FOTA_ECU_APP_GROUPS",
        ),
    }
}

fn configured_identity(target: Ecu) -> Result<(u32, u32, Vec<u32>), String> {
    let (uid_name, gid_name, groups_name) = identity_env_names(target);
    Ok((
        configured_id(uid_name)?,
        configured_id(gid_name)?,
        configured_groups(std::env::var_os(groups_name), groups_name)?,
    ))
}

fn configured_groups(value: Option<OsString>, name: &str) -> Result<Vec<u32>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let value = value
        .into_string()
        .map_err(|_| format!("{name} must be valid UTF-8"))?;
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|part| match part.trim().parse::<u32>() {
            Ok(id) if id != 0 => Ok(id),
            _ => Err(format!("{name} must be comma-separated non-root group IDs")),
        })
        .collect()
}

fn configured_id(name: &str) -> Result<u32, String> {
    match std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
    {
        Some(id) if id != 0 => Ok(id),
        Some(_) => Err(format!(
            "{name} must identify a non-root application account"
        )),
        None => Err(format!(
            "{name} must be set to a numeric application UID/GID"
        )),
    }
}

fn usage() -> String {
    "usage:\n  sdv-fota-launcher <state-root> <health-check-program> [health-check-args...]\n  sdv-fota-launcher --state-root <base-dir> --target vehicle --health-program <path> [--health-arg <arg> ...] [--app-arg <arg> ...]\n  sdv-fota-launcher --state-root <base-dir> --target <engine|brake|steering> [--app-arg <arg> ...] (active image only)\n  sdv-fota-launcher --state-root <base-dir> --target <engine|brake|steering> --trial-candidate [--app-arg <arg> ...] (pending image, shadow only; no metadata changes)".to_owned()
}

#[cfg(test)]
mod tests {
    use super::{configured_groups, identity_env_names, parse_invocation, Invocation};
    use sdv_fota::Ecu;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn vehicle_and_ecu_targets_use_separate_identity_environment_names() {
        assert_eq!(
            identity_env_names(Ecu::Vehicle),
            (
                "SDV_FOTA_APP_UID",
                "SDV_FOTA_APP_GID",
                "SDV_FOTA_APP_GROUPS"
            )
        );
        assert_eq!(
            identity_env_names(Ecu::Engine),
            (
                "SDV_FOTA_ECU_APP_UID",
                "SDV_FOTA_ECU_APP_GID",
                "SDV_FOTA_ECU_APP_GROUPS"
            )
        );
        assert_eq!(
            identity_env_names(Ecu::Brake),
            identity_env_names(Ecu::Engine)
        );
        assert_eq!(
            identity_env_names(Ecu::Steering),
            identity_env_names(Ecu::Engine)
        );
    }

    #[test]
    fn missing_ecu_groups_default_to_empty() {
        assert_eq!(
            configured_groups(None, "SDV_FOTA_ECU_APP_GROUPS").unwrap(),
            Vec::<u32>::new()
        );
        assert_eq!(
            configured_groups(Some("42, 43".into()), "SDV_FOTA_ECU_APP_GROUPS").unwrap(),
            [42, 43]
        );
    }

    #[test]
    fn positional_invocation_preserves_the_legacy_arguments() {
        let invocation = parse_invocation([
            "/var/lib/sdv-fota".into(),
            "/usr/bin/health".into(),
            "--socket".into(),
            "/run/sdv-picarx/health.sock".into(),
        ])
        .unwrap();
        match invocation {
            Invocation::Positional {
                state_root,
                health_program,
                health_args,
            } => {
                assert_eq!(state_root, PathBuf::from("/var/lib/sdv-fota"));
                assert_eq!(health_program, PathBuf::from("/usr/bin/health"));
                assert_eq!(health_args, ["--socket", "/run/sdv-picarx/health.sock"]);
            }
            Invocation::Flags { .. } => panic!("legacy invocation changed modes"),
        }
    }

    #[test]
    fn flag_invocation_preserves_each_app_argument_and_uses_a_closed_target_enum() {
        let suspicious_args = [
            "--ecu",
            "engine",
            "one argument with spaces",
            "; touch /tmp/should-not-run",
            "$(touch /tmp/should-not-run)",
            "`touch /tmp/should-not-run`",
        ];
        let mut args: Vec<OsString> = [
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "engine".into(),
        ]
        .into();
        for argument in suspicious_args {
            args.push("--app-arg".into());
            args.push(argument.into());
        }

        let invocation = parse_invocation(args).unwrap();
        match invocation {
            Invocation::Flags {
                target,
                health_program,
                health_args,
                app_args,
                ..
            } => {
                assert_eq!(target, Ecu::Engine);
                assert!(health_program.is_none());
                assert!(health_args.is_empty());
                assert_eq!(app_args, suspicious_args);
            }
            Invocation::Positional { .. } => panic!("flag invocation changed modes"),
        }

        assert!(parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "ecu/../../vehicle".into(),
        ])
        .is_err());
    }

    #[test]
    fn vehicle_flag_invocation_keeps_trial_health_options() {
        let invocation = parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "vehicle".into(),
            "--health-program".into(),
            "/usr/bin/vehicle-health".into(),
            "--health-arg".into(),
            "--socket".into(),
        ])
        .unwrap();
        match invocation {
            Invocation::Flags {
                target,
                health_program,
                health_args,
                ..
            } => {
                assert_eq!(target, Ecu::Vehicle);
                assert_eq!(
                    health_program,
                    Some(PathBuf::from("/usr/bin/vehicle-health"))
                );
                assert_eq!(health_args, ["--socket"]);
            }
            Invocation::Positional { .. } => panic!("flag invocation changed modes"),
        }
    }

    #[test]
    fn ecu_flag_invocation_rejects_vehicle_health_options() {
        assert!(parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "engine".into(),
            "--health-program".into(),
            "/usr/bin/sdv-picarx-health".into(),
        ])
        .is_err());
        assert!(parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "brake".into(),
            "--health-arg".into(),
            "anything".into(),
        ])
        .is_err());
    }

    #[test]
    fn trial_candidate_mode_is_explicit_and_ecu_only() {
        let invocation = parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "engine".into(),
            "--trial-candidate".into(),
            "--app-arg".into(),
            "--ecu".into(),
            "--app-arg".into(),
            "engine".into(),
        ])
        .unwrap();
        match invocation {
            Invocation::Flags {
                trial_candidate,
                app_args,
                ..
            } => {
                assert!(trial_candidate);
                assert_eq!(app_args, ["--ecu", "engine"]);
            }
            Invocation::Positional { .. } => panic!("candidate mode used positional CLI"),
        }
        assert!(parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "vehicle".into(),
            "--health-program".into(),
            "/usr/bin/health".into(),
            "--trial-candidate".into(),
        ])
        .is_err());
        assert!(parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "brake".into(),
            "--trial-candidate".into(),
            "--trial-candidate".into(),
        ])
        .is_err());
    }

    #[test]
    fn flag_invocation_rejects_duplicate_unknown_and_missing_options() {
        assert!(parse_invocation(["--unknown".into(), "value".into()]).is_err());
        assert!(parse_invocation(["--state-root".into()]).is_err());
        assert!(parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--state-root".into(),
            "/other".into(),
        ])
        .is_err());
        assert!(parse_invocation([
            "--state-root".into(),
            "/var/lib/sdv-fota".into(),
            "--target".into(),
            "vehicle".into(),
        ])
        .is_err());
    }
}

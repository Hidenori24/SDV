use sdv_fota::linux::run_selected_image_as_with_groups;
use std::ffi::OsString;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args_os();
    let program = args
        .next()
        .unwrap_or_else(|| OsString::from("sdv-fota-launcher"));
    let Some(root) = args.next() else {
        usage(&program);
    };
    let Some(health_program) = args.next() else {
        usage(&program);
    };
    let health_args: Vec<OsString> = args.collect();
    let uid = configured_id("SDV_FOTA_APP_UID");
    let gid = configured_id("SDV_FOTA_APP_GID");
    let groups = configured_groups();

    match run_selected_image_as_with_groups(
        PathBuf::from(root),
        PathBuf::from(health_program).as_path(),
        &health_args,
        uid,
        gid,
        &groups,
    ) {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("sdv-fota-launcher: {error:?}");
            std::process::exit(1);
        }
    }
}

fn configured_groups() -> Vec<u32> {
    let Ok(value) = std::env::var("SDV_FOTA_APP_GROUPS") else {
        return Vec::new();
    };
    if value.trim().is_empty() {
        return Vec::new();
    }
    value
        .split(',')
        .map(|part| match part.trim().parse::<u32>() {
            Ok(id) if id != 0 => id,
            _ => {
                eprintln!("sdv-fota-launcher: SDV_FOTA_APP_GROUPS must be comma-separated non-root group IDs");
                std::process::exit(2);
            }
        })
        .collect()
}

fn configured_id(name: &str) -> u32 {
    match std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
    {
        Some(id) if id != 0 => id,
        Some(_) => {
            eprintln!("sdv-fota-launcher: {name} must identify a non-root application account");
            std::process::exit(2);
        }
        None => {
            eprintln!("sdv-fota-launcher: {name} must be set to a numeric application UID/GID");
            std::process::exit(2);
        }
    }
}

fn usage(program: &OsString) -> ! {
    eprintln!(
        "usage: {} <state-root> <health-check-program> [health-check-args... ]",
        PathBuf::from(program).display()
    );
    std::process::exit(2);
}

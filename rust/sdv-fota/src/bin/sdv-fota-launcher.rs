use sdv_fota::linux::run_selected_image;
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

    match run_selected_image(
        PathBuf::from(root),
        PathBuf::from(health_program).as_path(),
        &health_args,
    ) {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("sdv-fota-launcher: {error:?}");
            std::process::exit(1);
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

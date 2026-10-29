use std::env;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("sdv-picarx-health: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_default();
    let socket = match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--socket" => PathBuf::from(path),
        _ => {
            return Err(format!(
                "usage: {} --socket <health-socket>",
                PathBuf::from(program).display()
            ))
        }
    };
    check_health(&socket)
}

fn check_health(socket: &Path) -> Result<(), String> {
    let stream = UnixStream::connect(socket)
        .map_err(|error| format!("cannot connect to {}: {error}", socket.display()))?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(|error| format!("cannot read health response: {error}"))?;
    if response == "READY\n" {
        Ok(())
    } else {
        Err("vehicle runtime has not completed a recent control cycle".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn accepts_only_the_ready_health_response() {
        for (response, expected) in [("READY\n", true), ("NOT_READY\n", false)] {
            let path = std::env::temp_dir().join(format!(
                "sdv-picarx-health-client-{}-{}.sock",
                std::process::id(),
                NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
            ));
            let listener = UnixListener::bind(&path).unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(response.as_bytes()).unwrap();
            });
            assert_eq!(check_health(&path).is_ok(), expected);
            server.join().unwrap();
            let _ = std::fs::remove_file(path);
        }
    }
}

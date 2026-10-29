use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};

fn main() {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    if let Err(error) = report(&mut output) {
        eprintln!("sdv-picarx-discover: {error}");
        std::process::exit(1);
    }
}

fn report(output: &mut impl Write) -> io::Result<()> {
    writeln!(output, "I2C devices:")?;
    let mut i2c_devices = fs::read_dir("/dev")?
        .filter_map(Result::ok)
        .filter(|entry| is_i2c_device(entry.file_name().as_os_str()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    i2c_devices.sort();
    if i2c_devices.is_empty() {
        writeln!(output, "  none found")?;
    } else {
        for device in i2c_devices {
            writeln!(output, "  {}", device.display())?;
        }
    }

    writeln!(output, "GPIO chips and line offsets:")?;
    let chips = gpiocdev::chip::chips().map_err(io::Error::other)?;
    if chips.is_empty() {
        writeln!(output, "  none found")?;
    }
    for path in chips {
        let chip = gpiocdev::Chip::from_path(&path).map_err(io::Error::other)?;
        let info = chip.info().map_err(io::Error::other)?;
        writeln!(
            output,
            "  {}: {} ({}, {} lines)",
            path.display(),
            info.name,
            info.label,
            info.num_lines
        )?;
        for line in chip.line_info_iter().map_err(io::Error::other)? {
            let line = line.map_err(io::Error::other)?;
            let name = if line.name.is_empty() {
                "<unnamed>"
            } else {
                line.name.as_str()
            };
            let consumer = if line.consumer.is_empty() {
                "unused"
            } else {
                line.consumer.as_str()
            };
            writeln!(
                output,
                "    offset {}: {} [{}; {:?}]",
                line.offset, name, consumer, line.direction
            )?;
        }
    }
    writeln!(output, "No pins were requested or changed.")
}

fn is_i2c_device(name: &OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix("i2c-"))
        .is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_numeric_i2c_device_names() {
        assert!(is_i2c_device(OsStr::new("i2c-1")));
        assert!(is_i2c_device(OsStr::new("i2c-12")));
        assert!(!is_i2c_device(OsStr::new("i2c-")));
        assert!(!is_i2c_device(OsStr::new("i2c-foo")));
        assert!(!is_i2c_device(OsStr::new("gpiochip0")));
    }
}

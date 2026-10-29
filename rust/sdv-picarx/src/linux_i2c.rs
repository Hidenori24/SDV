//! Linux i2c-dev transport for Robot HAT register writes.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_ulong};
use std::path::Path;

use crate::pwm::RegisterTransport;

// Linux UAPI: linux/i2c-dev.h, selecting the 7-bit device address on this file descriptor.
const I2C_SLAVE: c_ulong = 0x0703;
const MIN_DEVICE_ADDRESS: u16 = 0x03;
const MAX_DEVICE_ADDRESS: u16 = 0x77;

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

pub struct I2cTransport {
    device: File,
}

impl I2cTransport {
    /// Open an explicitly configured i2c-dev path and select a 7-bit MCU address.
    pub fn open(path: impl AsRef<Path>, address: u16) -> io::Result<Self> {
        if !(MIN_DEVICE_ADDRESS..=MAX_DEVICE_ADDRESS).contains(&address) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid 7-bit I2C device address",
            ));
        }
        let device = OpenOptions::new().read(true).write(true).open(path)?;
        // SAFETY: the file descriptor is open, and I2C_SLAVE takes an integer address argument.
        if unsafe { ioctl(device.as_raw_fd(), I2C_SLAVE, c_ulong::from(address)) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { device })
    }
}

impl RegisterTransport for I2cTransport {
    type Error = io::Error;

    fn write(&mut self, bytes: [u8; 3]) -> io::Result<()> {
        // A single write represents one I2C transaction; never retry a partial write as a
        // second transaction because it would lose the register prefix.
        match self.device.write(&bytes)? {
            n if n == bytes.len() => Ok(()),
            _ => Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short I2C register write",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_address_is_rejected_before_opening_device() {
        let result = I2cTransport::open("/nonexistent", 0x80);
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn non_i2c_device_does_not_silently_succeed() {
        assert!(I2cTransport::open("/dev/null", 0x14).is_err());
    }
}

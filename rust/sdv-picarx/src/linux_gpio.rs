//! Linux GPIO character-device port for Robot HAT motor direction lines.

use std::path::Path;

use gpiocdev::line::Value;
use gpiocdev::Request;

use crate::robothat::MotorDirections;

#[derive(Debug)]
pub enum Error {
    DuplicateLineOffsets,
    Gpio(gpiocdev::Error),
}

impl From<gpiocdev::Error> for Error {
    fn from(error: gpiocdev::Error) -> Self {
        Self::Gpio(error)
    }
}

/// Owns two requested output lines for the lifetime of the motor direction port.
///
/// The GPIO chip path and line offsets are explicit because chip numbering can vary across
/// Raspberry Pi OS images and attached GPIO controllers. Lines start inactive as part of the
/// kernel request, before this object is returned.
pub struct LinuxMotorDirections {
    request: Request,
    left_offset: u32,
    right_offset: u32,
}

impl LinuxMotorDirections {
    pub fn open(
        chip_path: impl AsRef<Path>,
        left_offset: u32,
        right_offset: u32,
    ) -> Result<Self, Error> {
        if left_offset == right_offset {
            return Err(Error::DuplicateLineOffsets);
        }

        let offsets = [left_offset, right_offset];
        let mut builder = Request::builder();
        builder
            .on_chip(chip_path.as_ref())
            .with_lines(&offsets)
            .as_output(Value::Inactive)
            .with_consumer("sdv-picarx");
        let request = builder.request()?;

        Ok(Self {
            request,
            left_offset,
            right_offset,
        })
    }
}

impl MotorDirections for LinuxMotorDirections {
    type Error = Error;

    fn set_left(&mut self, high: bool) -> Result<(), Self::Error> {
        self.request
            .set_value(self.left_offset, Value::from(high))
            .map_err(Error::Gpio)
    }

    fn set_right(&mut self, high: bool) -> Result<(), Self::Error> {
        self.request
            .set_value(self.right_offset, Value::from(high))
            .map_err(Error::Gpio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_the_same_line_for_both_motors_before_opening_chip() {
        assert!(matches!(
            LinuxMotorDirections::open("/no/such/gpiochip", 23, 23),
            Err(Error::DuplicateLineOffsets)
        ));
    }

    #[test]
    fn reports_a_missing_gpiochip() {
        assert!(matches!(
            LinuxMotorDirections::open("/no/such/gpiochip", 23, 24),
            Err(Error::Gpio(_))
        ));
    }
}

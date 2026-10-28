//! Robot HAT MCU PWM register transport, independent of Linux I2C.
//!
//! Register assignments follow SunFounder's Robot HAT PWM interface. Channels share timers;
//! callers must coordinate frequency and period before changing any shared timer.

const CHANNEL_REGISTER_BASE: u8 = 0x20;
const PRESCALER_REGISTER_BASE: u8 = 0x40;
const PERIOD_REGISTER_BASE: u8 = 0x44;
const EXTENDED_PRESCALER_BASE: u8 = 0x50;
const EXTENDED_PERIOD_BASE: u8 = 0x54;
const MAX_CHANNEL: u8 = 19;
const CHANNELS_PER_TIMER: u8 = 4;
const FIRST_EXTENDED_CHANNEL: u8 = 16;
const FIRST_EXTENDED_TIMER: u8 = 4;

/// Send one complete three-byte I2C message to the Robot HAT MCU.
pub trait RegisterTransport {
    type Error;
    fn write(&mut self, bytes: [u8; 3]) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq)]
pub enum Error<E> {
    InvalidChannel,
    InvalidPrescaler,
    InvalidPeriod,
    PulseExceedsPeriod,
    Transport(E),
}

pub struct Pwm<T> {
    transport: T,
}

impl<T: RegisterTransport> Pwm<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Configure the timer shared by this channel and its peers. Prescaler and period are
    /// nonzero hardware counts; the MCU's prescaler register encodes count minus one.
    pub fn configure_timer(
        &mut self,
        channel: u8,
        prescaler: u16,
        period: u16,
    ) -> Result<(), Error<T::Error>> {
        let timer = timer_for(channel).ok_or(Error::InvalidChannel)?;
        if prescaler == 0 {
            return Err(Error::InvalidPrescaler);
        }
        if period == 0 {
            return Err(Error::InvalidPeriod);
        }
        let (psc, arr) = if timer < FIRST_EXTENDED_TIMER {
            (
                PRESCALER_REGISTER_BASE + timer,
                PERIOD_REGISTER_BASE + timer,
            )
        } else {
            (
                EXTENDED_PRESCALER_BASE + timer - FIRST_EXTENDED_TIMER,
                EXTENDED_PERIOD_BASE + timer - FIRST_EXTENDED_TIMER,
            )
        };
        self.write_value(psc, prescaler - 1)?;
        self.write_value(arr, period)
    }

    pub fn set_pulse(
        &mut self,
        channel: u8,
        pulse: u16,
        period: u16,
    ) -> Result<(), Error<T::Error>> {
        timer_for(channel).ok_or(Error::InvalidChannel)?;
        if period == 0 {
            return Err(Error::InvalidPeriod);
        }
        if pulse > period {
            return Err(Error::PulseExceedsPeriod);
        }
        self.write_value(CHANNEL_REGISTER_BASE + channel, pulse)
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    fn write_value(&mut self, register: u8, value: u16) -> Result<(), Error<T::Error>> {
        let [high, low] = value.to_be_bytes();
        self.transport
            .write([register, high, low])
            .map_err(Error::Transport)
    }
}

fn timer_for(channel: u8) -> Option<u8> {
    match channel {
        0..=15 => Some(channel / CHANNELS_PER_TIMER),
        FIRST_EXTENDED_CHANNEL..=17 => Some(FIRST_EXTENDED_TIMER),
        18 => Some(5),
        MAX_CHANNEL => Some(6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Capture(Vec<[u8; 3]>);
    impl RegisterTransport for Capture {
        type Error = ();
        fn write(&mut self, bytes: [u8; 3]) -> Result<(), ()> {
            self.0.push(bytes);
            Ok(())
        }
    }
    #[test]
    fn configures_shared_timer_and_writes_big_endian_pulse() {
        let mut pwm = Pwm::new(Capture::default());
        pwm.configure_timer(13, 10, 4095).unwrap();
        pwm.set_pulse(13, 0x123, 4095).unwrap();
        assert_eq!(
            pwm.into_transport().0,
            vec![[0x43, 0, 9], [0x47, 0x0f, 0xff], [0x2d, 1, 0x23]]
        );
    }
    #[test]
    fn rejects_invalid_commands_before_writing() {
        let mut pwm = Pwm::new(Capture::default());
        assert_eq!(pwm.configure_timer(20, 10, 100), Err(Error::InvalidChannel));
        assert_eq!(pwm.set_pulse(2, 101, 100), Err(Error::PulseExceedsPeriod));
        assert!(pwm.into_transport().0.is_empty());
    }
}

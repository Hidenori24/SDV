//! Monotonic host scheduling for the 10 ms virtual ECU cycle.

use std::time::{Duration, Instant};

use crate::platform::{CycleError, VehicleExecutor, VehiclePlatform};

pub const ECU_PERIOD: Duration = Duration::from_millis(10);

/// A monotonic clock used by the host loop. Times are relative to an arbitrary origin.
pub trait Clock {
    fn now(&self) -> Duration;
    fn wait_until(&mut self, deadline: Duration);
}

pub struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn wait_until(&mut self, deadline: Duration) {
        if let Some(remaining) = deadline.checked_sub(self.now()) {
            std::thread::sleep(remaining);
        }
    }
}

#[derive(Debug)]
pub enum RunError<E> {
    Cycle(CycleError<E>),
    DeadlineMissed { stop_error: Option<E> },
    ClockOverflow { stop_error: Option<E> },
    Stop(E),
}

/// Run until shutdown is requested. A missed cycle stops the platform rather than replaying
/// stale 10 ms cycles. `stop_requested` must return promptly and is polled once per cycle.
pub fn run<P, C, F>(
    executor: &mut VehicleExecutor<P>,
    clock: &mut C,
    mut stop_requested: F,
) -> Result<(), RunError<P::Error>>
where
    P: VehiclePlatform,
    C: Clock,
    F: FnMut() -> bool,
{
    let mut deadline = clock.now();
    loop {
        if stop_requested() {
            return executor.stop().map_err(RunError::Stop);
        }
        clock.wait_until(deadline);
        let next = match deadline.checked_add(ECU_PERIOD) {
            Some(next) => next,
            None => {
                return Err(RunError::ClockOverflow {
                    stop_error: executor.stop().err(),
                })
            }
        };
        if clock.now() >= next {
            return Err(RunError::DeadlineMissed {
                stop_error: executor.stop().err(),
            });
        }
        executor.cycle().map_err(RunError::Cycle)?;
        if clock.now() >= next {
            return Err(RunError::DeadlineMissed {
                stop_error: executor.stop().err(),
            });
        }
        deadline = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::PlatformInputs;
    use crate::runtime::{VehicleActuation, VehicleRuntime};
    use std::cell::Cell;
    use std::rc::Rc;

    struct FakeClock(Rc<Cell<Duration>>);
    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn wait_until(&mut self, deadline: Duration) {
            self.0.set(self.0.get().max(deadline));
        }
    }
    struct FakePlatform {
        clock: Rc<Cell<Duration>>,
        work: Duration,
        cycles: usize,
        stops: usize,
    }
    impl VehiclePlatform for FakePlatform {
        type Error = ();
        fn sample(&mut self) -> Result<PlatformInputs, ()> {
            Ok(PlatformInputs::default())
        }
        fn apply(&mut self, _: VehicleActuation) -> Result<(), ()> {
            self.cycles += 1;
            self.clock.set(self.clock.get() + self.work);
            Ok(())
        }
        fn stop(&mut self) -> Result<(), ()> {
            self.stops += 1;
            Ok(())
        }
    }

    #[test]
    fn runs_at_fixed_deadlines_and_stops_on_shutdown() {
        let time = Rc::new(Cell::new(Duration::ZERO));
        let mut clock = FakeClock(time.clone());
        let platform = FakePlatform {
            clock: time.clone(),
            work: Duration::ZERO,
            cycles: 0,
            stops: 0,
        };
        let mut executor = VehicleExecutor::new(VehicleRuntime::default(), platform);
        let mut polls = 0;
        run(&mut executor, &mut clock, || {
            polls += 1;
            polls > 3
        })
        .unwrap();
        assert_eq!(executor.platform().cycles, 3);
        assert_eq!(executor.platform().stops, 1);
        assert_eq!(time.get(), ECU_PERIOD * 2);
    }

    #[test]
    fn overrun_stops_without_replaying_cycles() {
        let time = Rc::new(Cell::new(Duration::ZERO));
        let mut clock = FakeClock(time.clone());
        let platform = FakePlatform {
            clock: time,
            work: ECU_PERIOD,
            cycles: 0,
            stops: 0,
        };
        let mut executor = VehicleExecutor::new(VehicleRuntime::default(), platform);
        assert!(matches!(
            run(&mut executor, &mut clock, || false),
            Err(RunError::DeadlineMissed { stop_error: None })
        ));
        assert_eq!(executor.platform().cycles, 1);
        assert_eq!(executor.platform().stops, 1);
    }
}

//! Sticky actuator output hold for ECU trial supervision.
//!
//! This latch has no release operation by design. A later transaction manager may add a
//! separately authorized, attempt-bound release after it can prove the production route.

use sdv_core::platform::VehiclePlatform;
use sdv_core::rte::ActuatorCmd;
use sdv_core::runtime::VehicleActuation;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct StickyOutputHold {
    requested: Arc<AtomicBool>,
    zero_apply_acknowledged: Arc<AtomicBool>,
    zero_apply_generation: Arc<AtomicU64>,
}

impl StickyOutputHold {
    pub fn new(initially_held: bool) -> Self {
        Self {
            requested: Arc::new(AtomicBool::new(initially_held)),
            zero_apply_acknowledged: Arc::new(AtomicBool::new(false)),
            zero_apply_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn is_held(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// Return the number of successfully applied zero-output cycles while the hold was latched.
    /// A consumer can snapshot this value before a critical operation and require it to advance
    /// before acknowledging that operation.
    pub fn zero_apply_generation(&self) -> u64 {
        self.zero_apply_generation.load(Ordering::Acquire)
    }

    /// Whether at least one successful zero-output apply has acknowledged the latched hold.
    pub fn has_zero_apply_acknowledgement(&self) -> bool {
        self.zero_apply_acknowledged.load(Ordering::Acquire)
    }

    /// Latch a hold and wait until a physical apply has acknowledged zero output.
    /// Repeated requests are idempotent and can never clear an existing hold.
    pub fn request_and_wait(&self, timeout: Duration) -> bool {
        // Publish the safety request without waiting on the platform's I/O path. An in-flight
        // nonzero apply cannot be changed, so no ACK is returned until a later zero apply has
        // completed successfully.
        self.requested.store(true, Ordering::Release);
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            if self.zero_apply_acknowledged.load(Ordering::Acquire) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            std::thread::sleep(
                Duration::from_millis(1).min(deadline.saturating_duration_since(now)),
            );
        }
    }

    /// Apply safe zero commands while held. A request racing with an in-flight nonzero apply
    /// receives no acknowledgement until a later successful zero apply.
    pub fn apply<P: VehiclePlatform>(
        &self,
        platform: &mut P,
        actuation: VehicleActuation,
    ) -> Result<(), P::Error> {
        let held = self.requested.load(Ordering::Acquire);
        let applied = if held {
            platform.apply(zero_command_preserving_safety(actuation))
        } else {
            platform.apply(actuation)
        };
        if applied.is_ok() && held {
            self.zero_apply_acknowledged.store(true, Ordering::Release);
            let _ = self.zero_apply_generation.try_update(
                Ordering::AcqRel,
                Ordering::Acquire,
                |generation| generation.checked_add(1),
            );
        }
        applied
    }
}

fn zero_command_preserving_safety(actuation: VehicleActuation) -> VehicleActuation {
    VehicleActuation {
        command: ActuatorCmd::default(),
        safety: actuation.safety,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdv_core::platform::PlatformInputs;
    use sdv_core::rte::{ActuatorCmd, Safety, SystemState};
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    #[derive(Default)]
    struct FakePlatform {
        applied: Option<VehicleActuation>,
    }

    impl VehiclePlatform for FakePlatform {
        type Error = Infallible;

        fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
            Ok(PlatformInputs::default())
        }

        fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error> {
            self.applied = Some(actuation);
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct ApplyFailure;

    struct FailingPlatform;

    impl VehiclePlatform for FailingPlatform {
        type Error = ApplyFailure;

        fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
            Ok(PlatformInputs::default())
        }

        fn apply(&mut self, _: VehicleActuation) -> Result<(), Self::Error> {
            Err(ApplyFailure)
        }

        fn stop(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct BlockingPlatform {
        entered: Option<std::sync::mpsc::Sender<()>>,
        release: std::sync::mpsc::Receiver<()>,
        applied: Arc<std::sync::Mutex<Vec<VehicleActuation>>>,
    }

    impl VehiclePlatform for BlockingPlatform {
        type Error = Infallible;

        fn sample(&mut self) -> Result<PlatformInputs, Self::Error> {
            Ok(PlatformInputs::default())
        }

        fn apply(&mut self, actuation: VehicleActuation) -> Result<(), Self::Error> {
            if let Some(entered) = self.entered.take() {
                let _ = entered.send(());
                let _ = self.release.recv();
            }
            self.applied.lock().unwrap().push(actuation);
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn moving_actuation() -> VehicleActuation {
        VehicleActuation {
            command: ActuatorCmd {
                drive_accel_cmd: 1.5,
                brake_decel_cmd: 0.0,
                steer_angle_cmd: -0.25,
            },
            safety: Safety {
                estop: false,
                system_state: SystemState::Normal,
            },
        }
    }

    #[test]
    fn initially_held_zeroes_each_command_and_preserves_safety() {
        let hold = StickyOutputHold::new(true);
        let mut platform = FakePlatform::default();

        hold.apply(&mut platform, moving_actuation()).unwrap();
        let applied = platform.applied.unwrap();
        assert_eq!(applied.command, ActuatorCmd::default());
        assert_eq!(applied.safety, moving_actuation().safety);
        assert!(hold.is_held());
        assert!(hold.request_and_wait(Duration::ZERO));
        assert_eq!(hold.zero_apply_generation(), 1);
    }

    #[test]
    fn hold_request_acknowledges_only_after_zero_apply_and_remains_latched() {
        let hold = StickyOutputHold::new(false);
        let waiting_hold = hold.clone();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_by_thread = Arc::clone(&completed);
        let waiter = thread::spawn(move || {
            let result = waiting_hold.request_and_wait(Duration::from_secs(1));
            completed_by_thread.store(result, Ordering::Release);
        });

        while !hold.is_held() {
            thread::yield_now();
        }
        assert!(!completed.load(Ordering::Acquire));
        let mut platform = FakePlatform::default();
        hold.apply(&mut platform, moving_actuation()).unwrap();
        waiter.join().unwrap();

        assert!(completed.load(Ordering::Acquire));
        assert_eq!(platform.applied.unwrap().command, ActuatorCmd::default());
        assert_eq!(hold.zero_apply_generation(), 1);
        hold.apply(&mut platform, moving_actuation()).unwrap();
        assert_eq!(platform.applied.unwrap().command, ActuatorCmd::default());
        assert!(hold.is_held());
        assert_eq!(hold.zero_apply_generation(), 2);
    }

    #[test]
    fn hold_request_times_out_but_never_releases_the_latch() {
        let hold = StickyOutputHold::new(false);
        assert!(!hold.request_and_wait(Duration::ZERO));
        assert!(hold.is_held());
        let mut platform = FakePlatform::default();
        hold.apply(&mut platform, moving_actuation()).unwrap();
        assert_eq!(platform.applied.unwrap().command, ActuatorCmd::default());
    }

    #[test]
    fn contended_request_returns_by_deadline_and_later_apply_observes_latch() {
        let hold = StickyOutputHold::new(false);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let applied = Arc::new(std::sync::Mutex::new(Vec::new()));
        let applied_by_thread = Arc::clone(&applied);
        let hold_by_thread = hold.clone();
        let worker = std::thread::spawn(move || {
            let mut platform = BlockingPlatform {
                entered: Some(entered_tx),
                release: release_rx,
                applied: applied_by_thread,
            };
            hold_by_thread
                .apply(&mut platform, moving_actuation())
                .unwrap();
            hold_by_thread
                .apply(&mut platform, moving_actuation())
                .unwrap();
        });
        entered_rx.recv().unwrap();
        let start = Instant::now();
        assert!(!hold.request_and_wait(Duration::from_millis(25)));
        assert!(start.elapsed() < Duration::from_millis(250));
        assert!(hold.is_held());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        let applied = applied.lock().unwrap();
        assert_eq!(applied[0].command, moving_actuation().command);
        assert_eq!(applied[1].command, ActuatorCmd::default());
    }

    #[test]
    fn failed_platform_apply_never_confirms_the_hold() {
        let hold = StickyOutputHold::new(false);
        let waiting_hold = hold.clone();
        let waiter =
            thread::spawn(move || waiting_hold.request_and_wait(Duration::from_millis(20)));
        while !hold.is_held() {
            thread::yield_now();
        }
        assert!(matches!(
            hold.apply(&mut FailingPlatform, moving_actuation()),
            Err(ApplyFailure)
        ));
        assert!(!waiter.join().unwrap());
        assert!(hold.is_held());
        assert_eq!(hold.zero_apply_generation(), 0);
    }
}

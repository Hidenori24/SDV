//! Trial application of a new config (ADR-0003 §5).
//!
//! After a config update the new config runs on probation for a window of simulated time. If the system goes
//! Degraded or EStop in that window, the update is judged to have failed and the host rolls back to the previous
//! config. Same idea as the FOTA trial (ADR-0005): "update, try, go back if it is not healthy".
//!
//! This is only the judgement (pure, no I/O, no clock): the caller feeds it one tick at a time and acts on the
//! returned event. Rolling back and rebooting is the caller's job.

use crate::rte::SystemState;

/// Why the trial failed (the system state that was seen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailReason {
    Degraded,
    EStop,
}

impl FailReason {
    /// Number carried in the output buffer for the host (stable).
    pub fn code(self) -> u32 {
        match self {
            FailReason::Degraded => 1,
            FailReason::EStop => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FailReason::Degraded => "degraded",
            FailReason::EStop => "estop",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialState {
    /// No trial.
    Idle,
    Running {
        window_ms: u32,
        remaining_ms: u32,
    },
    /// The window passed without trouble: the config stays.
    Passed,
    /// The trial failed and the host rolled back.
    RolledBack(FailReason),
}

impl TrialState {
    /// Number carried in the output buffer for the host (stable): 0 idle, 1 running, 2 passed, 3 rolled back.
    pub fn code(self) -> u32 {
        match self {
            TrialState::Idle => 0,
            TrialState::Running { .. } => 1,
            TrialState::Passed => 2,
            TrialState::RolledBack(_) => 3,
        }
    }
}

/// What a tick decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialEvent {
    None,
    Passed,
    Failed(FailReason),
}

/// After the driver lets go of the E-Stop the state still reads EStop until the Safety task (every 100 ms) runs
/// again: that stale state is not judged for this long.
pub const DRIVER_ESTOP_GRACE_MS: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigTrial {
    state: TrialState,
    /// How much longer a state that follows the driver's E-Stop is forgiven.
    driver_grace_ms: u32,
}

impl Default for ConfigTrial {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigTrial {
    /// No trial (usable in a `static`).
    pub const fn new() -> Self {
        Self {
            state: TrialState::Idle,
            driver_grace_ms: 0,
        }
    }

    pub fn state(&self) -> TrialState {
        self.state
    }

    /// Begin (or restart) a trial. A zero window cannot judge anything: see `try_start`.
    pub fn start(&mut self, window_ms: u32) {
        self.state = TrialState::Running {
            window_ms,
            remaining_ms: window_ms,
        };
        self.driver_grace_ms = 0;
    }

    /// Like `start`, but refuses an empty window and then changes nothing.
    pub fn try_start(&mut self, window_ms: u32) -> bool {
        if window_ms == 0 {
            return false;
        }
        self.start(window_ms);
        true
    }

    pub fn cancel(&mut self) {
        self.state = TrialState::Idle;
    }

    /// Judge one tick of `dt_ms`. `driver_estop` is true while the driver (UI / hardware / scenario) is asking for
    /// an E-Stop: the system is then in EStop on purpose, which says nothing about the config. For one Safety cycle
    /// after the driver lets go (`DRIVER_ESTOP_GRACE_MS`) the stale state is forgiven too; a watchdog EStop that is
    /// still there after that does count.
    pub fn tick(
        &mut self,
        dt_ms: u32,
        system_state: SystemState,
        driver_estop: bool,
    ) -> TrialEvent {
        let TrialState::Running {
            window_ms,
            remaining_ms,
        } = self.state
        else {
            return TrialEvent::None;
        };
        if driver_estop {
            self.driver_grace_ms = DRIVER_ESTOP_GRACE_MS;
        }
        let forgiven = driver_estop || self.driver_grace_ms > 0;
        if !driver_estop {
            self.driver_grace_ms = self.driver_grace_ms.saturating_sub(dt_ms);
        }
        let failure = match system_state {
            SystemState::Normal => None,
            _ if forgiven => None,
            SystemState::Degraded => Some(FailReason::Degraded),
            SystemState::EStop => Some(FailReason::EStop),
        };
        if let Some(reason) = failure {
            self.state = TrialState::RolledBack(reason);
            return TrialEvent::Failed(reason);
        }
        let remaining_ms = remaining_ms.saturating_sub(dt_ms);
        if remaining_ms == 0 {
            self.state = TrialState::Passed;
            TrialEvent::Passed
        } else {
            self.state = TrialState::Running {
                window_ms,
                remaining_ms,
            };
            TrialEvent::None
        }
    }
}

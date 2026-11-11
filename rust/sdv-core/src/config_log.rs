//! Log of the config lifecycle: which generation was applied when, and what became of a trial.
//!
//! It is a separate record on purpose: the simulation log (CSV) and its golden files stay as they are. The time
//! is the simulated time of the running system (the core has no wall clock); a host that has one adds its own.

use serde::Serialize;

/// Older entries are dropped beyond this.
pub const MAX_ENTRIES: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    /// Boot counter at the time of the event (the first boot is 1).
    pub boot: u32,
    /// Simulated time since that boot [s].
    pub t_s: f32,
    /// `boot`, `commit`, `rollback`, `trial_started`, `trial_passed`, `trial_failed`, `rolled_back`, `trial_cancelled`.
    pub event: &'static str,
    /// Generation of the Active config after the event (0 = the factory defaults).
    pub seq: u32,
    pub detail: String,
}

#[derive(Default)]
pub struct ConfigLog {
    boots: u32,
    entries: Vec<Entry>,
}

impl ConfigLog {
    /// An empty log (usable in a `static`).
    pub const fn new() -> Self {
        Self {
            boots: 0,
            entries: Vec::new(),
        }
    }

    /// Count a boot and return its number.
    pub fn next_boot(&mut self) -> u32 {
        self.boots += 1;
        self.boots
    }

    pub fn push(&mut self, t_s: f32, event: &'static str, seq: u32, detail: impl Into<String>) {
        if self.entries.len() == MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push(Entry {
            boot: self.boots,
            t_s,
            event,
            seq,
            detail: detail.into(),
        });
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.entries).expect("entries are always serializable")
    }
}

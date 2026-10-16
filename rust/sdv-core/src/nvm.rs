//! Pseudo NVM for the Config (ADR-0003): two slots (A/B), a sequence number and CRC32.
//!
//! ```text
//! slot Active : the config in use          [magic "SDVN" | seq u32 | len u32 | json | crc32]
//! slot Backup : the previous config
//! ```
//! - `commit`: only a config that passes validation is stored (`Backup <- Active`, `Active <- new`)
//! - `load`: Active if its CRC and content are valid, else Backup, else the built-in defaults
//! - `rollback`: `Active <- Backup`, Backup becomes empty (so it cannot be repeated)

use crate::config::{self, Config, ConfigError};

const MAGIC: &[u8; 4] = b"SDVN";
const HEADER: usize = 12; // magic + seq + len

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Slot {
    Active = 0,
    Backup = 1,
}

/// Storage backend. The memory implementation is used by the tests and the WASM build;
/// the browser persists the bytes in localStorage.
pub trait NvmStorage {
    fn read(&self, slot: Slot) -> Option<Vec<u8>>;
    fn write(&mut self, slot: Slot, bytes: Vec<u8>);
    fn erase(&mut self, slot: Slot);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemStorage {
    slots: [Option<Vec<u8>>; 2],
}

impl MemStorage {
    pub fn read(&self, slot: Slot) -> Option<Vec<u8>> {
        self.slots[slot as usize].clone()
    }
    pub fn write(&mut self, slot: Slot, bytes: Vec<u8>) {
        self.slots[slot as usize] = Some(bytes);
    }
    pub fn erase(&mut self, slot: Slot) {
        self.slots[slot as usize] = None;
    }
}

impl NvmStorage for MemStorage {
    fn read(&self, slot: Slot) -> Option<Vec<u8>> {
        MemStorage::read(self, slot)
    }
    fn write(&mut self, slot: Slot, bytes: Vec<u8>) {
        MemStorage::write(self, slot, bytes)
    }
    fn erase(&mut self, slot: Slot) {
        MemStorage::erase(self, slot)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotStatus {
    Empty,
    Valid { seq: u32 },
    /// Wrong magic / length / CRC, or the content is not a valid config.
    Corrupt,
}

/// Where the config returned by `load` came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Active,
    Backup,
    Defaults,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub source: Source,
    /// [Active, Backup]
    pub slots: [SlotStatus; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub enum NvmError {
    /// The new config failed validation (every problem is listed). Nothing was stored.
    Invalid(Vec<ConfigError>),
    /// No valid backup to roll back to.
    NoBackup,
}

/// CRC-32 (IEEE 802.3, reflected, as in zlib).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn encode(seq: u32, json: &str) -> Vec<u8> {
    let payload = json.as_bytes();
    let mut v = Vec::with_capacity(HEADER + payload.len() + 4);
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    let crc = crc32(&v[4..]);
    v.extend_from_slice(&crc.to_le_bytes());
    v
}

/// A decoded, valid record.
struct Record {
    seq: u32,
    config: Config,
    raw: Vec<u8>,
}

fn decode(bytes: &[u8]) -> Option<Record> {
    if bytes.len() < HEADER + 4 || &bytes[..4] != MAGIC {
        return None;
    }
    let seq = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let len = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    if bytes.len() != HEADER + len + 4 {
        return None;
    }
    let crc = u32::from_le_bytes(bytes[HEADER + len..].try_into().ok()?);
    if crc != crc32(&bytes[4..HEADER + len]) {
        return None;
    }
    let json = std::str::from_utf8(&bytes[HEADER..HEADER + len]).ok()?;
    let config = config::parse(json).ok()?;
    Some(Record { seq, config, raw: bytes.to_vec() })
}

pub struct Nvm<S: NvmStorage> {
    storage: S,
}

impl<S: NvmStorage> Nvm<S> {
    pub fn new(storage: S) -> Self {
        Self { storage }
    }

    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Direct access to the medium (used to inject corruption in tests and in the GUI).
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }

    fn record(&self, slot: Slot) -> (SlotStatus, Option<Record>) {
        match self.storage.read(slot) {
            None => (SlotStatus::Empty, None),
            Some(bytes) => match decode(&bytes) {
                Some(r) => (SlotStatus::Valid { seq: r.seq }, Some(r)),
                None => (SlotStatus::Corrupt, None),
            },
        }
    }

    /// Boot-time read: Active, else Backup, else defaults.
    pub fn load(&self) -> Loaded {
        let (sa, ra) = self.record(Slot::Active);
        let (sb, rb) = self.record(Slot::Backup);
        let slots = [sa, sb];
        match (ra, rb) {
            (Some(a), _) => Loaded { config: a.config, source: Source::Active, slots },
            (None, Some(b)) => Loaded { config: b.config, source: Source::Backup, slots },
            (None, None) => Loaded { config: Config::default(), source: Source::Defaults, slots },
        }
    }

    /// Validate `json` and, only if it is valid, store it as the new Active config.
    /// Returns the new sequence number.
    pub fn commit(&mut self, json: &str) -> Result<u32, NvmError> {
        let cfg = config::parse(json).map_err(NvmError::Invalid)?;
        let (_, ra) = self.record(Slot::Active);
        let (_, rb) = self.record(Slot::Backup);
        let seq = ra.iter().chain(rb.iter()).map(|r| r.seq).max().unwrap_or(0) + 1;
        if let Some(a) = ra {
            self.storage.write(Slot::Backup, a.raw);
        }
        self.storage.write(Slot::Active, encode(seq, &config::to_json(&cfg)));
        Ok(seq)
    }

    /// `Active <- Backup`; the backup is cleared. Fails if there is no valid backup.
    pub fn rollback(&mut self) -> Result<u32, NvmError> {
        let (_, ra) = self.record(Slot::Active);
        let (_, rb) = self.record(Slot::Backup);
        let b = rb.ok_or(NvmError::NoBackup)?;
        let seq = ra.iter().map(|r| r.seq).chain(std::iter::once(b.seq)).max().unwrap_or(0) + 1;
        self.storage.write(Slot::Active, encode(seq, &config::to_json(&b.config)));
        self.storage.erase(Slot::Backup);
        Ok(seq)
    }
}

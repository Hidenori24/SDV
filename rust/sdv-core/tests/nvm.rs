//! Pseudo NVM (ADR-0003): A/B slots + sequence number + CRC32, fallback and rollback.

use sdv_core::config::Config;
use sdv_core::nvm::{crc32, MemStorage, Nvm, NvmError, Slot, SlotStatus, Source};

fn nvm() -> Nvm<MemStorage> {
    Nvm::new(MemStorage::default())
}

fn accel(c: &Config) -> f32 {
    c.vehicle_spec.max_accel_mps2
}

fn commit(n: &mut Nvm<MemStorage>, accel_mps2: f32) -> u32 {
    n.commit(&format!(
        r#"{{"engine":{{"max_accel_mps2":{accel_mps2}}}}}"#
    ))
    .expect("valid config")
}

fn corrupt(n: &mut Nvm<MemStorage>, slot: Slot) {
    let mut bytes = n.storage().read(slot).expect("slot has data");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    n.storage_mut().write(slot, bytes);
}

#[test]
fn crc32_matches_the_standard_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn empty_nvm_boots_with_defaults() {
    let l = nvm().load();
    assert_eq!(l.config, Config::default());
    assert_eq!(l.source, Source::Defaults);
    assert_eq!(l.slots, [SlotStatus::Empty, SlotStatus::Empty]);
}

#[test]
fn commit_then_load_returns_the_committed_config() {
    let mut n = nvm();
    assert_eq!(commit(&mut n, 4.0), 1);
    let l = n.load();
    assert_eq!(accel(&l.config), 4.0);
    assert_eq!(l.source, Source::Active);
    assert_eq!(l.slots[Slot::Active as usize], SlotStatus::Valid { seq: 1 });
    assert_eq!(l.slots[Slot::Backup as usize], SlotStatus::Empty);
}

#[test]
fn partial_updates_are_stored_as_full_canonical_configs() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    let stored = String::from_utf8_lossy(&n.storage().read(Slot::Active).unwrap()).to_string();
    assert!(
        stored.contains("heartbeat_timeout_ms"),
        "the full config is stored, not just the patch"
    );
}

#[test]
fn second_commit_moves_the_previous_config_to_backup() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    assert_eq!(commit(&mut n, 6.0), 2);
    let l = n.load();
    assert_eq!(accel(&l.config), 6.0);
    assert_eq!(
        l.slots,
        [SlotStatus::Valid { seq: 2 }, SlotStatus::Valid { seq: 1 }]
    );
}

#[test]
fn invalid_commit_is_rejected_and_changes_nothing() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    let before = (
        n.storage().read(Slot::Active),
        n.storage().read(Slot::Backup),
    );
    let err = n
        .commit(r#"{"engine":{"max_accel_mps2":99.0},"safety":{"heartbeat_timeout_ms":1}}"#)
        .unwrap_err();
    match err {
        NvmError::Invalid(e) => assert_eq!(e.len(), 2, "every problem is reported"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        (
            n.storage().read(Slot::Active),
            n.storage().read(Slot::Backup)
        ),
        before
    );
    assert_eq!(accel(&n.load().config), 4.0);
}

#[test]
fn rollback_restores_the_previous_config_and_clears_the_backup() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    commit(&mut n, 6.0);
    n.rollback().unwrap();
    let l = n.load();
    assert_eq!(accel(&l.config), 4.0);
    assert_eq!(l.source, Source::Active);
    assert_eq!(l.slots[Slot::Backup as usize], SlotStatus::Empty);
    assert_eq!(
        n.rollback(),
        Err(NvmError::NoBackup),
        "cannot roll back twice in a row"
    );
}

#[test]
fn sequence_numbers_keep_increasing_across_rollback() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    commit(&mut n, 6.0); // seq 2
    n.rollback().unwrap(); // active = old config, seq 3
    assert_eq!(
        n.load().slots[Slot::Active as usize],
        SlotStatus::Valid { seq: 3 }
    );
    assert_eq!(commit(&mut n, 8.0), 4);
}

#[test]
fn rollback_without_a_backup_fails() {
    let mut n = nvm();
    assert_eq!(n.rollback(), Err(NvmError::NoBackup));
    commit(&mut n, 4.0);
    assert_eq!(n.rollback(), Err(NvmError::NoBackup));
}

#[test]
fn corrupt_active_falls_back_to_the_backup() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    commit(&mut n, 6.0);
    corrupt(&mut n, Slot::Active);
    let l = n.load();
    assert_eq!(accel(&l.config), 4.0);
    assert_eq!(l.source, Source::Backup);
    assert_eq!(l.slots[Slot::Active as usize], SlotStatus::Corrupt);
}

#[test]
fn both_slots_corrupt_boots_with_defaults() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    commit(&mut n, 6.0);
    corrupt(&mut n, Slot::Active);
    corrupt(&mut n, Slot::Backup);
    let l = n.load();
    assert_eq!(l.config, Config::default());
    assert_eq!(l.source, Source::Defaults);
}

#[test]
fn truncated_or_foreign_data_counts_as_corrupt() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    let mut bytes = n.storage().read(Slot::Active).unwrap();
    bytes.truncate(bytes.len() - 3);
    n.storage_mut().write(Slot::Active, bytes);
    assert_eq!(n.load().slots[0], SlotStatus::Corrupt);
    n.storage_mut()
        .write(Slot::Active, b"not an nvm record".to_vec());
    assert_eq!(n.load().slots[0], SlotStatus::Corrupt);
    n.storage_mut().write(Slot::Active, vec![]);
    assert_eq!(n.load().slots[0], SlotStatus::Corrupt);
}

#[test]
fn commit_after_a_corrupt_active_keeps_the_valid_backup() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    commit(&mut n, 6.0);
    corrupt(&mut n, Slot::Active);
    commit(&mut n, 8.0);
    let l = n.load();
    assert_eq!(accel(&l.config), 8.0);
    assert_eq!(l.slots[Slot::Backup as usize], SlotStatus::Valid { seq: 1 });
}

#[test]
fn rollback_with_a_corrupt_backup_fails_and_changes_nothing() {
    let mut n = nvm();
    commit(&mut n, 4.0);
    commit(&mut n, 6.0);
    corrupt(&mut n, Slot::Backup);
    assert_eq!(n.rollback(), Err(NvmError::NoBackup));
    assert_eq!(accel(&n.load().config), 6.0);
}

#[test]
fn a_sim_can_boot_from_the_nvm() {
    use sdv_core::sim::Sim;
    let mut n = nvm();
    commit(&mut n, 4.0);
    let mut sim = Sim::with_config(n.load().config);
    sim.input_mut().throttle = 1.0;
    for _ in 0..50 {
        sim.step_10ms();
    }
    assert!(
        sim.state().v > 1.5,
        "4 m/s^2 engine reaches more than 1.5 m/s in 0.5 s"
    );
}

// ---- abandoning a failed update (trial application) ----

#[test]
fn active_seq_is_the_generation_of_the_running_config() {
    let mut n = nvm();
    assert_eq!(
        n.active_seq(),
        0,
        "nothing stored: the factory defaults, generation 0"
    );
    assert_eq!(commit(&mut n, 3.0), 1);
    assert_eq!(n.active_seq(), 1);
    assert_eq!(commit(&mut n, 4.0), 2);
    assert_eq!(n.active_seq(), 2);
    corrupt(&mut n, Slot::Active);
    assert_eq!(n.active_seq(), 0, "a corrupt Active is not a generation");
}

#[test]
fn abandoning_an_update_goes_back_to_the_previous_config() {
    let mut n = nvm();
    commit(&mut n, 3.0);
    commit(&mut n, 4.0);
    n.abandon_update().expect("there is a previous config");
    let l = n.load();
    assert_eq!(accel(&l.config), 3.0);
    assert_eq!(l.source, Source::Active);
    assert_eq!(
        l.slots[1],
        SlotStatus::Empty,
        "the failed config is gone, not kept as a backup"
    );
}

#[test]
fn abandoning_the_first_update_goes_back_to_the_factory_defaults() {
    let mut n = nvm();
    commit(&mut n, 3.0);
    assert!(
        matches!(n.rollback(), Err(NvmError::NoBackup)),
        "a manual rollback needs a backup..."
    );
    n.abandon_update()
        .expect("...but abandoning a failed first update does not");
    let l = n.load();
    assert_eq!(l.config, Config::default());
    assert_eq!(l.source, Source::Defaults);
    assert_eq!(l.slots, [SlotStatus::Empty, SlotStatus::Empty]);
}

#[test]
fn abandoning_with_nothing_stored_is_a_no_op() {
    let mut n = nvm();
    n.abandon_update()
        .expect("nothing to abandon is not an error");
    assert_eq!(n.load().source, Source::Defaults);
}

#[test]
fn the_next_commit_after_an_abandoned_update_gets_a_fresh_generation() {
    let mut n = nvm();
    commit(&mut n, 3.0);
    commit(&mut n, 4.0);
    n.abandon_update().unwrap();
    let after = n.active_seq();
    assert!(
        after > 2,
        "the rolled-back config is a new record, newer than the failed one (got {after})"
    );
    assert_eq!(commit(&mut n, 5.0), after + 1);
}

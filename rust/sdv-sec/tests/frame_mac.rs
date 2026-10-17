//! Frame authentication (SecOC-style): what the tag covers and what it must reject.
use sdv_sec::*;

const KEY: &[u8] = b"per-ecu demo key (not a secret)";

#[test]
fn tag_is_truncated_hmac_of_the_documented_layout() {
    let payload = [1u8, 2, 3, 4, 5];
    let mut msg = Vec::new();
    msg.extend_from_slice(&0x0123u16.to_be_bytes());
    msg.extend_from_slice(&0xdead_beefu32.to_be_bytes());
    msg.extend_from_slice(&payload);
    let full = hmac_sha256(KEY, &msg);
    assert_eq!(
        frame_mac(KEY, 0x0123, 0xdead_beef, &payload),
        full[..FRAME_MAC_LEN]
    );
}

#[test]
fn verifies_the_genuine_frame_and_rejects_every_kind_of_change() {
    let payload = b"steer=0.25".to_vec();
    let tag = frame_mac(KEY, 0x10, 41, &payload);
    assert_eq!(frame_mac_verify(KEY, 0x10, 41, &payload, &tag), Ok(()));

    assert_eq!(
        frame_mac_verify(KEY, 0x10, 41, b"steer=0.26", &tag),
        Err(MacError::BadTag),
        "payload"
    );
    assert_eq!(
        frame_mac_verify(KEY, 0x11, 41, &payload, &tag),
        Err(MacError::BadTag),
        "frame type"
    );
    assert_eq!(
        frame_mac_verify(KEY, 0x10, 42, &payload, &tag),
        Err(MacError::BadTag),
        "counter"
    );
    assert_eq!(
        frame_mac_verify(b"another key", 0x10, 41, &payload, &tag),
        Err(MacError::BadTag),
        "key"
    );
    for i in 0..FRAME_MAC_LEN {
        let mut bad = tag;
        bad[i] ^= 0x80;
        assert_eq!(
            frame_mac_verify(KEY, 0x10, 41, &payload, &bad),
            Err(MacError::BadTag),
            "tag byte {i}"
        );
    }
}

#[test]
fn empty_payload_is_authenticated_too() {
    let tag = frame_mac(KEY, 1, 1, &[]);
    assert_eq!(frame_mac_verify(KEY, 1, 1, &[], &tag), Ok(()));
    assert_eq!(
        frame_mac_verify(KEY, 1, 2, &[], &tag),
        Err(MacError::BadTag)
    );
}

#[test]
fn freshness_accepts_only_newer_counters() {
    let mut f = Freshness::new(10);
    assert!(!f.accept(10), "replay of the last frame");
    assert!(!f.accept(3), "old frame");
    assert_eq!(f.last(), 10, "a rejected frame must not move the counter");
    assert!(f.accept(11));
    assert!(f.accept(15), "gaps are fine (lost frames)");
    assert!(!f.accept(14));
    assert_eq!(f.last(), 15);
}

#[test]
fn an_attacker_cannot_replay_or_reorder_authentic_frames() {
    // sender
    let mut counter = 0u32;
    let mut send = |payload: &[u8]| {
        counter += 1;
        (
            counter,
            payload.to_vec(),
            frame_mac(KEY, 0x20, counter, payload),
        )
    };
    let (f1, p1, t1) = send(b"commit");
    let (f2, p2, t2) = send(b"abort");

    // receiver: verify the MAC first, then the counter
    let mut rx = Freshness::default();
    let mut receive = |f: u32, p: &[u8], t: &[u8; FRAME_MAC_LEN]| {
        frame_mac_verify(KEY, 0x20, f, p, t).is_ok() && rx.accept(f)
    };
    assert!(
        receive(f2, &p2, &t2),
        "frame 2 arrives first and is genuine"
    );
    assert!(!receive(f1, &p1, &t1), "the older genuine frame is stale");
    assert!(!receive(f2, &p2, &t2), "a replay is stale");
    assert!(
        !receive(f2 + 1, &p2, &t2),
        "a forged counter breaks the tag"
    );
}

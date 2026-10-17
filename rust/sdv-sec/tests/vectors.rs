//! Known-answer tests: NIST (SHA-256), RFC 4231 (HMAC-SHA256), RFC 8032 section 7.1 (Ed25519).
use sdv_sec::*;

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn arr<const N: usize>(s: &str) -> [u8; N] {
    hex(s).try_into().unwrap()
}

#[test]
fn sha256_known_answers() {
    assert_eq!(
        sha256(b"").to_vec(),
        hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );
    assert_eq!(
        sha256(b"abc").to_vec(),
        hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert_eq!(
        sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").to_vec(),
        hex("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1")
    );
}

#[test]
fn hmac_sha256_rfc4231() {
    // test case 1
    assert_eq!(
        hmac_sha256(&[0x0b; 20], b"Hi There").to_vec(),
        hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
    );
    // test case 2
    assert_eq!(
        hmac_sha256(b"Jefe", b"what do ya want for nothing?").to_vec(),
        hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
    );
    // test case 6: key longer than the block size
    assert_eq!(
        hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )
        .to_vec(),
        hex("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")
    );
}

const RFC8032_1: (&str, &str, &str, &str) = (
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
    "",
    "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155 5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
);
const RFC8032_2: (&str, &str, &str, &str) = (
    "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
    "72",
    "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da 085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
);

#[test]
fn ed25519_verify_rfc8032() {
    for (_, pk, msg, sig) in [RFC8032_1, RFC8032_2] {
        assert_eq!(ed25519_verify(&arr(pk), &hex(msg), &arr(sig)), Ok(()));
    }
}

#[test]
fn ed25519_rejects_tampering() {
    let (_, pk, msg, sig) = RFC8032_2;
    let (pk, msg, sig) = (arr::<32>(pk), hex(msg), arr::<64>(sig));
    // another message
    assert_eq!(
        ed25519_verify(&pk, b"\x73", &sig),
        Err(VerifyError::BadSignature)
    );
    // one flipped bit anywhere in the signature
    for i in 0..64 {
        let mut bad = sig;
        bad[i] ^= 0x01;
        assert!(
            ed25519_verify(&pk, &msg, &bad).is_err(),
            "bit flip in byte {i} must be rejected"
        );
    }
    // another key
    let other = arr::<32>(RFC8032_1.1);
    assert_eq!(
        ed25519_verify(&other, &msg, &sig),
        Err(VerifyError::BadSignature)
    );
    // weak / invalid keys: the identity point is a weak key
    let mut identity = [0u8; 32];
    identity[0] = 1;
    assert_eq!(
        ed25519_verify(&identity, &msg, &sig),
        Err(VerifyError::BadKey)
    );
}

#[cfg(feature = "sign")]
#[test]
fn signing_matches_rfc8032_and_round_trips() {
    for (sk, pk, msg, sig) in [RFC8032_1, RFC8032_2] {
        let seed = arr::<32>(sk);
        assert_eq!(ed25519_public_key(&seed).to_vec(), hex(pk));
        assert_eq!(ed25519_sign(&seed, &hex(msg)).to_vec(), hex(sig));
    }
    let seed = [7u8; 32];
    let pk = ed25519_public_key(&seed);
    let msg = b"manifest bytes";
    assert_eq!(ed25519_verify(&pk, msg, &ed25519_sign(&seed, msg)), Ok(()));
}

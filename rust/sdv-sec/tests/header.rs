//! include/sdv_sec.h is hand-written; keep it in step with the exported C ABI.
use std::collections::BTreeSet;

const HEADER: &str = include_str!("../include/sdv_sec.h");
const FFI: &str = include_str!("../src/ffi.rs");

fn header_functions() -> BTreeSet<String> {
    HEADER
        .lines()
        .filter(|l| l.starts_with("int32_t sdv_sec_"))
        .map(|l| l["int32_t ".len()..l.find('(').unwrap()].to_string())
        .collect()
}

fn exported_functions() -> BTreeSet<String> {
    FFI.lines()
        .filter_map(|l| l.strip_prefix("pub unsafe extern \"C\" fn "))
        .map(|l| l[..l.find('(').unwrap()].to_string())
        .collect()
}

#[test]
fn header_and_exports_list_the_same_functions() {
    let (h, e) = (header_functions(), exported_functions());
    assert!(!h.is_empty());
    assert_eq!(h, e, "header (left) and src/ffi.rs exports (right) differ");
}

#[test]
fn every_export_is_unmangled() {
    let n_fns = FFI.matches("pub unsafe extern \"C\" fn ").count();
    assert_eq!(FFI.matches("#[no_mangle]").count(), n_fns);
}

#[test]
fn status_codes_and_lengths_match() {
    for (name, value) in [
        ("SDV_SEC_OK", 0),
        ("SDV_SEC_BAD_KEY", 1),
        ("SDV_SEC_BAD_SIGNATURE", 2),
        ("SDV_SEC_BAD_TAG", 3),
        ("SDV_SEC_STALE", 4),
        ("SDV_SEC_INVALID_ARG", -1),
    ] {
        let rust = format!("pub const {name}: i32 = {value};");
        assert!(FFI.contains(&rust), "ffi.rs should define `{rust}`");
        let c = if value < 0 {
            format!("#define {name} ({value})")
        } else {
            format!("#define {name} {value}")
        };
        assert!(HEADER.contains(&c), "header should define `{c}`");
    }
    for (name, value) in [
        ("SDV_SEC_SHA256_LEN", sdv_sec::SHA256_LEN),
        (
            "SDV_SEC_ED25519_PUBLIC_KEY_LEN",
            sdv_sec::ED25519_PUBLIC_KEY_LEN,
        ),
        (
            "SDV_SEC_ED25519_SIGNATURE_LEN",
            sdv_sec::ED25519_SIGNATURE_LEN,
        ),
        ("SDV_SEC_FRAME_MAC_LEN", sdv_sec::FRAME_MAC_LEN),
    ] {
        assert!(
            HEADER.contains(&format!("#define {name} {value}")),
            "{name} should be {value}"
        );
    }
}

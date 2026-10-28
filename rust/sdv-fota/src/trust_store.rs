//! Root-provisioned Linux trust anchors for the host installer.

use crate::{TrustKey, MAX_KEY_ID_BYTES};
use sdv_sec::ED25519_PUBLIC_KEY_LEN;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

pub const TRUST_STORE_PATH: &str = "/etc/sdv-fota/trusted-keys";
const MAX_TRUST_STORE_BYTES: usize = 8192;
const MAX_TRUST_KEYS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedKey {
    pub key_id: String,
    pub public_key: [u8; ED25519_PUBLIC_KEY_LEN],
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidFile,
    UntrustedPermissions,
    TooLarge,
    InvalidLine(usize),
    DuplicateKeyId,
    TooManyKeys,
    Empty,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl TrustedKey {
    pub fn as_trust_key(&self) -> TrustKey<'_> {
        TrustKey {
            key_id: &self.key_id,
            public_key: &self.public_key,
        }
    }
}

/// Load the fixed trust store. Release builds require a root-owned, non-writable file.
pub fn load(path: impl AsRef<Path>) -> Result<Vec<TrustedKey>, Error> {
    load_inner(path.as_ref(), true)
}

/// Load a trust store while allowing an unprivileged owner, for isolated host tests only.
#[cfg(debug_assertions)]
pub fn load_for_development(path: impl AsRef<Path>) -> Result<Vec<TrustedKey>, Error> {
    load_inner(path.as_ref(), false)
}

fn load_inner(path: &Path, require_root_owner: bool) -> Result<Vec<TrustedKey>, Error> {
    if require_root_owner {
        let parent = path.parent().ok_or(Error::InvalidFile)?;
        let parent_metadata = fs::symlink_metadata(parent)?;
        let parent_mode = parent_metadata.permissions().mode();
        if !parent_metadata.file_type().is_dir()
            || parent_metadata.uid() != 0
            || parent_mode & 0o022 != 0
        {
            return Err(Error::UntrustedPermissions);
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(Error::InvalidFile);
    }
    let mode = metadata.permissions().mode();
    if mode & 0o022 != 0 || (require_root_owner && metadata.uid() != 0) {
        return Err(Error::UntrustedPermissions);
    }
    if metadata.len() > MAX_TRUST_STORE_BYTES as u64 {
        return Err(Error::TooLarge);
    }

    let mut contents = String::new();
    File::open(path)?
        .take((MAX_TRUST_STORE_BYTES + 1) as u64)
        .read_to_string(&mut contents)?;
    if contents.len() > MAX_TRUST_STORE_BYTES {
        return Err(Error::TooLarge);
    }

    let mut keys = Vec::new();
    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_ascii_whitespace();
        let (Some(key_id), Some(public_key_hex), None) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(Error::InvalidLine(index + 1));
        };
        if key_id.is_empty()
            || key_id.len() > MAX_KEY_ID_BYTES
            || !key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            || public_key_hex.len() != ED25519_PUBLIC_KEY_LEN * 2
        {
            return Err(Error::InvalidLine(index + 1));
        }
        if keys.iter().any(|key: &TrustedKey| key.key_id == key_id) {
            return Err(Error::DuplicateKeyId);
        }
        if keys.len() == MAX_TRUST_KEYS {
            return Err(Error::TooManyKeys);
        }
        let mut public_key = [0u8; ED25519_PUBLIC_KEY_LEN];
        for (byte, pair) in public_key
            .iter_mut()
            .zip(public_key_hex.as_bytes().chunks_exact(2))
        {
            *byte = (parse_hex(pair[0]).ok_or(Error::InvalidLine(index + 1))? << 4)
                | parse_hex(pair[1]).ok_or(Error::InvalidLine(index + 1))?;
        }
        keys.push(TrustedKey {
            key_id: key_id.to_owned(),
            public_key,
        });
    }
    if keys.is_empty() {
        return Err(Error::Empty);
    }
    Ok(keys)
}

fn parse_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new(contents: &str, mode: u32) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("sdv-fota-trust-{}-{id}", std::process::id()));
            fs::write(&path, contents).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn parses_multiple_keys_and_ignores_comments() {
        let file = TempFile::new(
            "# release keys\nrelease-1 0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20\nrelease-2 ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\n",
            0o644,
        );
        let keys = load_for_development(&file.0).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].key_id, "release-1");
        assert_eq!(keys[0].public_key[0], 1);
        assert_eq!(keys[0].public_key[31], 32);
        assert_eq!(keys[1].public_key, [0xff; 32]);
    }

    #[test]
    fn rejects_duplicate_ids_malformed_keys_and_unsafe_modes() {
        let duplicate = TempFile::new(
            "release 0000000000000000000000000000000000000000000000000000000000000000\nrelease ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\n",
            0o644,
        );
        assert!(matches!(
            load_for_development(&duplicate.0),
            Err(Error::DuplicateKeyId)
        ));

        let malformed = TempFile::new("release not-a-public-key\n", 0o644);
        assert!(matches!(
            load_for_development(&malformed.0),
            Err(Error::InvalidLine(1))
        ));

        let writable = TempFile::new(
            "release 0000000000000000000000000000000000000000000000000000000000000000\n",
            0o666,
        );
        assert!(matches!(
            load_for_development(&writable.0),
            Err(Error::UntrustedPermissions)
        ));
    }

    #[test]
    fn production_loader_requires_root_owned_store() {
        let file = TempFile::new(
            "release 0000000000000000000000000000000000000000000000000000000000000000\n",
            0o644,
        );
        if fs::metadata(&file.0).unwrap().uid() != 0 {
            assert!(matches!(load(&file.0), Err(Error::UntrustedPermissions)));
        }
    }
}

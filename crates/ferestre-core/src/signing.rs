//! Release signatures.
//!
//! This is the one place release signatures are checked. Every artefact the
//! launcher downloads and then runs -- today the runtime, a 300 MB Proton
//! build, fetched by `ferestre install-runtime`, which the window also runs for
//! its first-run download -- goes through [`crate::http::download_signed`],
//! which verifies against [`RELEASE_PUBLIC_KEY`] before anything is renamed
//! into place.
//!
//! TLS says the bytes came from GitHub; it says nothing about whether they are
//! the bytes the release train built, and a checksum file published beside the
//! archive comes over the same connection from the same place, so it can be
//! swapped along with it. A signature cannot: the secret key lives in the
//! release workflows' secrets (`MINISIGN_SECRET_KEY`, and `MINISIGN_PASSWORD`
//! when the key has one) and nowhere else, and the public half is compiled
//! into the launcher below. `packaging/sign-release.sh` makes the signatures.
//!
//! The format is minisign's, made with `minisign -S -s <key> -m <file> -t
//! <comment>`, which publishes `<file>.minisig` beside the artefact. Only
//! pre-hashed signatures (the default since minisign 0.10) are accepted. The
//! signed "trusted comment" must be `file:<name> version:<version>`: the
//! artefact's published file name and the release version it belongs to (the
//! tag without its `runtime-` or `v` prefix). Both are checked ([`Artefact`]),
//! so a genuine signature for one artefact cannot be replayed for another --
//! an older runtime served under a newer name, or under a newer release's tag.
//! A comment that names a second, different file or version is refused too.

use anyhow::{anyhow, bail, Context, Result};
use minisign_verify::{PublicKey, Signature};
use std::io::Read;
use std::path::Path;

/// The public half of the key the release workflows sign with, in minisign's
/// base64 form (the second line of a `minisign.pub` file, "RW...").
///
/// PLACEHOLDER: replace this value with the real release public key before
/// releasing. While it holds [`RELEASE_PUBLIC_KEY_PLACEHOLDER`] every signed
/// download is refused, which is the safe way for a build without a key to
/// fail, and release CI refuses to publish a launcher that does not carry the
/// key it signs with.
pub const RELEASE_PUBLIC_KEY: &str = "RWQhHyR/MHoTJtPui/l2Xbhp3ETYauN4ErUGb6wYrjpw/rzt+874RvOb";

/// What [`RELEASE_PUBLIC_KEY`] holds until the real key is put in.
pub const RELEASE_PUBLIC_KEY_PLACEHOLDER: &str = "FERESTRE_RELEASE_PUBLIC_KEY_PLACEHOLDER";

/// Whether this build carries a release key at all.
pub fn has_release_key() -> bool {
    RELEASE_PUBLIC_KEY.trim() != RELEASE_PUBLIC_KEY_PLACEHOLDER
}

/// Where a signature is published, relative to its artefact's URL.
pub const SIGNATURE_SUFFIX: &str = ".minisig";

fn public_key(base64: &str) -> Result<PublicKey> {
    if base64.trim() == RELEASE_PUBLIC_KEY_PLACEHOLDER {
        bail!(
            "this build of Ferestre has no release signing key, so it cannot check \
             a download; install a released build or the ferestre-runtime-bin package"
        );
    }
    PublicKey::from_base64(base64.trim())
        .map_err(|e| anyhow!("the built-in release signing key is unusable: {e}"))
}

/// Fail early, before a large download, when no usable key is built in.
pub fn check_public_key(base64: &str) -> Result<()> {
    public_key(base64).map(|_| ())
}

/// What a release signature has to say it was made for: the artefact's
/// published file name and the version of the release it belongs to, as the
/// trusted comment's `file:` and `version:` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Artefact<'a> {
    pub file: &'a str,
    pub version: &'a str,
}

impl<'a> Artefact<'a> {
    pub fn new(file: &'a str, version: &'a str) -> Self {
        Artefact { file, version }
    }

    /// The trusted comment release CI writes for this artefact.
    pub fn trusted_comment(&self) -> String {
        format!("file:{} version:{}", self.file, self.version)
    }
}

fn signature(text: &str, artefact: Artefact) -> Result<Signature> {
    let Artefact { file, version } = artefact;
    if file.is_empty() {
        bail!("no file name to check the release signature against");
    }
    if version.is_empty() {
        bail!("{file}: no release version to check the signature against");
    }
    let signature = Signature::decode(text.trim_start())
        .map_err(|e| anyhow!("{file}: the release signature is not readable: {e}"))?;
    let comment = signature.trusted_comment();
    let fields: Vec<&str> = comment
        .split(['\t', ' '])
        .filter(|field| !field.is_empty())
        .collect();
    // Every field of a kind has to agree: a comment naming two files would
    // otherwise vouch for both.
    let named = |key: &str| -> Vec<&str> {
        fields
            .iter()
            .filter_map(|field| field.strip_prefix(key))
            .collect()
    };
    let files = named("file:");
    if files.is_empty() || files.iter().any(|named| *named != file) {
        bail!("{file}: the release signature was made for a different file");
    }
    let versions = named("version:");
    if versions.is_empty() {
        bail!(
            "{file}: the release signature does not say which release it belongs to, so it \
             cannot vouch for version {version}"
        );
    }
    if let Some(other) = versions.iter().find(|named| **named != version) {
        bail!("{file}: the release signature was made for version {other}, not version {version}");
    }
    Ok(signature)
}

/// Check that a signature reads and was made for `artefact`. Says nothing
/// about whether it is genuine; [`verify_release_file`] does that.
pub fn check_signature(text: &str, artefact: Artefact) -> Result<()> {
    signature(text, artefact).map(|_| ())
}

/// Verify the file at `path`, published as `artefact`, against a minisign
/// signature and the release key built into this launcher.
pub fn verify_release_file(signature_text: &str, artefact: Artefact, path: &Path) -> Result<()> {
    verify_file_with_key(RELEASE_PUBLIC_KEY, signature_text, artefact, path)
}

/// [`verify_release_file`] with an explicit public key.
pub fn verify_file_with_key(
    public_key_base64: &str,
    signature_text: &str,
    artefact: Artefact,
    path: &Path,
) -> Result<()> {
    let name = artefact.file;
    let key = public_key(public_key_base64)?;
    let signature = signature(signature_text, artefact)?;
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|e| anyhow!("{name}: {e}; it was not signed by the Ferestre release key"))?;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if count == 0 {
            break;
        }
        verifier.update(&buffer[..count]);
    }
    verifier.finalize().map_err(|_| {
        anyhow!("{name}: the signature does not match; the download is damaged or not genuine")
    })
}

/// A throwaway minisign key, for tests that need something signed the way a
/// release is. Made from a seed on the spot: never a release key.
#[cfg(any(test, feature = "test-signing"))]
pub mod testing {
    use ed25519_compact::{KeyPair, Seed};
    use std::hash::{BuildHasher, Hasher};

    pub struct TestKey {
        pair: KeyPair,
        key_id: [u8; 8],
    }

    impl Default for TestKey {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestKey {
        /// A new key pair from fresh randomness.
        pub fn new() -> Self {
            let mut seed = [0u8; 32];
            for chunk in seed.chunks_mut(8) {
                let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
                hasher.write_u128(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_nanos()),
                );
                chunk.copy_from_slice(&hasher.finish().to_le_bytes());
            }
            let pair = KeyPair::from_seed(Seed::new(seed));
            let mut key_id = [0u8; 8];
            key_id.copy_from_slice(&pair.pk[..8]);
            TestKey { pair, key_id }
        }

        /// The same key under another key's id: what an impostor claiming the
        /// release key's id looks like.
        pub fn claiming_the_id_of(mut self, other: &TestKey) -> Self {
            self.key_id = other.key_id;
            self
        }

        /// The public key as the second line of a `minisign.pub`.
        pub fn public(&self) -> String {
            let mut bytes = b"Ed".to_vec();
            bytes.extend_from_slice(&self.key_id);
            bytes.extend_from_slice(&self.pair.pk[..]);
            base64(&bytes)
        }

        /// What release CI writes for `artefact`, trusted comment included.
        pub fn sign_as(&self, artefact: super::Artefact, data: &[u8]) -> String {
            self.sign_with_comment(&artefact.trusted_comment(), data)
        }

        /// A pre-hashed signature of `data`, as `minisign -S -t <comment>`
        /// writes it.
        pub fn sign_with_comment(&self, trusted_comment: &str, data: &[u8]) -> String {
            let signature = self.pair.sk.sign(blake2b_512(data), None);
            let mut blob = b"ED".to_vec();
            blob.extend_from_slice(&self.key_id);
            blob.extend_from_slice(&signature[..]);
            let mut global = signature.to_vec();
            global.extend_from_slice(trusted_comment.as_bytes());
            let global = self.pair.sk.sign(global, None);
            format!(
                "untrusted comment: signature from a test key\n{}\ntrusted comment: {trusted_comment}\n{}\n",
                base64(&blob),
                base64(&global[..])
            )
        }
    }

    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// BLAKE2b with a 64-byte digest and no key (RFC 7693), which is what
    /// minisign hashes a file with before signing it.
    pub fn blake2b_512(data: &[u8]) -> [u8; 64] {
        const IV: [u64; 8] = [
            0x6a09e667f3bcc908,
            0xbb67ae8584caa73b,
            0x3c6ef372fe94f82b,
            0xa54ff53a5f1d36f1,
            0x510e527fade682d1,
            0x9b05688c2b3e6c1f,
            0x1f83d9abfb41bd6b,
            0x5be0cd19137e2179,
        ];
        const SIGMA: [[usize; 16]; 10] = [
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
            [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
            [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
            [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
            [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
            [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
            [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
            [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
            [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
        ];
        fn mix(v: &mut [u64; 16], [a, b, c, d]: [usize; 4], x: u64, y: u64) {
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
            v[d] = (v[d] ^ v[a]).rotate_right(32);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(24);
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
            v[d] = (v[d] ^ v[a]).rotate_right(16);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(63);
        }
        fn compress(h: &mut [u64; 8], block: &[u8; 128], counter: u128, last: bool) {
            let mut m = [0u64; 16];
            for (i, word) in m.iter_mut().enumerate() {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&block[i * 8..i * 8 + 8]);
                *word = u64::from_le_bytes(bytes);
            }
            let mut v = [0u64; 16];
            v[..8].copy_from_slice(h);
            v[8..].copy_from_slice(&IV);
            v[12] ^= counter as u64;
            v[13] ^= (counter >> 64) as u64;
            if last {
                v[14] = !v[14];
            }
            for round in 0..12 {
                let s = &SIGMA[round % 10];
                mix(&mut v, [0, 4, 8, 12], m[s[0]], m[s[1]]);
                mix(&mut v, [1, 5, 9, 13], m[s[2]], m[s[3]]);
                mix(&mut v, [2, 6, 10, 14], m[s[4]], m[s[5]]);
                mix(&mut v, [3, 7, 11, 15], m[s[6]], m[s[7]]);
                mix(&mut v, [0, 5, 10, 15], m[s[8]], m[s[9]]);
                mix(&mut v, [1, 6, 11, 12], m[s[10]], m[s[11]]);
                mix(&mut v, [2, 7, 8, 13], m[s[12]], m[s[13]]);
                mix(&mut v, [3, 4, 9, 14], m[s[14]], m[s[15]]);
            }
            for i in 0..8 {
                h[i] ^= v[i] ^ v[i + 8];
            }
        }
        let mut h = IV;
        h[0] ^= 0x0101_0000 ^ 64;
        let mut counter: u128 = 0;
        let mut rest = data;
        while rest.len() > 128 {
            let mut block = [0u8; 128];
            block.copy_from_slice(&rest[..128]);
            counter += 128;
            compress(&mut h, &block, counter, false);
            rest = &rest[128..];
        }
        let mut block = [0u8; 128];
        block[..rest.len()].copy_from_slice(rest);
        counter += rest.len() as u128;
        compress(&mut h, &block, counter, true);
        let mut out = [0u8; 64];
        for (i, word) in h.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&word.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TestKey;
    use super::*;

    const VERSION: &str = "11.0.1";
    const NAME: &str = "ferestre-runtime-11.0.1.tar.xz";
    const RUNTIME: Artefact<'static> = Artefact {
        file: NAME,
        version: VERSION,
    };

    fn written(dir: &Path, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn a_genuine_artefact_verifies() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let data = b"runtime archive bytes".repeat(10_000);
        let path = written(dir.path(), NAME, &data);
        let signature = key.sign_as(RUNTIME, &data);
        assert!(signature
            .contains("trusted comment: file:ferestre-runtime-11.0.1.tar.xz version:11.0.1\n"));
        verify_file_with_key(&key.public(), &signature, RUNTIME, &path).unwrap();
    }

    #[test]
    fn a_changed_byte_is_refused() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let mut data = b"runtime archive bytes".repeat(1_000);
        let signature = key.sign_as(RUNTIME, &data);
        data[500] ^= 1;
        let path = written(dir.path(), NAME, &data);
        let error = verify_file_with_key(&key.public(), &signature, RUNTIME, &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not match"), "{error}");
    }

    #[test]
    fn another_key_is_refused() {
        let ours = TestKey::new();
        let theirs = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), NAME, b"data");
        let signature = theirs.sign_as(RUNTIME, b"data");
        assert!(verify_file_with_key(&ours.public(), &signature, RUNTIME, &path).is_err());
    }

    /// A genuine signature for one release must not vouch for another file
    /// served under this one's name.
    #[test]
    fn a_signature_for_another_file_is_refused() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let old = Artefact::new("ferestre-runtime-10.0.1.tar.xz", "10.0.1");
        let path = written(dir.path(), NAME, b"old runtime");
        let signature = key.sign_as(old, b"old runtime");
        let error = verify_file_with_key(&key.public(), &signature, RUNTIME, &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("different file"), "{error}");
        // A name that merely contains the expected one is not it either.
        let signature =
            key.sign_with_comment(&format!("file:x{NAME} version:{VERSION}"), b"old runtime");
        assert!(verify_file_with_key(&key.public(), &signature, RUNTIME, &path).is_err());
        let signature = key.sign_with_comment("timestamp:1", b"old runtime");
        assert!(verify_file_with_key(&key.public(), &signature, RUNTIME, &path).is_err());
        // Nor does a comment that names this file and another one.
        let signature = key.sign_with_comment(
            &format!("file:{NAME} file:{} version:{VERSION}", old.file),
            b"old runtime",
        );
        let error = verify_file_with_key(&key.public(), &signature, RUNTIME, &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("different file"), "{error}");
    }

    /// The release version is part of what is signed: the right file name
    /// under another release's version, or under none, is refused.
    #[test]
    fn a_signature_for_another_release_version_is_refused() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), NAME, b"data");
        let other = key.sign_as(Artefact::new(NAME, "11.0.0"), b"data");
        let error = verify_file_with_key(&key.public(), &other, RUNTIME, &path)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("version 11.0.0, not version 11.0.1"),
            "{error}"
        );
        // minisign's own default comment names the file and no release.
        let unversioned =
            key.sign_with_comment(&format!("timestamp:1\tfile:{NAME}\thashed"), b"data");
        let error = verify_file_with_key(&key.public(), &unversioned, RUNTIME, &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("which release"), "{error}");
        // The old CI comment carried the tag as `release:`; that is not it.
        let tagged =
            key.sign_with_comment(&format!("file:{NAME} release:runtime-{VERSION}"), b"data");
        assert!(verify_file_with_key(&key.public(), &tagged, RUNTIME, &path).is_err());
        // Two versions, one of them right, is still not one release.
        let both = key.sign_with_comment(
            &format!("file:{NAME} version:{VERSION} version:12.0.0"),
            b"data",
        );
        assert!(verify_file_with_key(&key.public(), &both, RUNTIME, &path).is_err());
        // A version that merely starts with the expected one is not it.
        let longer = key.sign_with_comment(&format!("file:{NAME} version:{VERSION}.1"), b"data");
        assert!(verify_file_with_key(&key.public(), &longer, RUNTIME, &path).is_err());
        // No version to check against is a refusal, not a match.
        let good = key.sign_as(RUNTIME, b"data");
        assert!(
            verify_file_with_key(&key.public(), &good, Artefact::new(NAME, ""), &path).is_err()
        );
    }

    /// The comment is covered by the global signature, so editing it to name
    /// a different file or version breaks the signature rather than
    /// redirecting it.
    #[test]
    fn an_edited_trusted_comment_is_refused() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let old = Artefact::new("ferestre-runtime-10.0.1.tar.xz", "10.0.1");
        let path = written(dir.path(), NAME, b"old runtime");
        let forged = key
            .sign_as(old, b"old runtime")
            .replace(&old.trusted_comment(), &RUNTIME.trusted_comment());
        assert!(forged.contains(NAME));
        assert!(verify_file_with_key(&key.public(), &forged, RUNTIME, &path).is_err());
    }

    #[test]
    fn the_test_hash_is_blake2b() {
        let hex: String = super::testing::blake2b_512(b"abc")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1\
             7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
    }

    /// Somebody else's key that claims the release key's id: what refuses it
    /// has to be the Ed25519 check, not the key id comparison.
    #[test]
    fn another_key_under_the_release_keys_id_is_refused() {
        let ours = TestKey::new();
        let theirs = TestKey::new().claiming_the_id_of(&ours);
        assert_ne!(ours.public(), theirs.public(), "two different keys");
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), NAME, b"data");
        let error = verify_file_with_key(
            &ours.public(),
            &theirs.sign_as(RUNTIME, b"data"),
            RUNTIME,
            &path,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("does not match"), "{error}");
    }

    /// Larger than one read, so the streaming path is what is checked.
    #[test]
    fn a_large_artefact_is_checked_as_it_streams() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let mut data: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        let signature = key.sign_as(RUNTIME, &data);
        let path = written(dir.path(), NAME, &data);
        verify_file_with_key(&key.public(), &signature, RUNTIME, &path).unwrap();
        data[650_000] ^= 1;
        let path = written(dir.path(), NAME, &data);
        assert!(verify_file_with_key(&key.public(), &signature, RUNTIME, &path).is_err());
        // No name to check the comment against is a refusal, not a match.
        assert!(
            verify_file_with_key(&key.public(), &signature, Artefact::new("", VERSION), &path)
                .is_err()
        );
    }

    /// Release CI writes `file:<name> version:<version>`, space-separated;
    /// tabs, as minisign's own default comment has, separate fields too.
    #[test]
    fn the_fields_are_found_however_the_comment_separates_them() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), NAME, b"data");
        for comment in [
            format!("file:{NAME} version:{VERSION}"),
            format!("version:{VERSION}  file:{NAME}"),
            format!("timestamp:1\tfile:{NAME}\tversion:{VERSION}\thashed"),
        ] {
            let signature = key.sign_with_comment(&comment, b"data");
            verify_file_with_key(&key.public(), &signature, RUNTIME, &path).unwrap();
        }
    }

    #[test]
    fn the_placeholder_key_refuses_everything() {
        let error = check_public_key(RELEASE_PUBLIC_KEY);
        if !has_release_key() {
            let error = error.unwrap_err().to_string();
            assert!(error.contains("no release signing key"), "{error}");
        } else {
            error.expect("the built-in release key decodes");
        }
        assert!(check_public_key("not base64 at all !").is_err());
        assert!(check_signature("garbage", Artefact::new("x.tar.xz", "1")).is_err());
    }
}

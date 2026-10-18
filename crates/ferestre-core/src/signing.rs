//! Release signatures.
//!
//! Release CI signs every artefact it publishes with minisign and puts the
//! signature beside it as `<name>.minisig`. The public half of that key is
//! compiled in here, so something the launcher downloads -- a runtime, which it
//! then runs -- is checked against a key that did not arrive over the same
//! connection as the file. A checksum list from the same release would only
//! catch corruption; whoever could replace the file could replace the list.

use anyhow::{anyhow, bail, Context, Result};
use minisign_verify::{PublicKey, Signature};
use std::io::Read;
use std::path::Path;

/// The minisign public key release artefacts are signed with: the second line
/// of the `.pub` file `minisign -G` writes.
///
/// PLACEHOLDER. It is replaced with the real release key before a build that
/// downloads anything is published. While it is still here, nothing downloaded
/// verifies, so nothing downloaded is installed.
pub const RELEASE_PUBLIC_KEY: &str = PLACEHOLDER_PUBLIC_KEY;

const PLACEHOLDER_PUBLIC_KEY: &str = "REPLACE-WITH-THE-FERESTRE-RELEASE-MINISIGN-PUBLIC-KEY";

/// Check `file` against a release signature made for an artefact called
/// `name`, with [`RELEASE_PUBLIC_KEY`].
pub fn verify_release_file(file: &Path, signature: &str, name: &str) -> Result<()> {
    verify_file(RELEASE_PUBLIC_KEY, file, signature, name)
}

/// Check `file` against a minisign `signature` by `public_key`.
///
/// The signature's trusted comment -- the part the signature also covers --
/// has to name the file (`file:<name>`), so a validly signed artefact cannot be
/// passed off under another one's name. Only pre-hashed signatures are
/// accepted, which is what minisign has made by default since 0.8.
pub fn verify_file(public_key: &str, file: &Path, signature: &str, name: &str) -> Result<()> {
    if public_key.trim() == PLACEHOLDER_PUBLIC_KEY {
        bail!(
            "{name} cannot be verified: this build of Ferestre carries no release signing key; \
             use a released build"
        );
    }
    let key = PublicKey::from_base64(public_key.trim())
        .map_err(|e| anyhow!("the release signing key is malformed: {e}"))?;
    let signature =
        Signature::decode(signature).map_err(|e| anyhow!("{name}: unreadable signature: {e}"))?;
    if !names_file(signature.trusted_comment(), name) {
        bail!(
            "{name}: the signature is for something else (\"{}\")",
            signature.trusted_comment()
        );
    }
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|e| anyhow!("{name}: {e}"))?;
    let mut reader =
        std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?;
    let mut buffer = vec![0; 1 << 16];
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
        };
        verifier.update(&buffer[..count]);
    }
    verifier
        .finalize()
        .map_err(|e| anyhow!("{name} does not match its signature: {e}"))
}

fn names_file(trusted_comment: &str, name: &str) -> bool {
    trusted_comment
        .split_whitespace()
        .any(|token| token.strip_prefix("file:") == Some(name))
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

    impl TestKey {
        /// A new key pair from fresh randomness.
        pub fn generate() -> Self {
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

        /// The public key as the second line of a `minisign.pub`.
        pub fn public_key(&self) -> String {
            let mut bytes = b"Ed".to_vec();
            bytes.extend_from_slice(&self.key_id);
            bytes.extend_from_slice(&self.pair.pk[..]);
            base64(&bytes)
        }

        /// A pre-hashed signature of `data`, as `minisign -S -t <comment>`
        /// writes it.
        pub fn sign(&self, data: &[u8], trusted_comment: &str) -> String {
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
    use super::testing::{blake2b_512, TestKey};
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferestre-signing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_test_hash_is_blake2b() {
        let hex: String = blake2b_512(b"abc")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1\
             7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
    }

    #[test]
    fn a_signed_file_verifies_and_anything_else_does_not() {
        let dir = scratch("verify");
        let file = dir.join("ferestre-runtime-1.tar.xz");
        // More than one hash block and more than one read.
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7) as u8).collect();
        std::fs::write(&file, &data).unwrap();
        let key = TestKey::generate();
        let name = "ferestre-runtime-1.tar.xz";
        let signature = key.sign(&data, &format!("timestamp:1 file:{name} hashed"));
        verify_file(&key.public_key(), &file, &signature, name).unwrap();

        // One byte changed.
        let mut tampered = data.clone();
        tampered[100_000] ^= 1;
        std::fs::write(&file, &tampered).unwrap();
        let error = verify_file(&key.public_key(), &file, &signature, name).unwrap_err();
        assert!(error.to_string().contains("does not match"), "{error}");
        std::fs::write(&file, &data).unwrap();

        // Another key.
        let other = TestKey::generate();
        assert!(verify_file(&other.public_key(), &file, &signature, name).is_err());
        // Signed, but for another artefact.
        let elsewhere = key.sign(&data, "file:ferestre-runtime-0.tar.xz");
        let error = verify_file(&key.public_key(), &file, &elsewhere, name).unwrap_err();
        assert!(error.to_string().contains("something else"), "{error}");
        // Not a signature at all.
        assert!(verify_file(&key.public_key(), &file, "not a signature", name).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_placeholder_key_verifies_nothing() {
        let dir = scratch("placeholder");
        let file = dir.join("artefact");
        std::fs::write(&file, b"data").unwrap();
        let key = TestKey::generate();
        let signature = key.sign(b"data", "file:artefact");
        let error = verify_file(PLACEHOLDER_PUBLIC_KEY, &file, &signature, "artefact").unwrap_err();
        assert!(
            error.to_string().contains("no release signing key"),
            "{error}"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}

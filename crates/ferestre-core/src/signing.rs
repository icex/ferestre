//! Release signatures.
//!
//! This is the one place release signatures are checked. Every artefact the
//! launcher downloads and then runs -- today the runtime, a 300 MB Proton
//! build, fetched by `ferestre install-runtime`, which the window also runs for
//! its first-run download -- goes through [`crate::http::download_signed`] or
//! [`crate::http::download_large_cancellable`], which verify against
//! [`RELEASE_PUBLIC_KEY`] before anything is renamed into place.
//!
//! TLS says the bytes came from GitHub; it says nothing about whether they are
//! the bytes the release train built, and a checksum file published beside the
//! archive comes over the same connection from the same place, so it can be
//! swapped along with it. A signature cannot: the secret key lives in the
//! release workflows' secrets (`MINISIGN_SECRET_KEY`, and `MINISIGN_PASSWORD`
//! when the key has one) and nowhere else, and the public half is compiled
//! into the launcher below. `packaging/sign-release.sh` makes the signatures.
//!
//! The format is minisign's, made with `minisign -S -s <key> -m <file>`, which
//! publishes `<file>.minisig` beside the artefact. Only pre-hashed signatures
//! (the default since minisign 0.10) are accepted. The signed "trusted comment"
//! must name the artefact (`file:<name>`, as a tab- or space-separated field;
//! minisign writes it by default), so a genuine signature for one artefact
//! cannot be replayed for another -- an older runtime served under a newer
//! name, say.

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
pub const RELEASE_PUBLIC_KEY: &str = "FERESTRE_RELEASE_PUBLIC_KEY_PLACEHOLDER";

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

fn signature(text: &str, name: &str) -> Result<Signature> {
    if name.is_empty() {
        bail!("no file name to check the release signature against");
    }
    let signature = Signature::decode(text.trim_start())
        .map_err(|e| anyhow!("{name}: the release signature is not readable: {e}"))?;
    let names_it = signature
        .trusted_comment()
        .split(['\t', ' '])
        .any(|field| field.strip_prefix("file:") == Some(name));
    if !names_it {
        bail!("{name}: the release signature was made for a different file");
    }
    Ok(signature)
}

/// Check that a signature reads and was made for the artefact named `name`.
/// Says nothing about whether it is genuine; [`verify_release_file`] does that.
pub fn check_signature(text: &str, name: &str) -> Result<()> {
    signature(text, name).map(|_| ())
}

/// Verify the file at `path`, published as `name`, against a minisign
/// signature and the release key built into this launcher.
pub fn verify_release_file(signature_text: &str, name: &str, path: &Path) -> Result<()> {
    verify_file_with_key(RELEASE_PUBLIC_KEY, signature_text, name, path)
}

/// [`verify_release_file`] with an explicit public key.
pub fn verify_file_with_key(
    public_key_base64: &str,
    signature_text: &str,
    name: &str,
    path: &Path,
) -> Result<()> {
    let key = public_key(public_key_base64)?;
    let signature = signature(signature_text, name)?;
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
/// release is. Generated on the spot with the real minisign implementation:
/// never a release key.
#[cfg(any(test, feature = "test-signing"))]
pub mod testing {
    use minisign::KeyPair;

    /// A key pair made for the test and thrown away with it.
    pub struct TestKey {
        pair: KeyPair,
    }

    impl Default for TestKey {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestKey {
        pub fn new() -> Self {
            TestKey {
                pair: KeyPair::generate_unencrypted_keypair().expect("a test key"),
            }
        }

        /// The public key as the second line of a `minisign.pub`.
        pub fn public(&self) -> String {
            self.pair.pk.to_base64()
        }

        /// What `minisign -S -m <name>` writes, trusted comment included.
        pub fn sign_as(&self, name: &str, data: &[u8]) -> String {
            self.sign_with_comment(&format!("timestamp:1700000000\tfile:{name}\thashed"), data)
        }

        /// A pre-hashed signature with this trusted comment, as
        /// `minisign -S -t <comment>` writes it.
        pub fn sign_with_comment(&self, trusted: &str, data: &[u8]) -> String {
            minisign::sign(
                Some(&self.pair.pk),
                &self.pair.sk,
                std::io::Cursor::new(data),
                Some(trusted),
                Some("signature from a throwaway test key"),
            )
            .expect("signs")
            .into_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TestKey;
    use super::*;

    fn written(dir: &Path, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn a_genuine_artefact_verifies() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let name = "ferestre-runtime-11.0.1.tar.xz";
        let data = b"runtime archive bytes".repeat(10_000);
        let path = written(dir.path(), name, &data);
        let signature = key.sign_as(name, &data);
        verify_file_with_key(&key.public(), &signature, name, &path).unwrap();
    }

    #[test]
    fn a_changed_byte_is_refused() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let name = "ferestre-runtime-11.0.1.tar.xz";
        let mut data = b"runtime archive bytes".repeat(1_000);
        let signature = key.sign_as(name, &data);
        data[500] ^= 1;
        let path = written(dir.path(), name, &data);
        let error = verify_file_with_key(&key.public(), &signature, name, &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not match"), "{error}");
    }

    #[test]
    fn another_key_is_refused() {
        let ours = TestKey::new();
        let theirs = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let name = "ferestre-runtime-11.0.1.tar.xz";
        let path = written(dir.path(), name, b"data");
        let signature = theirs.sign_as(name, b"data");
        assert!(verify_file_with_key(&ours.public(), &signature, name, &path).is_err());
    }

    /// A genuine signature for one release must not vouch for another file
    /// served under this one's name.
    #[test]
    fn a_signature_for_another_file_is_refused() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let old = "ferestre-runtime-10.0.1.tar.xz";
        let new = "ferestre-runtime-11.0.1.tar.xz";
        let path = written(dir.path(), new, b"old runtime");
        let signature = key.sign_as(old, b"old runtime");
        let error = verify_file_with_key(&key.public(), &signature, new, &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("different file"), "{error}");
        // A name that merely contains the expected one is not it either.
        let signature = key.sign_with_comment(&format!("file:x{new}"), b"old runtime");
        assert!(verify_file_with_key(&key.public(), &signature, new, &path).is_err());
        let signature = key.sign_with_comment("timestamp:1", b"old runtime");
        assert!(verify_file_with_key(&key.public(), &signature, new, &path).is_err());
    }

    /// The comment is covered by the global signature, so editing it to name
    /// a different file breaks the signature rather than redirecting it.
    #[test]
    fn an_edited_trusted_comment_is_refused() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let old = "ferestre-runtime-10.0.1.tar.xz";
        let new = "ferestre-runtime-11.0.1.tar.xz";
        let path = written(dir.path(), new, b"old runtime");
        let forged = key
            .sign_as(old, b"old runtime")
            .replace(&format!("file:{old}"), &format!("file:{new}"));
        assert!(forged.contains(new));
        assert!(verify_file_with_key(&key.public(), &forged, new, &path).is_err());
    }

    const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn from_base64(text: &str) -> Vec<u8> {
        let mut bits = 0u32;
        let mut count = 0;
        let mut out = Vec::new();
        for c in text.trim().bytes().take_while(|&c| c != b'=') {
            let value = BASE64.iter().position(|&b| b == c).expect("base64") as u32;
            bits = (bits << 6) | value;
            count += 6;
            if count >= 8 {
                count -= 8;
                out.push((bits >> count) as u8);
            }
        }
        out
    }

    fn to_base64(bytes: &[u8]) -> String {
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
                    out.push(BASE64[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// Somebody else's key that claims the release key's id: what refuses it
    /// has to be the Ed25519 check, not the key id comparison.
    #[test]
    fn another_key_under_the_release_keys_id_is_refused() {
        let ours = TestKey::new();
        let theirs = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let name = "ferestre-runtime-11.0.1.tar.xz";
        let path = written(dir.path(), name, b"data");
        // "Ed", the 8-byte key id, then the 32-byte Ed25519 key.
        let our_key = from_base64(&ours.public());
        let mut their_key = from_base64(&theirs.public());
        assert_eq!(our_key.len(), 42);
        assert_ne!(our_key[10..], their_key[10..], "two different keys");
        their_key[2..10].copy_from_slice(&our_key[2..10]);
        let impostor = to_base64(&their_key);
        assert_eq!(
            to_base64(&our_key),
            ours.public(),
            "the test codec round-trips"
        );
        let error = verify_file_with_key(&impostor, &ours.sign_as(name, b"data"), name, &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not match"), "{error}");
    }

    /// Larger than one read, so the streaming path is what is checked.
    #[test]
    fn a_large_artefact_is_checked_as_it_streams() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let name = "ferestre-runtime-11.0.1.tar.xz";
        let mut data: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        let signature = key.sign_as(name, &data);
        let path = written(dir.path(), name, &data);
        verify_file_with_key(&key.public(), &signature, name, &path).unwrap();
        data[650_000] ^= 1;
        let path = written(dir.path(), name, &data);
        assert!(verify_file_with_key(&key.public(), &signature, name, &path).is_err());
        // No name to check the comment against is a refusal, not a match.
        assert!(verify_file_with_key(&key.public(), &signature, "", &path).is_err());
    }

    /// Release CI writes `file:<name> release:<tag>`; minisign's default is
    /// tab-separated. Both name the file.
    #[test]
    fn the_file_field_is_found_however_the_comment_separates_it() {
        let key = TestKey::new();
        let dir = tempfile::tempdir().unwrap();
        let name = "ferestre-runtime-11.0.1.tar.xz";
        let path = written(dir.path(), name, b"data");
        for comment in [
            format!("file:{name} release:runtime-11.0.1"),
            format!("timestamp:1\tfile:{name}\thashed"),
        ] {
            let signature = key.sign_with_comment(&comment, b"data");
            verify_file_with_key(&key.public(), &signature, name, &path).unwrap();
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
        assert!(check_signature("garbage", "x.tar.xz").is_err());
    }
}

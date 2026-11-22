//! The model cache: the NVIDIA model files, extracted once from the person's
//! own `nvngx_dlssnr.dll` with the package's `model-tools/extract_model.sh`
//! and kept in `<root>/models`. The DLL itself is not kept.
//!
//! Before an install the cached models are linked into the package's
//! `dlssnr-amd/`, where upstream's `install.sh` looks for them, so `--dll` is
//! never passed to it.

use super::dll::Accepted;
use super::{DllVersion, Expect, ModelFile};
use anyhow::{bail, Context, Result};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the models are kept.
pub fn dir(root: &Path) -> PathBuf {
    root.join("models")
}

fn all(expect: &Expect) -> [&ModelFile; 2] {
    [&expect.aardwold, &expect.small]
}

/// The models a DLL of `version` carries.
fn carried<'a>(version: &DllVersion, expect: &'a Expect) -> Vec<&'a ModelFile> {
    let mut models = vec![&expect.aardwold];
    if version.small_model {
        models.push(&expect.small);
    }
    models
}

/// The cached models that have their expected size. Cheap; [`verify`] also
/// checks their SHA-256.
pub fn cached(root: &Path, expect: &Expect) -> Vec<ModelFile> {
    let dir = dir(root);
    all(expect)
        .into_iter()
        .filter(|m| {
            std::fs::metadata(dir.join(&m.file))
                .is_ok_and(|meta| meta.is_file() && meta.len() == m.size)
        })
        .cloned()
        .collect()
}

/// The cached models whose SHA-256 is the expected one. A cached file that
/// does not match is removed, so the next extraction replaces it.
pub fn verify(root: &Path, expect: &Expect) -> Result<Vec<ModelFile>> {
    let dir = dir(root);
    let mut good = Vec::new();
    for model in cached(root, expect) {
        let path = dir.join(&model.file);
        let sha256 = crate::updater::sha256_file(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if sha256 == model.sha256 {
            good.push(model);
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("cannot remove damaged {}", path.display()))?;
        }
    }
    Ok(good)
}

/// The last lines of a command's output, for an error.
fn tail(output: &std::process::Output) -> String {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(10)..].join("\n")
}

/// Extract the models `dll` carries into the cache, unless they are already
/// there. Each must match its expected SHA-256; otherwise nothing is kept.
pub fn extract(
    package_dir: &Path,
    dll: &Accepted,
    root: &Path,
    expect: &Expect,
) -> Result<Vec<ModelFile>> {
    let wanted = carried(&dll.version, expect);
    let have = verify(root, expect)?;
    if wanted.iter().all(|w| have.contains(w)) {
        return Ok(have);
    }
    let models = dir(root);
    std::fs::create_dir_all(&models).with_context(|| format!("{}", models.display()))?;
    crate::cache::sweep_stale_temporaries(&models, "extract");
    let staging = crate::cache::temporary_beside(&models, "extract");
    std::fs::create_dir_all(&staging).with_context(|| format!("{}", staging.display()))?;
    let result = (|| -> Result<Vec<ModelFile>> {
        let script = package_dir.join("model-tools").join("extract_model.sh");
        let output = Command::new("bash")
            .arg(&script)
            .arg(&dll.path)
            .arg(&staging)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("cannot run {}", script.display()))?;
        if !output.status.success() {
            bail!(
                "extracting the model from {} failed ({}):\n{}",
                dll.found_at.display(),
                output.status,
                tail(&output)
            );
        }
        for model in &wanted {
            let path = staging.join(&model.file);
            let sha256 = crate::updater::sha256_file(&path).with_context(|| {
                format!(
                    "{} was not extracted from {}",
                    model.file,
                    dll.found_at.display()
                )
            })?;
            if sha256 != model.sha256 {
                bail!(
                    "{} extracted from {} has SHA-256 {sha256}, not {}",
                    model.file,
                    dll.found_at.display(),
                    model.sha256
                );
            }
        }
        for model in &wanted {
            std::fs::rename(staging.join(&model.file), models.join(&model.file))
                .with_context(|| format!("cannot keep {}", model.file))?;
        }
        crate::cache::sync_directory(&models);
        verify(root, expect)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Put every cached model into `<package_dir>/dlssnr-amd/`: a hard link when
/// both are on one filesystem, else a copy. A different file already there is
/// replaced.
pub fn stage(package_dir: &Path, root: &Path, expect: &Expect) -> Result<()> {
    let models = dir(root);
    let dest_dir = package_dir.join("dlssnr-amd");
    std::fs::create_dir_all(&dest_dir).with_context(|| format!("{}", dest_dir.display()))?;
    for model in cached(root, expect) {
        let src = models.join(&model.file);
        let dest = dest_dir.join(&model.file);
        let source = std::fs::metadata(&src).with_context(|| format!("{}", src.display()))?;
        if std::fs::metadata(&dest)
            .is_ok_and(|d| d.dev() == source.dev() && d.ino() == source.ino())
        {
            continue;
        }
        let temporary = crate::cache::temporary_beside(&dest, "stage");
        let result = (|| -> Result<()> {
            if std::fs::hard_link(&src, &temporary).is_err() {
                std::fs::copy(&src, &temporary)
                    .with_context(|| format!("cannot copy {}", src.display()))?;
            }
            std::fs::rename(&temporary, &dest)
                .with_context(|| format!("cannot write {}", dest.display()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neural::dll::tests::{expect_with, FAKE_NEW, FAKE_OLD};
    use crate::neural::dll::{accept, Accepted};

    const SCRIPT: &str = "echo run >> \"$(dirname \"$0\")/runs\"\n\
                          printf aardwold > \"$2/CC_SILVER_AARDWOLD.bin\"\n\
                          printf small > \"$2/CC_SMALL_FP8.bin\"\n";

    fn package(dir: &Path) -> PathBuf {
        let package = dir.join("package");
        std::fs::create_dir_all(package.join("model-tools")).unwrap();
        std::fs::write(package.join("model-tools/extract_model.sh"), SCRIPT).unwrap();
        package
    }

    fn runs(package: &Path) -> usize {
        std::fs::read_to_string(package.join("model-tools/runs"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }

    fn dll(dir: &Path, bytes: &[u8], expect: &Expect) -> Accepted {
        let path = dir.join("nvngx_dlssnr.dll");
        std::fs::write(&path, bytes).unwrap();
        accept(&path, &dir.join("work"), expect).unwrap()
    }

    #[test]
    fn models_are_extracted_once_and_verified() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dlssnr");
        let package = package(tmp.path());
        let expect = expect_with(b"aardwold", b"small");
        let dll = dll(tmp.path(), FAKE_NEW, &expect);

        assert!(cached(&root, &expect).is_empty());
        let models = extract(&package, &dll, &root, &expect).unwrap();
        assert_eq!(models, vec![expect.aardwold.clone(), expect.small.clone()]);
        assert_eq!(runs(&package), 1);
        assert_eq!(
            std::fs::read(dir(&root).join("CC_SILVER_AARDWOLD.bin")).unwrap(),
            b"aardwold"
        );
        extract(&package, &dll, &root, &expect).unwrap();
        assert_eq!(runs(&package), 1, "extracted once");

        // A cached file damaged with its size kept is found by verify, and
        // the next extraction replaces it.
        std::fs::write(dir(&root).join("CC_SMALL_FP8.bin"), b"SMALL").unwrap();
        assert_eq!(cached(&root, &expect).len(), 2);
        assert_eq!(
            verify(&root, &expect).unwrap(),
            vec![expect.aardwold.clone()]
        );
        extract(&package, &dll, &root, &expect).unwrap();
        assert_eq!(runs(&package), 2);
        assert_eq!(verify(&root, &expect).unwrap().len(), 2);
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("models")]);
    }

    #[test]
    fn an_older_dll_needs_only_the_aardwold_model() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dlssnr");
        let package = package(tmp.path());
        let expect = expect_with(b"aardwold", b"small");
        std::fs::create_dir_all(dir(&root)).unwrap();
        std::fs::write(dir(&root).join("CC_SILVER_AARDWOLD.bin"), b"aardwold").unwrap();
        let dll = dll(tmp.path(), FAKE_OLD, &expect);
        extract(&package, &dll, &root, &expect).unwrap();
        assert_eq!(runs(&package), 0);
    }

    #[test]
    fn a_wrong_extracted_hash_keeps_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dlssnr");
        let package = package(tmp.path());
        // The script writes "small", which is not this model.
        let expect = expect_with(b"aardwold", b"other small");
        let dll = dll(tmp.path(), FAKE_NEW, &expect);
        let err = extract(&package, &dll, &root, &expect)
            .unwrap_err()
            .to_string();
        assert!(err.contains("CC_SMALL_FP8.bin"), "{err}");
        assert!(cached(&root, &expect).is_empty());
        let names: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("models")]);
        assert_eq!(std::fs::read_dir(dir(&root)).unwrap().count(), 0);

        // A failing script is an error with its output.
        std::fs::write(
            package.join("model-tools/extract_model.sh"),
            "echo cannot read the dll >&2\nexit 3\n",
        )
        .unwrap();
        let err = extract(&package, &dll, &root, &expect)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot read the dll"), "{err}");
    }

    #[test]
    fn staging_links_the_cached_models_into_the_package() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dlssnr");
        let package = package(tmp.path());
        let expect = expect_with(b"aardwold", b"small");
        let dll = dll(tmp.path(), FAKE_NEW, &expect);
        extract(&package, &dll, &root, &expect).unwrap();
        // A stale model from an earlier package build is replaced.
        std::fs::create_dir_all(package.join("dlssnr-amd")).unwrap();
        std::fs::write(package.join("dlssnr-amd/CC_SMALL_FP8.bin"), b"old").unwrap();

        stage(&package, &root, &expect).unwrap();
        stage(&package, &root, &expect).unwrap();
        for model in [&expect.aardwold, &expect.small] {
            let cached = std::fs::metadata(dir(&root).join(&model.file)).unwrap();
            let staged = std::fs::metadata(package.join("dlssnr-amd").join(&model.file)).unwrap();
            assert_eq!(
                (cached.dev(), cached.ino()),
                (staged.dev(), staged.ino()),
                "{}",
                model.file
            );
        }
        assert_eq!(
            std::fs::read_dir(package.join("dlssnr-amd"))
                .unwrap()
                .count(),
            2
        );
    }
}

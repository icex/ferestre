use super::{cancel, install_runtime_release, Cli, Paths, Result};
use anyhow::{anyhow, bail, Context};
use ferestre_core::{signing, updater};
use serde::Serialize;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

fn status(paths: &Paths, cancelled: impl Fn() -> bool) -> Result<updater::Status> {
    let packaged = std::env::var("FERESTRE_PACKAGE_VERSION").ok();
    let current = ferestre_core::package_version(packaged.as_deref());
    let image = updater::current_image();
    updater::check_cancellable(paths, current, image.as_deref(), cancelled)
}

pub fn check(cli: &Cli) -> Result<ExitCode> {
    let status = status(&Paths::from_env()?, || false)?;
    if cli.json {
        super::print_json(&serde_json::to_value(&status)?);
    } else {
        println!("{}", status.title());
        if status.launcher.is_some() && status.appimage.is_none() {
            eprintln!(
                "{}",
                status
                    .launcher_unavailable
                    .as_deref()
                    .unwrap_or("This installation cannot be replaced automatically")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Serialize)]
struct Outcome {
    schema: u32,
    updated: Vec<String>,
    appimage: Option<updater::AppliedAppImage>,
}

pub fn apply(
    cli: &Cli,
    minimum_launcher: Option<&str>,
    minimum_runtime: Option<&str>,
    expected_image: Option<&str>,
) -> Result<ExitCode> {
    let cancellation = cancel::Cancellation::from_gui();
    let paths = Paths::from_env()?;
    if let Some(expected) = expected_image {
        let image = updater::current_image().ok_or_else(|| {
            anyhow!("the original AppImage is unavailable; restart Ferestre before updating")
        })?;
        anyhow::ensure!(
            updater::image_fingerprint(&image)? == expected,
            "the AppImage was replaced since this window opened; restart Ferestre before updating"
        );
    }
    eprintln!("-- checking signed Ferestre releases");
    let status = status(&paths, || cancellation.requested())?;
    let packaged = std::env::var("FERESTRE_PACKAGE_VERSION").ok();
    let current = ferestre_core::package_version(packaged.as_deref());
    if let Some(minimum) = minimum_launcher {
        let version = status
            .launcher
            .as_ref()
            .map_or(current, |r| r.version.as_str());
        anyhow::ensure!(updater::at_least(version, minimum, updater::Component::Launcher)?,
            "the release feed is older than the application update offered by this window; nothing was installed");
    }
    if let Some(minimum) = minimum_runtime {
        let installed = updater::current_runtime_version(&paths);
        let version = status
            .runtime
            .as_ref()
            .map(|r| r.version.as_str())
            .or(installed.as_deref())
            .ok_or_else(|| {
                anyhow!("the runtime update offered by this window is no longer available")
            })?;
        anyhow::ensure!(updater::at_least(version, minimum, updater::Component::Runtime)?,
            "the release feed is older than the runtime update offered by this window; nothing was installed");
    }
    cancellation.check()?;
    if status.available() && !status.actionable() {
        bail!(
            "{}",
            status
                .launcher_unavailable
                .as_deref()
                .unwrap_or("This installation cannot be replaced automatically")
        );
    }
    // Prepare and probe the application before changing either component.
    // Keep its lock and old inode while the runtime is installed beside old runtimes.
    let prepared = match (&status.launcher, &status.appimage) {
        (Some(release), Some(target)) => {
            eprintln!("-- downloading and verifying Ferestre {}", release.version);
            Some(updater::PreparedAppImage::prepare(
                target,
                release,
                &paths.cache_dir().join("application-updates"),
                || cancellation.requested(),
                |image, version| probe(image, version, &cancellation),
            )?)
        }
        _ => None,
    };
    let mut updated = Vec::new();
    if let Some(release) = &status.runtime {
        eprintln!("-- installing runtime {}", release.version);
        let code = install_runtime_release(
            false,
            &paths,
            release.runtime_release(),
            signing::RELEASE_PUBLIC_KEY,
            &cancellation,
            false,
            Some(&release.sha256),
        )?;
        anyhow::ensure!(
            code == ExitCode::SUCCESS,
            "runtime update did not finish; the previous AppImage is still installed"
        );
        updated.push(format!("Runtime {}", release.version));
    }
    cancellation.check()?;
    let appimage = prepared
        .map(|image| {
            image.apply_with_receipt(&paths.state_dir().join("last-application-update.json"))
        })
        .transpose()?;
    if let Some(release) = &status.launcher {
        if appimage.is_some() {
            updated.push(format!("Ferestre {}", release.version));
        } else if let Some(reason) = &status.launcher_unavailable {
            eprintln!("-- {reason}");
        }
    }
    let result = Outcome {
        schema: 1,
        updated,
        appimage,
    };
    if cli.json {
        super::print_json(&serde_json::to_value(&result)?);
    } else {
        for name in &result.updated {
            println!("Updated {name}");
        }
        if let Some(applied) = &result.appimage {
            println!(
                "Restart Ferestre to use the update. Previous copy: {}",
                applied.backup.display()
            );
        } else if result.updated.is_empty() {
            println!("Ferestre is up to date");
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn rollback(cli: &Cli) -> Result<ExitCode> {
    let paths = Paths::from_env()?;
    let receipt = paths.state_dir().join("last-application-update.json");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&receipt)
        .context("no application update receipt is available")?;
    let meta = file.metadata()?;
    // SAFETY: geteuid has no preconditions and does not alter process state.
    anyhow::ensure!(
        meta.is_file() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
        "unsafe application update receipt"
    );
    let mut text = String::new();
    file.take(64 * 1024 + 1).read_to_string(&mut text)?;
    anyhow::ensure!(
        text.len() <= 64 * 1024,
        "invalid application update receipt"
    );
    let applied: updater::AppliedAppImage = serde_json::from_str(&text)?;
    updater::rollback(&applied)?;
    std::fs::remove_file(receipt)?;
    if cli.json {
        super::print_json(&serde_json::json!({"schema":1,"restored":applied.target}));
    } else {
        println!("Restored the previous Ferestre. Restart the application to use it.");
    }
    Ok(ExitCode::SUCCESS)
}

fn probe(image: &Path, version: &str, cancellation: &cancel::Cancellation) -> Result<()> {
    // A private output file avoids a descendant keeping a pipe open forever.
    // It is created exclusively and removed only after that creation succeeds.
    let output_path = image.with_extension("probe-output");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&output_path)?;
    let result = (|| -> Result<()> {
        let mut command = updater::image_command(image);
        command
            .args(["--appimage-extract-and-run", "version"])
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = ProbeChild(
            command
                .spawn()
                .context("the new AppImage could not be started")?,
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.0.try_wait()? {
                break status;
            }
            if cancellation.requested() || Instant::now() >= deadline {
                // SAFETY: this process group was created for this probe only.
                unsafe {
                    libc::kill(-(child.0.id() as i32), libc::SIGKILL);
                }
                let _ = child.0.wait();
                bail!("the new AppImage did not answer its startup check; the previous version was kept");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        anyhow::ensure!(
            status.success(),
            "the new AppImage failed its startup check ({status}); the previous version was kept"
        );
        let mut text = String::new();
        std::fs::File::open(&output_path)?
            .take(4097)
            .read_to_string(&mut text)?;
        anyhow::ensure!(
            text.len() <= 4096 && text.trim() == version,
            "the new AppImage reports an unexpected version; the previous version was kept"
        );
        Ok(())
    })();
    let cleanup = std::fs::remove_file(&output_path);
    result?;
    cleanup.map_err(|e| anyhow!("could not remove the startup check output: {e}"))
}

struct ProbeChild(std::process::Child);
impl Drop for ProbeChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            // SAFETY: the probe is the only owner of this dedicated group.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn new_image_must_run_and_report_the_exact_signed_version() {
        let dir = tempfile_for_probe();
        let image = dir.join("probe.AppImage");
        std::fs::write(&image, "#!/bin/sh\nprintf '0.1.16\\n'\n").unwrap();
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(probe(&image, "0.1.16", &cancel::Cancellation::default()).is_ok());
        assert!(probe(&image, "0.1.17", &cancel::Cancellation::default()).is_err());
        std::fs::write(&image, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(probe(&image, "0.1.16", &cancel::Cancellation::default()).is_err());
        assert!(!image.with_extension("probe-output").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn tempfile_for_probe() -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ferestre-update-probe-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
}

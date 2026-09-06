//! The little HTTP this crate does.
//!
//! Two anonymous GETs -- a catalog lookup and an image -- and nothing else.
//! Everything that needs an account goes through the client as a child process,
//! which is where the tokens live and where they stay.
//!
//! Blocking on purpose. Both callers already run these off their main thread,
//! and an async runtime is a large dependency to carry for two requests.

use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// Short, because everything fetched here is decoration. A window that hangs
/// for half a minute because a CDN is sulking is worse than one showing product
/// ids and no art.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Art can be a megabyte and a slow line is not a failure.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// A ceiling, so a wrong URL that streams forever cannot fill a disk.
const MAX_DOWNLOAD: u64 = 16 * 1024 * 1024;

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(concat!("xgdk-launcher/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub fn get_text(url: &str) -> Result<String> {
    agent(TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| anyhow!("{url}: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| anyhow!("{url}: {e}"))
}

/// Fetch to `dest`, atomically. Existing files are left alone.
///
/// Written beside the target and renamed, because the alternative is that a
/// launcher killed mid-download leaves a truncated image that every later run
/// then loads and fails on -- a corruption that outlives the thing that caused
/// it and looks like a bug in the drawing code.
pub fn download(url: &str, dest: &Path) -> Result<()> {
    if dest.is_file() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut reader = agent(DOWNLOAD_TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| anyhow!("{url}: {e}"))?
        .into_body()
        .into_reader()
        .take(MAX_DOWNLOAD);

    let temporary = dest.with_extension("part");
    let mut file = std::fs::File::create(&temporary)?;
    let copied = std::io::copy(&mut reader, &mut file);
    drop(file);
    match copied {
        Ok(_) => {
            std::fs::rename(&temporary, dest)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temporary);
            Err(anyhow!("{url}: {e}"))
        }
    }
}

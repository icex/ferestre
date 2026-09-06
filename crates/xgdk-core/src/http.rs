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

/// Set query parameters on a URL, replacing any that are already there.
///
/// Appending blindly is the obvious thing and it is wrong: an Xbox gamerpic URL
/// already carries `format`, and a second copy of it is answered with 400. The
/// failure is invisible -- an avatar that quietly never appears -- so this
/// replaces rather than appends.
///
/// Deliberately not a URL parser. It splits on the first `?`, walks `&`-joined
/// pairs and leaves every value exactly as it found it, because these URLs
/// carry opaque tokens with `.` and `_` in them that a re-encoding round trip
/// would corrupt.
pub fn with_params(url: &str, params: &[(&str, String)]) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, query),
        None => (url, ""),
    };
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect();

    for (key, value) in params {
        match pairs.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.clone(),
            None => pairs.push((key.to_string(), value.clone())),
        }
    }

    let query: Vec<String> = pairs
        .into_iter()
        .map(|(k, v)| if v.is_empty() { k } else { format!("{k}={v}") })
        .collect();
    if query.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

/// A square image URL at a given pixel size. Both Microsoft image hosts resize
/// server-side, which turns a 1.6 MB avatar into 1.7 KB.
pub fn sized_image(url: &str, px: u32) -> String {
    with_params(
        url,
        &[
            ("format", "png".to_string()),
            ("w", px.to_string()),
            ("h", px.to_string()),
        ],
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_replace_rather_than_pile_up() {
        // The real shape of a gamerpic URL: it already carries `format`, and a
        // second copy is answered with 400.
        let url = "https://images-eds-ssl.xboxlive.com/image?url=aB.c_d&format=png";
        assert_eq!(
            sized_image(url, 48),
            "https://images-eds-ssl.xboxlive.com/image?url=aB.c_d&format=png&w=48&h=48"
        );
        assert_eq!(
            sized_image(&sized_image(url, 48), 64),
            "https://images-eds-ssl.xboxlive.com/image?url=aB.c_d&format=png&w=64&h=64",
            "sizing twice is sizing once"
        );
    }

    #[test]
    fn a_url_with_no_query_gains_one() {
        assert_eq!(
            sized_image("https://store-images.example/image/apps.1", 128),
            "https://store-images.example/image/apps.1?format=png&w=128&h=128"
        );
    }

    /// These URLs carry opaque tokens with `.` and `_` in them, and a
    /// re-encoding round trip would corrupt them.
    #[test]
    fn values_are_left_exactly_as_found() {
        let url = "https://h/i?url=wH.K4gGf7_cq%2Fx&q=90";
        assert_eq!(
            with_params(url, &[("w", "48".into())]),
            "https://h/i?url=wH.K4gGf7_cq%2Fx&q=90&w=48"
        );
    }

    #[test]
    fn a_valueless_parameter_survives() {
        assert_eq!(
            with_params("https://h/i?flag&a=1", &[("b", "2".into())]),
            "https://h/i?flag&a=1&b=2"
        );
    }
}

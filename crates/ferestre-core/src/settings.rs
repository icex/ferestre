//! `settings.toml`: the person's own settings that are not per game.
//!
//! Today only `[dlssnr] dll-url`, the one place a download URL for
//! `nvngx_dlssnr.dll` may come from besides [`DLL_URL_ENV`]. This repository
//! never holds such a URL.
//!
//! The file is edited as a plain TOML table, so tables and keys this build
//! does not know survive a write.

use crate::paths::Paths;
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};

pub const FILE: &str = "settings.toml";
/// Overrides `[dlssnr] dll-url` for one run.
pub const DLL_URL_ENV: &str = "FERESTRE_DLSSNR_DLL_URL";

const TABLE: &str = "dlssnr";
const DLL_URL: &str = "dll-url";

fn file(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// The file as a table. An empty table when there is none; an error naming
/// the file when there is one that cannot be read.
fn read(config_dir: &Path) -> Result<toml::Table> {
    let path = file(config_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(toml::Table::new()),
        Err(e) => bail!("{}: {e}", path.display()),
    };
    text.parse::<toml::Table>()
        .map_err(|e| anyhow!("{}: {e}", path.display()))
}

fn check_https(url: &str) -> Result<()> {
    if !url.starts_with("https://") || url.len() == "https://".len() {
        bail!("{url:?} is not an https:// URL");
    }
    Ok(())
}

/// The DLL URL: [`DLL_URL_ENV`] when set, else `[dlssnr] dll-url`.
pub fn dlssnr_dll_url(paths: &Paths) -> Result<Option<String>> {
    if let Some(url) = paths.var(DLL_URL_ENV) {
        check_https(url).map_err(|e| anyhow!("{DLL_URL_ENV}: {e}"))?;
        return Ok(Some(url.to_string()));
    }
    file_dlssnr_dll_url(paths.config_dir())
}

/// `[dlssnr] dll-url` from the file alone, ignoring the environment: what a
/// settings field shows and saves.
pub fn file_dlssnr_dll_url(config_dir: &Path) -> Result<Option<String>> {
    let table = read(config_dir)?;
    let Some(value) = table.get(TABLE).and_then(|t| t.get(DLL_URL)) else {
        return Ok(None);
    };
    let url = value.as_str().ok_or_else(|| {
        anyhow!(
            "{}: [{TABLE}] {DLL_URL} is not a string",
            file(config_dir).display()
        )
    })?;
    if url.is_empty() {
        return Ok(None);
    }
    check_https(url).map_err(|e| anyhow!("{}: {e}", file(config_dir).display()))?;
    Ok(Some(url.to_string()))
}

/// Set or, with `None`, remove `[dlssnr] dll-url`. Only `https://` URLs.
pub fn set_dlssnr_dll_url(config_dir: &Path, url: Option<&str>) -> Result<()> {
    if let Some(url) = url {
        check_https(url)?;
    }
    let mut table = read(config_dir)?;
    match url {
        Some(url) => {
            let section = table
                .entry(TABLE)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            let Some(section) = section.as_table_mut() else {
                bail!("{}: {TABLE} is not a table", file(config_dir).display());
            };
            section.insert(DLL_URL.into(), toml::Value::String(url.into()));
        }
        None => {
            let now_empty = match table.get_mut(TABLE).and_then(toml::Value::as_table_mut) {
                Some(section) => {
                    section.remove(DLL_URL);
                    section.is_empty()
                }
                None => false,
            };
            if now_empty {
                table.remove(TABLE);
            }
        }
    }
    crate::cache::write(
        &file(config_dir),
        toml::to_string_pretty(&table)?.as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;

    fn paths(home: &Path, env_url: Option<&str>) -> Paths {
        let mut vars = vec![
            ("HOME".to_string(), home.to_str().unwrap().to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                home.join("config").to_str().unwrap().to_string(),
            ),
        ];
        if let Some(url) = env_url {
            vars.push((DLL_URL_ENV.to_string(), url.to_string()));
        }
        Paths::resolve(&Env::from_map(vars)).unwrap()
    }

    #[test]
    fn the_environment_wins_over_the_file() {
        let home = tempfile::tempdir().unwrap();
        let p = paths(home.path(), None);
        set_dlssnr_dll_url(p.config_dir(), Some("https://example.invalid/from-file")).unwrap();
        assert_eq!(
            dlssnr_dll_url(&p).unwrap().as_deref(),
            Some("https://example.invalid/from-file")
        );
        let p = paths(home.path(), Some("https://example.invalid/from-env"));
        assert_eq!(
            dlssnr_dll_url(&p).unwrap().as_deref(),
            Some("https://example.invalid/from-env")
        );
    }

    #[test]
    fn the_file_value_is_read_without_the_environment() {
        let home = tempfile::tempdir().unwrap();
        let p = paths(home.path(), Some("https://example.invalid/from-env"));
        assert_eq!(file_dlssnr_dll_url(p.config_dir()).unwrap(), None);
        set_dlssnr_dll_url(p.config_dir(), Some("https://example.invalid/from-file")).unwrap();
        assert_eq!(
            file_dlssnr_dll_url(p.config_dir()).unwrap().as_deref(),
            Some("https://example.invalid/from-file")
        );
    }

    #[test]
    fn setting_the_url_keeps_other_tables() {
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join(FILE),
            "[other]\nx = 1\n\n[dlssnr]\nfuture = true\n",
        )
        .unwrap();
        set_dlssnr_dll_url(config.path(), Some("https://example.invalid/a")).unwrap();
        let table = read(config.path()).unwrap();
        assert_eq!(table["other"]["x"].as_integer(), Some(1));
        assert_eq!(table["dlssnr"]["future"].as_bool(), Some(true));
        assert_eq!(
            table["dlssnr"][DLL_URL].as_str(),
            Some("https://example.invalid/a")
        );
        set_dlssnr_dll_url(config.path(), None).unwrap();
        let table = read(config.path()).unwrap();
        assert_eq!(table["other"]["x"].as_integer(), Some(1));
        assert!(table["dlssnr"].get(DLL_URL).is_none());
        assert_eq!(file_dlssnr_dll_url(config.path()).unwrap(), None);
    }

    #[test]
    fn a_non_https_url_is_refused() {
        let config = tempfile::tempdir().unwrap();
        set_dlssnr_dll_url(config.path(), Some("https://example.invalid/a")).unwrap();
        let before = std::fs::read(config.path().join(FILE)).unwrap();
        for bad in ["http://example.invalid/a", "file:///tmp/a", "https://", ""] {
            assert!(
                set_dlssnr_dll_url(config.path(), Some(bad)).is_err(),
                "{bad}"
            );
        }
        assert_eq!(std::fs::read(config.path().join(FILE)).unwrap(), before);

        let home = tempfile::tempdir().unwrap();
        let p = paths(home.path(), Some("http://example.invalid/a"));
        let err = dlssnr_dll_url(&p).unwrap_err().to_string();
        assert!(err.contains(DLL_URL_ENV), "{err}");
    }
}

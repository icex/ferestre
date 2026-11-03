//! Small allowlisted diagnostics. Never store child output, arguments or tokens.
use crate::transfers::{private_read, private_write};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

const STAGES: &[&str] = &[
    "authentication",
    "license",
    "metadata-invalid",
    "metadata-network",
    "transfer-io",
    "transfer-cancelled",
    "transfer-range",
    "transfer-status",
    "transfer-eof",
    "transfer-connect",
    "transfer-timeout",
    "transfer-idle",
    "transfer-integrity",
    "disk-space",
    "spawn",
    "backend-exit",
    "installation-busy",
];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub stage: String,
    pub http_status: u16,
}
impl Diagnostic {
    pub fn new(stage: &str, status: u16) -> Option<Self> {
        (STAGES.contains(&stage) && (status == 0 || (100..=599).contains(&status))).then(|| Self {
            stage: stage.into(),
            http_status: status,
        })
    }
    pub fn parse(line: &str) -> Option<Self> {
        if line.len() > 256 {
            return None;
        }
        let mut words = line
            .strip_prefix("FERESTRE_TRANSFER_DIAGNOSTIC_V1 ")?
            .split_ascii_whitespace();
        let result = Self::new(words.next()?, words.next()?.parse().ok()?);
        if words.next().is_some() {
            return None;
        }
        result
    }
    pub fn priority(&self) -> u8 {
        if self.http_status > 0 {
            80
        } else if self.stage == "backend-exit" {
            0
        } else if self.stage == "transfer-io" {
            10
        } else {
            40
        }
    }
    pub fn message(&self) -> String {
        let text = match self.stage.as_str() {
            "authentication" => "Xbox sign-in failed. Sign in again and retry.",
            "license" => "The Store account could not obtain this game's license.",
            "metadata-invalid" => "Package metadata or its signature failed verification.",
            "metadata-network" => {
                "Package metadata could not be downloaded. Check the connection and retry."
            }
            "transfer-range" => "The download server returned an unexpected byte range.",
            "transfer-integrity" => {
                "Downloaded data failed verification. Retry to download it again."
            }
            "transfer-connect" | "transfer-timeout" | "transfer-idle" | "transfer-eof" => {
                "The connection was interrupted. Resume to continue the download."
            }
            "transfer-status" if self.http_status == 401 || self.http_status == 403 => {
                "The download link was refused. Resume to obtain a fresh link."
            }
            "transfer-status" if self.http_status == 429 => {
                "The server is limiting requests. Wait a moment, then resume."
            }
            "disk-space" => "There is not enough free space to finish this update.",
            "installation-busy" => "The game is running or another installer is using its files. Finish that operation and retry.",
            "spawn" => "The download client could not start. Open Diagnostics for details.",
            "transfer-cancelled" => "Download stopped. Progress was kept for resuming.",
            "transfer-io" => {
                "The download could not read or write game data. Check disk space and permissions."
            }
            _ => {
                "The download client failed. Resume to retry; see Diagnostics for its reason code."
            }
        };
        if self.http_status > 0 {
            format!("{text} (HTTP {}; {})", self.http_status, self.stage)
        } else {
            format!("{text} ({})", self.stage)
        }
    }
    pub fn line(&self) -> String {
        format!(
            "FERESTRE_TRANSFER_DIAGNOSTIC_V1 {} {}",
            self.stage, self.http_status
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub unix_seconds: u64,
    pub diagnostic: Diagnostic,
}
pub fn record(state: &Path, diagnostic: &Diagnostic) -> Result<()> {
    if Diagnostic::new(&diagnostic.stage, diagnostic.http_status).is_none() {
        anyhow::bail!("Invalid diagnostic");
    }
    let mut entries = read(state)?;
    entries.push(Entry {
        unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        diagnostic: diagnostic.clone(),
    });
    if entries.len() > 128 {
        entries.drain(..entries.len() - 128);
    }
    private_write(
        state,
        "transfer-diagnostics.json",
        &serde_json::to_vec(&entries)?,
    )
}
pub fn read(state: &Path) -> Result<Vec<Entry>> {
    let entries: Vec<Entry> =
        match private_read(&state.join("transfer-diagnostics.json"), 128 * 1024)? {
            None => vec![],
            Some(bytes) => serde_json::from_slice(&bytes)?,
        };
    anyhow::ensure!(
        entries.len() <= 128
            && entries.iter().all(|e| Diagnostic::new(
                &e.diagnostic.stage,
                e.diagnostic.http_status
            )
            .is_some()),
        "Invalid diagnostic file"
    );
    Ok(entries)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_raw_output_unknown_stages_and_embedded_secrets() {
        for raw in [
            "token=secret",
            "FERESTRE_TRANSFER_DIAGNOSTIC_V1 token 403",
            "FERESTRE_TRANSFER_DIAGNOSTIC_V1 license 0 token=secret",
            "FERESTRE_TRANSFER_DIAGNOSTIC_V1 transfer-status 700",
        ] {
            assert!(Diagnostic::parse(raw).is_none());
        }
        assert!(
            Diagnostic::parse("FERESTRE_TRANSFER_DIAGNOSTIC_V1 transfer-status 403")
                .unwrap()
                .message()
                .contains("fresh link")
        );
    }
    #[test]
    fn busy_installs_are_actionable_and_http_causes_outweigh_generic_io() {
        let busy =
            Diagnostic::parse("FERESTRE_TRANSFER_DIAGNOSTIC_V1 installation-busy 0").unwrap();
        assert!(busy.message().contains("game is running"));
        assert!(
            Diagnostic::new("transfer-status", 403).unwrap().priority()
                > Diagnostic::new("transfer-io", 0).unwrap().priority()
        );
    }
    #[test]
    fn log_is_bounded_and_contains_only_known_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        for _ in 0..140 {
            record(tmp.path(), &Diagnostic::new("license", 0).unwrap()).unwrap();
        }
        assert_eq!(read(tmp.path()).unwrap().len(), 128);
        let text = std::fs::read_to_string(tmp.path().join("transfer-diagnostics.json")).unwrap();
        assert!(!text.contains("token"));
    }
}

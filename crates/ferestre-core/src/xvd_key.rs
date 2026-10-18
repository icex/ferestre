//! Whether the client can check package header signatures.
//!
//! A package's header carries Microsoft's signature, and the client checks it
//! when it has the key that signs retail game packages. Neither the client nor
//! this launcher ships that key: the client fetches it on the person's machine,
//! anonymously, from Microsoft Gaming Services in the Store, and keeps it only
//! if its fingerprint is the one it pins. See the client's `xvd-key` command
//! and docs/PACKAGE-SIGNATURES.md.
//!
//! The launcher only asks the client how things stand, for `ferestre doctor`.
//! It never handles the key, and `xvd-key status` needs no account.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Set to `1`, the client refuses to install a package whose header signature
/// it cannot check.
pub const REQUIRE_SIGNED_ENV: &str = "XODUS_REQUIRE_SIGNED_PACKAGE";
/// Set to `0`, the client never fetches the key on its own.
pub const AUTO_FETCH_ENV: &str = "XODUS_XVD_KEY_AUTOFETCH";

/// The longest a status read may take; it reads one small local file.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TEXT: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// The key is stored and is the pinned one: headers are checked.
    Present,
    /// No key yet: the next install fetches it, unless that is turned off.
    Absent,
    /// A stored key that is not the pinned one, or not private.
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// SHA-256 of the key, when present. Never the key.
    pub fingerprint: Option<String>,
    pub detail: Option<String>,
    /// `XODUS_REQUIRE_SIGNED_PACKAGE=1` is set for the client.
    pub required: bool,
    /// The client fetches the key on the way to an install.
    pub auto_fetch: bool,
    /// When an automatic fetch last failed, as the client wrote it.
    pub last_failed_fetch: Option<String>,
    /// Until when the client skips the automatic fetch because one failed
    /// recently; `None` when it is not paused (or the client does not say).
    pub auto_fetch_paused_until: Option<String>,
}

impl Status {
    /// Whether an install would try to fetch the key now.
    pub fn fetches_on_install(&self) -> bool {
        self.auto_fetch && self.auto_fetch_paused_until.is_none()
    }
}

#[derive(Deserialize)]
struct Raw {
    status: String,
    fingerprint: Option<String>,
    detail: Option<String>,
    #[serde(default)]
    required: bool,
    #[serde(default = "yes")]
    auto_fetch: bool,
    last_failed_fetch: Option<String>,
    auto_fetch_paused_until: Option<String>,
}

fn yes() -> bool {
    true
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Read what `xvd-key status --json` printed.
pub fn parse_status(stdout: &[u8]) -> Result<Status> {
    let raw: Raw = serde_json::from_slice(stdout).context("the client's key status is not JSON")?;
    let state = match raw.status.as_str() {
        "present" => State::Present,
        "absent" => State::Absent,
        "invalid" => State::Invalid,
        other => bail!("the client reports key status {:?}", clean(other)),
    };
    let fingerprint = raw
        .fingerprint
        .filter(|f| f.len() == 64 && f.bytes().all(|b| b.is_ascii_hexdigit()));
    if state == State::Present && fingerprint.is_none() {
        bail!("the client reports a key without a fingerprint");
    }
    Ok(Status {
        state,
        fingerprint,
        detail: raw.detail.map(|d| clean(&d)).filter(|d| !d.is_empty()),
        required: raw.required,
        auto_fetch: raw.auto_fetch,
        last_failed_fetch: raw.last_failed_fetch.map(|t| clean(&t)),
        auto_fetch_paused_until: raw
            .auto_fetch_paused_until
            .map(|t| clean(&t))
            .filter(|t| !t.is_empty()),
    })
}

/// The command that reads the status. It touches no account or keychain.
pub fn status_command(client: &Path) -> Command {
    let mut command = Command::new(client);
    command
        .args(["xvd-key", "status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

/// Ask the client. `Ok(None)` when it is a client without `xvd-key`, which
/// checks no signatures at all.
pub fn status(client: &Path) -> Result<Option<Status>> {
    let mut child = status_command(client)
        .spawn()
        .context("cannot run the client")?;
    let started = Instant::now();
    let exit = loop {
        if let Some(exit) = child.try_wait()? {
            break exit;
        }
        if started.elapsed() > STATUS_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the client did not report its key status in time");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stdout = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        use std::io::Read;
        out.by_ref().take(64 << 10).read_to_end(&mut stdout)?;
    }
    if !exit.success() {
        // clap refuses an unknown subcommand with exit code 2.
        if exit.code() == Some(2) {
            return Ok(None);
        }
        bail!("the client could not report its key status");
    }
    parse_status(&stdout).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "183f0ae05431e4ad91554e88946967c872997227dbe6c85116f5fd2fd2d1229e";

    #[test]
    fn the_clients_answers_are_read() {
        let present = parse_status(
            format!(
                r#"{{"status":"present","fingerprint":"{PIN}","pinned":"{PIN}","required":false,"auto_fetch":true,"dir":"/x","last_failed_fetch":null}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(present.state, State::Present);
        assert_eq!(present.fingerprint.as_deref(), Some(PIN));
        assert!(present.auto_fetch && !present.required);

        let absent = parse_status(
            br#"{"status":"absent","required":true,"auto_fetch":false,"last_failed_fetch":"2026-10-18T10:00:00+00:00"}"#,
        )
        .unwrap();
        assert_eq!(absent.state, State::Absent);
        assert!(absent.required && !absent.auto_fetch);
        assert!(!absent.fetches_on_install());

        let paused = parse_status(
            br#"{"status":"absent","auto_fetch":true,"auto_fetch_paused_until":"2026-10-19T10:00:00+00:00"}"#,
        )
        .unwrap();
        assert!(paused.auto_fetch && !paused.fetches_on_install());
        assert_eq!(
            paused.auto_fetch_paused_until.as_deref(),
            Some("2026-10-19T10:00:00+00:00")
        );
        assert_eq!(
            absent.last_failed_fetch.as_deref(),
            Some("2026-10-18T10:00:00+00:00")
        );

        let invalid =
            parse_status(br#"{"status":"invalid","detail":"not the pinned\u0007 one"}"#).unwrap();
        assert_eq!(invalid.state, State::Invalid);
        assert_eq!(invalid.detail.as_deref(), Some("not the pinned one"));
    }

    #[test]
    fn answers_that_make_no_sense_are_refused() {
        assert!(parse_status(b"not json").is_err());
        assert!(parse_status(br#"{"status":"maybe"}"#).is_err());
        // Present, but with no usable fingerprint.
        assert!(parse_status(br#"{"status":"present"}"#).is_err());
        assert!(parse_status(br#"{"status":"present","fingerprint":"zz"}"#).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_client_is_asked_and_an_old_one_is_recognised() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ferestre-xvd-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let new = script(
            "new",
            &format!(
                r#"[ "$1 $2 $3" = "xvd-key status --json" ] || exit 9
echo '{{"status":"present","fingerprint":"{PIN}"}}'"#
            ),
        );
        let status_now = status(&new).unwrap().unwrap();
        assert_eq!(status_now.state, State::Present);

        let old = script(
            "old",
            "echo \"error: unrecognized subcommand 'xvd-key'\" >&2; exit 2",
        );
        assert_eq!(status(&old).unwrap(), None);

        let broken = script("broken", "exit 1");
        assert!(status(&broken).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

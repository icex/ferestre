//! Who is signed in.
//!
//! The launcher never touches a token. Signing in, keeping the session and
//! minting anything is the client's job, and this module only asks it questions
//! and reads the answers -- which is the licence boundary and also the right
//! shape: an account is exactly the thing that should have one owner.
//!
//! Identifiers are masked by default everywhere they are shown. A gamertag is
//! a name someone chose to be known by; an XUID is not, and it turns up in
//! logs, screenshots and issue reports that people paste in public.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The signed-in account, as much of it as could be found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Always present when signed in: it is a claim on the token itself.
    pub gamertag: String,
    #[serde(default)]
    pub xuid: String,
    /// Absent when the profile service could not be reached. The identity still
    /// works; only the picture is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamerpic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
}

impl Account {
    /// The XUID with its digits hidden. Length is kept because "is there one at
    /// all" is the only thing anyone reads it for, and the digits themselves
    /// are what should never end up in a pasted log.
    pub fn masked_xuid(&self) -> String {
        "•".repeat(self.xuid.chars().count())
    }

    /// What to show beside the avatar. Gamertag alone: the tier is Microsoft's
    /// word for a subscription and means nothing to a launcher.
    pub fn label(&self) -> &str {
        &self.gamertag
    }
}

/// Ask the client who is signed in. Fails when nobody is.
pub fn command(xodus_cli: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(xodus_cli);
    command.arg("account").arg("--json");
    command
}

/// Start an interactive sign-in.
///
/// It is interactive: the client opens a browser and waits. That is why this
/// hands back a command instead of running one -- a GUI has to attach it to a
/// terminal or a window rather than let it block a redraw.
pub fn login_command(xodus_cli: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(xodus_cli);
    command.arg("login");
    command
}

pub fn logout_command(xodus_cli: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(xodus_cli);
    command.arg("logout");
    command
}

/// Parse `xodus-cli account --json`.
pub fn parse(json: &str) -> Result<Account> {
    let account: Account =
        serde_json::from_str(json.trim()).map_err(|e| anyhow!("not an account document: {e}"))?;
    if account.gamertag.is_empty() {
        return Err(anyhow!("signed in, but the token carries no gamertag"));
    }
    Ok(account)
}

/// Where the avatar is cached.
///
/// Named for the size and nothing else. The gamerpic URL carries an opaque
/// token rather than an account id, and keeping the XUID out of a filename
/// means a cache directory can be shown, tarred or shared without leaking one.
///
/// No extension that claims a format: the picture host answers `format=png`
/// with a JPEG, and a `.png` full of JPEG is the kind of thing that works
/// everywhere until it meets something that trusts the name.
pub fn avatar_path(cache_dir: &Path, px: u32) -> PathBuf {
    cache_dir.join(format!("gamerpic-{px}.img"))
}

/// Sizes the gamerpic host will actually serve.
///
/// It is an allowlist, not a range, and anything else is answered with **400**
/// -- 48 fails where 40 and 64 succeed. Measured against the live service,
/// because nothing documents it, and the failure mode is an avatar that simply
/// never appears.
const AVATAR_SIZES: [u32; 5] = [40, 64, 100, 128, 208];

/// The smallest offered size that is at least `px`, so the picture is never
/// upscaled into a blur. Falls back to the largest when asked for more.
pub fn supported_avatar_size(px: u32) -> u32 {
    AVATAR_SIZES
        .into_iter()
        .find(|size| *size >= px)
        .unwrap_or(AVATAR_SIZES[AVATAR_SIZES.len() - 1])
}

/// Fetch the avatar at the size it will be drawn, and say where it went.
pub fn avatar(cache_dir: &Path, account: &Account, px: u32) -> Result<PathBuf> {
    let url = account
        .gamerpic
        .as_deref()
        .ok_or_else(|| anyhow!("no gamerpic for this account"))?;
    let px = supported_avatar_size(px);
    let dest = avatar_path(cache_dir, px);
    // The host resizes, so a 64px avatar is a 1.7 KB download instead of the
    // 1.6 MB the unsized URL returns. The size goes on with `with_params`, not
    // by appending: this URL already carries `format`, and a duplicate of it is
    // answered with 400 too.
    crate::http::download(&crate::http::sized_image(url, px), &dest)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like the client's output. Every identifier here is invented.
    const PAYLOAD: &str = r#"{
      "gamerpic": "https://images-eds-ssl.xboxlive.com/image?url=ZZZZtest",
      "gamertag": "TestPlayer",
      "modern_gamertag": "TestPlayer",
      "tier": "Gold",
      "xuid": "2500000000000000"
    }"#;

    #[test]
    fn the_client_output_becomes_an_account() {
        let account = parse(PAYLOAD).expect("parses");
        assert_eq!(account.gamertag, "TestPlayer");
        assert_eq!(account.xuid, "2500000000000000");
        assert_eq!(account.tier.as_deref(), Some("Gold"));
        assert_eq!(account.label(), "TestPlayer");
    }

    /// The profile service is best-effort; the identity is not.
    #[test]
    fn an_account_without_a_picture_is_still_an_account() {
        let account = parse(r#"{"gamertag":"TestPlayer","xuid":"25"}"#).expect("parses");
        assert!(account.gamerpic.is_none());
        assert!(avatar(Path::new("/nonexistent"), &account, 64).is_err());
    }

    #[test]
    fn no_gamertag_is_not_a_signed_in_account() {
        assert!(parse(r#"{"gamertag":"","xuid":"25"}"#).is_err());
        assert!(parse("nobody is signed in").is_err());
    }

    /// An XUID turns up in screenshots and pasted logs. Its length is all
    /// anyone reads it for.
    #[test]
    fn the_xuid_is_masked_but_keeps_its_shape() {
        let account = parse(PAYLOAD).expect("parses");
        let masked = account.masked_xuid();
        assert_eq!(masked.chars().count(), 16);
        assert!(!masked.contains(|c: char| c.is_ascii_digit()), "{masked}");
    }

    /// A cache directory should be shareable without leaking whose it is.
    #[test]
    fn the_avatar_filename_names_a_size_not_a_person() {
        let path = avatar_path(Path::new("/cache"), 64);
        assert_eq!(path, PathBuf::from("/cache/gamerpic-64.img"));
    }

    /// The host serves an allowlist of sizes and answers anything else with
    /// 400, which shows up as an avatar that silently never appears.
    #[test]
    fn an_unserved_size_is_rounded_up_to_one_that_is_served() {
        assert_eq!(supported_avatar_size(48), 64, "48 is a 400; 64 is not");
        assert_eq!(supported_avatar_size(32), 40);
        assert_eq!(supported_avatar_size(64), 64, "an exact size is left alone");
        assert_eq!(supported_avatar_size(1), 40);
        assert_eq!(supported_avatar_size(4096), 208, "the largest, not a 400");
        for px in [1, 32, 48, 64, 65, 300] {
            assert!(
                AVATAR_SIZES.contains(&supported_avatar_size(px)),
                "asked for {px}"
            );
        }
    }

    #[test]
    fn the_commands_are_the_clients_own() {
        let spelled = |c: Command| {
            std::iter::once(c.get_program().to_string_lossy().to_string())
                .chain(c.get_args().map(|a| a.to_string_lossy().to_string()))
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(spelled(command("xodus-cli")), "xodus-cli account --json");
        assert_eq!(spelled(login_command("xodus-cli")), "xodus-cli login");
        assert_eq!(spelled(logout_command("xodus-cli")), "xodus-cli logout");
    }
}

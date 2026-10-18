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

/// Public account metadata used to bind the cached library to its owner.
/// Tokens remain exclusively in the client's keyring.
pub fn cache_path(state_dir: &Path) -> PathBuf {
    state_dir.join("account.json")
}

pub fn load_cache(state_dir: &Path) -> Option<Account> {
    parse(&std::fs::read_to_string(cache_path(state_dir)).ok()?).ok()
}

pub fn save_cache(state_dir: &Path, account: &Account) -> Result<()> {
    crate::cache::write(&cache_path(state_dir), &serde_json::to_vec(account)?)
}

pub fn same_identity(left: &Account, right: &Account) -> bool {
    if !left.xuid.is_empty() && !right.xuid.is_empty() {
        left.xuid == right.xuid
    } else {
        left.gamertag == right.gamertag
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

/// Which picture the cached avatar at this size is: a hash of the URL it was
/// fetched from, beside it. The URL changes when the picture does and differs
/// between accounts, and its hash says neither who nor what.
fn avatar_source_path(cache_dir: &Path, px: u32) -> PathBuf {
    cache_dir.join(format!("gamerpic-{px}.source"))
}

fn avatar_source(url: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(url.as_bytes()))
}

fn sized_gamerpic(account: &Account, px: u32) -> Option<String> {
    let url = account.gamerpic.as_deref()?;
    // The host resizes, so a 64px avatar is a 1.7 KB download instead of the
    // 1.6 MB the unsized URL returns. The size goes on with `with_params`, not
    // by appending: this URL already carries `format`, and a duplicate of it is
    // answered with 400 too.
    Some(crate::http::sized_image(url, px))
}

/// The avatar already on disk for this account, if it is this account's.
///
/// The file is named for its size only, so without the recorded source it
/// would show whichever account last fetched one -- the previous person's
/// picture, on a shared machine -- and never notice a new gamerpic.
pub fn cached_avatar(cache_dir: &Path, account: &Account, px: u32) -> Option<PathBuf> {
    let px = supported_avatar_size(px);
    let url = sized_gamerpic(account, px)?;
    let dest = avatar_path(cache_dir, px);
    let recorded = std::fs::read_to_string(avatar_source_path(cache_dir, px)).ok()?;
    (recorded.trim() == avatar_source(&url) && dest.is_file()).then_some(dest)
}

/// Fetch the avatar at the size it will be drawn, and say where it went.
///
/// A cached picture is reused only when it was fetched from this account's
/// current gamerpic URL; otherwise it is replaced.
pub fn avatar(cache_dir: &Path, account: &Account, px: u32) -> Result<PathBuf> {
    let px = supported_avatar_size(px);
    let url = sized_gamerpic(account, px).ok_or_else(|| anyhow!("no gamerpic for this account"))?;
    let dest = avatar_path(cache_dir, px);
    if let Some(cached) = cached_avatar(cache_dir, account, px) {
        // Still checked: a cached file that is not a picture is fetched again.
        crate::http::download(&url, &cached)?;
        return Ok(cached);
    }
    // Forget the old source first, so a failure part way never leaves a new
    // picture labelled as the old one or the other way round.
    let source = avatar_source_path(cache_dir, px);
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&dest);
    crate::http::download(&url, &dest)?;
    crate::cache::write(&source, avatar_source(&url).as_bytes())?;
    Ok(dest)
}

/// Remove every cached avatar, for a sign-out: the next account to sign in
/// must not be shown this one's picture, even briefly.
pub fn forget_avatars(cache_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("gamerpic-") && (name.ends_with(".img") || name.ends_with(".source")) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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

    /// One account signs out and another signs in on the same machine. The
    /// first one's picture must not be reused for the second, and a new
    /// gamerpic must replace the old one.
    #[test]
    fn a_cached_avatar_belongs_to_the_account_it_was_fetched_for() {
        use std::io::{Read, Write};
        const PNG: &[u8] = include_bytes!("../tests/fixtures/steam-art.png");
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = server.accept().unwrap();
                let mut request = [0; 4096];
                let n = stream.read(&mut request).unwrap();
                let line = String::from_utf8_lossy(&request[..n]).to_string();
                paths.push(line.split_whitespace().nth(1).unwrap().to_string());
                let body: Vec<u8> = PNG
                    .iter()
                    .copied()
                    .chain(paths.len().to_le_bytes())
                    .collect();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
            paths
        });
        let dir = tempfile::tempdir().unwrap();
        let account = |pic: &str| Account {
            gamertag: pic.into(),
            xuid: String::new(),
            gamerpic: Some(format!("{base}/image?url={pic}")),
            tier: None,
        };
        let first = account("first");
        let second = account("second");

        let path = avatar(dir.path(), &first, 64).unwrap();
        let first_picture = std::fs::read(&path).unwrap();
        assert_eq!(cached_avatar(dir.path(), &first, 64), Some(path.clone()));
        assert_eq!(
            cached_avatar(dir.path(), &second, 64),
            None,
            "the first account's picture is not the second's"
        );
        // Cached for the same account: no request.
        assert_eq!(avatar(dir.path(), &first, 64).unwrap(), path);

        let path = avatar(dir.path(), &second, 64).unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), first_picture);
        assert_eq!(cached_avatar(dir.path(), &second, 64), Some(path));
        assert_eq!(cached_avatar(dir.path(), &first, 64), None);
        let paths = worker.join().unwrap();
        assert!(
            paths[0].contains("first") && paths[1].contains("second"),
            "{paths:?}"
        );

        forget_avatars(dir.path());
        assert_eq!(cached_avatar(dir.path(), &second, 64), None);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
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

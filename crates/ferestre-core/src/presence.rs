//! Showing friends the game being played.
//!
//! The launcher never posts presence itself. Posting needs a token, and tokens
//! belong to the client (see `account`). The client's `presence run` is a
//! long-running child that tells Xbox a game is being played, using that
//! game's own title identity, for as long as its stdin stays open. It prints
//! what happens as JSON lines, and closing its stdin takes the person offline
//! again. `presence get` is a one-shot read of what Xbox currently shows.
//!
//! This module holds what the CLI and the window share: whether the Player
//! wants it, what the client said, and what the badge and the menu say.
//! Nothing here starts a process; the callers decide when.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::accounts::{Profile, Profiles, Selection};

/// What `account-profiles --json` lists when the client has `presence`.
pub const CLIENT_CAPABILITY: &str = "presence-v1";

/// How a launch passes the runner's events on, one per line on its stderr.
/// It starts with "::" so a launch failure is never read from it.
pub const RELAY_PREFIX: &str = ":: presence ";

/// `presence run` exits with this when another runner already holds the
/// Player's lock. Not a failure: somebody else is already saying it.
pub const BUSY_EXIT: i32 = 3;

/// A title or a reason is the client's text, shown in a label. Longer than
/// anything Xbox names a game, short enough that a runaway line is cut.
const MAX_TEXT: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    pub schema: u32,
    #[serde(default)]
    pub players: BTreeMap<String, PlayerPreference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerPreference {
    pub share_games: bool,
}

pub fn preferences_path(config_dir: &Path) -> PathBuf {
    config_dir.join("presence.json")
}

/// The saved choices. A missing file is `None`; a file that cannot be read,
/// or that a newer launcher wrote in another shape, is an error.
pub fn load(config_dir: &Path) -> Result<Option<Preferences>> {
    let bytes = match std::fs::read(preferences_path(config_dir)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let preferences: Preferences =
        serde_json::from_slice(&bytes).context("Invalid online status settings")?;
    if preferences.schema != 1 {
        bail!("Online status settings are from another version");
    }
    Ok(Some(preferences))
}

/// Whether this Player shows friends the games they play from Ferestre.
///
/// On until they switch it off. A settings file that cannot be read counts as
/// off: it may hold an "off" that would otherwise be ignored, and showing
/// somebody online against their wishes is the worse mistake.
pub fn sharing(config_dir: &Path, player: &str) -> bool {
    match load(config_dir) {
        Ok(None) => true,
        Ok(Some(preferences)) => preferences
            .players
            .get(player)
            .is_none_or(|p| p.share_games),
        Err(_) => false,
    }
}

/// Save the switch for one Player. Other Players' choices are kept; a file
/// that could not be read is replaced, since this is the person deciding.
pub fn set_sharing(config_dir: &Path, player: &str, on: bool) -> Result<()> {
    if !crate::accounts::valid_profile_id(player) {
        bail!("Invalid Player account");
    }
    let mut preferences = load(config_dir).ok().flatten().unwrap_or(Preferences {
        schema: 1,
        players: BTreeMap::new(),
    });
    preferences
        .players
        .insert(player.to_string(), PlayerPreference { share_games: on });
    crate::cache::write(
        &preferences_path(config_dir),
        &serde_json::to_vec(&preferences)?,
    )
}

/// Whether the client can do any of this.
pub fn supported(profiles: &Profiles) -> bool {
    profiles.supports(CLIENT_CAPABILITY)
}

/// The account friends would see: the selected Player, or the client's
/// default account when no roles are chosen. Never the Store account by
/// accident: the inherited `XODUS_ACCOUNT_PROFILE` is the Store's.
pub fn player<'a>(profiles: &'a Profiles, roles: Option<&Selection>) -> Option<&'a Profile> {
    let id = roles
        .map(|r| r.player.as_str())
        .or(profiles.default_profile.as_deref())?;
    profiles.profiles.iter().find(|p| p.id == id)
}

fn for_player(client: &OsStr, player: &str) -> Command {
    let mut command = Command::new(client);
    command.arg("--profile").arg(player);
    // The flag is the only selector. Launch variables name the Store account
    // and must not decide whose status this is.
    for name in [
        "XODUS_ACCOUNT_PROFILE",
        "XODUS_STORE_PROFILE",
        "XODUS_PLAYER_PROFILE",
    ] {
        command.env_remove(name);
    }
    command
}

/// The long-running child that keeps "Playing <game>" up while it runs.
pub fn run_command(client: impl AsRef<OsStr>, player: &str, game_dir: &Path) -> Command {
    let mut command = for_player(client.as_ref(), player);
    command
        .args(["presence", "run", "--json", "--game-dir"])
        .arg(game_dir);
    command
}

/// One read of what Xbox currently shows for this Player.
pub fn get_command(client: impl AsRef<OsStr>, player: &str) -> Command {
    let mut command = for_player(client.as_ref(), player);
    command.args(["presence", "get", "--json"]);
    command
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Unauthorized,
    Forbidden,
    SignedOut,
    Network,
    NoTitleIdentity,
    Other(String),
}

impl ErrorCode {
    fn parse(code: &str) -> Self {
        match code {
            "unauthorized" => Self::Unauthorized,
            "forbidden" => Self::Forbidden,
            "signed-out" => Self::SignedOut,
            "network" => Self::Network,
            "no-title-identity" => Self::NoTitleIdentity,
            other => Self::Other(clean(other)),
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::SignedOut => "signed-out",
            Self::Network => "network",
            Self::NoTitleIdentity => "no-title-identity",
            Self::Other(code) => code,
        }
    }

    /// One sentence for the menu and the tooltip.
    pub fn message(&self) -> String {
        match self {
            Self::Unauthorized => "Xbox did not accept the sign-in. Sign in again.".into(),
            Self::Forbidden => "Xbox refused to show this game's status".into(),
            Self::SignedOut => "Sign in the Player account to share your status".into(),
            Self::Network => "Cannot reach Xbox".into(),
            Self::NoTitleIdentity => "Online status unavailable for this game".into(),
            Self::Other(code) => format!("Online status failed ({code})"),
        }
    }
}

/// A runner's reason for retrying, in words. The client sends short codes;
/// anything else is shown as it came.
pub fn describe_retry(reason: &str) -> String {
    match reason {
        "" => "Reconnecting to Xbox".into(),
        "network" => "Cannot reach Xbox".into(),
        "throttled" => "Xbox asked to wait".into(),
        "unavailable" => "Xbox is unavailable".into(),
        "sign-in" => "Could not sign the game in".into(),
        other => format!("Xbox answered: {other}"),
    }
}

/// Why a launch did not start a runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disabled {
    /// The Player switched it off.
    Off,
    SignedOut,
}

/// A line from `presence run`, or one the launch adds around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Online {
        title: String,
        heartbeat_after: Option<u64>,
    },
    Retrying {
        reason: String,
        in_secs: Option<u64>,
    },
    Error {
        fatal: bool,
        code: ErrorCode,
    },
    Offline,
    Busy,
    /// Launch side: a runner was started.
    Starting,
    /// Launch side: no runner, and why.
    Disabled(Disabled),
    /// Launch side: the runner process has exited.
    Ended,
}

/// Control characters and length cut from text that ends up in a label.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Read one line. Anything unknown or malformed is `None`, and ignored: a
/// newer client may say more than this launcher understands.
pub fn parse_event(line: &str) -> Option<Event> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let object = value.as_object()?;
    if object.get("schema").is_some_and(|s| s != 1) {
        return None;
    }
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(clean);
    let number = |key: &str| object.get(key).and_then(Value::as_u64);
    Some(match object.get("event")?.as_str()? {
        "online" => Event::Online {
            title: text("title").filter(|t| !t.is_empty())?,
            heartbeat_after: number("heartbeat_after"),
        },
        "retrying" => Event::Retrying {
            reason: text("reason").unwrap_or_default(),
            in_secs: number("in"),
        },
        "error" => Event::Error {
            fatal: object.get("fatal").and_then(Value::as_bool).unwrap_or(true),
            code: ErrorCode::parse(object.get("code").and_then(Value::as_str)?),
        },
        "offline" => Event::Offline,
        "busy" => Event::Busy,
        "starting" => Event::Starting,
        "disabled" => Event::Disabled(match object.get("reason")?.as_str()? {
            "off" => Disabled::Off,
            "signed-out" => Disabled::SignedOut,
            _ => return None,
        }),
        "ended" => Event::Ended,
        _ => return None,
    })
}

/// The event as a line, rebuilt from the fields this launcher knows. A relay
/// never passes the client's text through whole, so nothing it did not mean
/// to say can reach a log.
pub fn event_json(event: &Event) -> String {
    let value = match event {
        Event::Online {
            title,
            heartbeat_after,
        } => json!({"schema":1,"event":"online","title":title,"heartbeat_after":heartbeat_after}),
        Event::Retrying { reason, in_secs } => {
            json!({"event":"retrying","reason":reason,"in":in_secs})
        }
        Event::Error { fatal, code } => {
            json!({"event":"error","fatal":fatal,"code":code.as_str()})
        }
        Event::Offline => json!({"event":"offline"}),
        Event::Busy => json!({"event":"busy"}),
        Event::Starting => json!({"event":"starting"}),
        Event::Disabled(reason) => json!({"event":"disabled","reason":match reason {
            Disabled::Off => "off",
            Disabled::SignedOut => "signed-out",
        }}),
        Event::Ended => json!({"event":"ended"}),
    };
    value.to_string()
}

pub fn relay_line(event: &Event) -> String {
    format!("{RELAY_PREFIX}{}", event_json(event))
}

pub fn parse_relay(line: &str) -> Option<Event> {
    parse_event(line.strip_prefix(RELAY_PREFIX)?)
}

/// What one launch's runner is doing, as far as its events say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Runner {
    /// No runner, or one that has finished without a lasting error.
    #[default]
    Idle,
    Starting,
    Online(String),
    Retrying(String),
    /// Stopped for good on this launch until somebody acts.
    Failed(ErrorCode),
}

impl Runner {
    pub fn apply(&mut self, event: &Event) {
        *self = match event {
            Event::Starting => Runner::Starting,
            Event::Online { title, .. } => Runner::Online(title.clone()),
            Event::Retrying { reason, .. } => Runner::Retrying(describe_retry(reason)),
            Event::Error { fatal: false, code } => Runner::Retrying(code.message()),
            Event::Error { fatal: true, code } => Runner::Failed(code.clone()),
            // A fatal error outlives the process that reported it.
            Event::Ended if matches!(self, Runner::Failed(_)) => return,
            Event::Offline | Event::Busy | Event::Disabled(_) | Event::Ended => Runner::Idle,
        }
    }

    /// Whether a runner process may still be up and posting.
    pub fn live(&self) -> bool {
        matches!(
            self,
            Runner::Starting | Runner::Online(_) | Runner::Retrying(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Online,
    Away,
    Offline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceTitle {
    pub device: String,
    pub name: String,
}

/// What `presence get` said Xbox shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub titles: Vec<DeviceTitle>,
}

pub fn parse_status(bytes: &[u8]) -> Result<Status> {
    let value: Value = serde_json::from_slice(bytes).context("Invalid online status")?;
    if value["schema"] != 1 {
        bail!("Online status from another client version");
    }
    let state = match value["state"].as_str() {
        Some("Online") => State::Online,
        Some("Away") => State::Away,
        Some("Offline") => State::Offline,
        _ => bail!("Unknown online state"),
    };
    let mut titles = Vec::new();
    for device in value["devices"].as_array().into_iter().flatten() {
        let kind = device["type"].as_str().map(clean).unwrap_or_default();
        for title in device["titles"].as_array().into_iter().flatten() {
            if let Some(name) = title["name"].as_str().map(clean).filter(|n| !n.is_empty()) {
                titles.push(DeviceTitle {
                    device: kind.clone(),
                    name,
                });
            }
        }
    }
    Ok(Status { state, titles })
}

/// The last answer from `presence get`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Server {
    #[default]
    Unknown,
    Failed,
    Known(Status),
}

/// Whether the switch can do anything for this Player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Availability {
    #[default]
    Checking,
    /// The client has no `presence-v1`.
    Unsupported,
    /// No Player account, or it is signed out.
    SignedOut,
    Ready,
}

/// The dot and the words beside the account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Badge {
    Hidden,
    Checking,
    Unknown,
    /// Sharing is off and Xbox shows nothing.
    Off,
    Offline,
    /// Online, and not because of a game this launcher reported.
    Online(Vec<DeviceTitle>),
    Away,
    Playing(String),
    Reconnecting(String),
    Problem(String),
}

impl Badge {
    pub fn label(&self) -> Option<String> {
        Some(match self {
            Badge::Hidden => return None,
            Badge::Checking => "Checking".into(),
            Badge::Unknown => "Unknown".into(),
            Badge::Off => "Off".into(),
            Badge::Offline => "Offline".into(),
            Badge::Online(_) => "Online".into(),
            Badge::Away => "Away".into(),
            Badge::Playing(title) => format!("Playing {title}"),
            Badge::Reconnecting(_) => "Reconnecting".into(),
            Badge::Problem(_) => "Not shared".into(),
        })
    }

    /// The modifier class on `presence-dot`; grey without one.
    pub fn css_class(&self) -> Option<&'static str> {
        match self {
            Badge::Online(_) => Some("online"),
            Badge::Away => Some("away"),
            Badge::Playing(_) => Some("ingame"),
            Badge::Checking | Badge::Reconnecting(_) => Some("pending"),
            Badge::Problem(_) => Some("failed"),
            _ => None,
        }
    }

    pub fn tooltip(&self) -> Option<String> {
        let label = self.label()?;
        let detail = match self {
            Badge::Online(titles) if titles.is_empty() => "Online on another device".to_string(),
            Badge::Online(titles) => format!(
                "Online on another device: {}",
                titles
                    .iter()
                    .map(|t| if t.device.is_empty() {
                        t.name.clone()
                    } else {
                        format!("{} on {}", t.name, t.device)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Badge::Off => "Not sharing the games you play".into(),
            Badge::Unknown => "Could not check your status".into(),
            Badge::Reconnecting(reason) if !reason.is_empty() => reason.clone(),
            Badge::Problem(reason) => reason.clone(),
            _ => return Some(format!("Status: {label}")),
        };
        Some(format!("Status: {label}\n{detail}"))
    }
}

/// The runner that says the most, among this Player's launches: a game being
/// shown beats one reconnecting, which beats one that failed.
pub fn strongest<'a>(runners: impl IntoIterator<Item = &'a Runner>) -> Option<&'a Runner> {
    let rank = |r: &Runner| match r {
        Runner::Online(_) => 4,
        Runner::Retrying(_) => 3,
        Runner::Failed(_) => 2,
        Runner::Starting => 1,
        Runner::Idle => 0,
    };
    runners
        .into_iter()
        .filter(|r| rank(r) > 0)
        .max_by_key(|r| rank(r))
}

/// What the badge shows. The runner speaks for the game it is reporting;
/// otherwise Xbox's own answer does, whatever the switch says, because friends
/// can see a console even when this launcher shares nothing.
pub fn badge(
    availability: Availability,
    sharing: bool,
    runner: Option<&Runner>,
    server: &Server,
) -> Badge {
    if availability != Availability::Ready {
        return Badge::Hidden;
    }
    match runner {
        Some(Runner::Online(title)) => return Badge::Playing(title.clone()),
        Some(Runner::Retrying(reason)) => return Badge::Reconnecting(reason.clone()),
        Some(Runner::Failed(code)) => return Badge::Problem(code.message()),
        _ => {}
    }
    match server {
        Server::Known(status) => match status.state {
            State::Online => Badge::Online(status.titles.clone()),
            State::Away => Badge::Away,
            State::Offline if sharing => Badge::Offline,
            State::Offline => Badge::Off,
        },
        _ if !sharing => Badge::Hidden,
        Server::Failed => Badge::Unknown,
        Server::Unknown => Badge::Checking,
    }
}

/// The line under the switch.
pub fn caption(availability: Availability, sharing: bool, runner: Option<&Runner>) -> String {
    match availability {
        Availability::Checking => return "Checking your status…".into(),
        Availability::Unsupported => return "Update the Xodus client to share your status".into(),
        Availability::SignedOut => return "Sign in the Player account to share your status".into(),
        Availability::Ready => {}
    }
    if !sharing {
        return "Friends see your status from other devices only".into();
    }
    match runner {
        Some(Runner::Online(title)) => format!("Playing {title}"),
        Some(Runner::Retrying(_)) => "Reconnecting to Xbox…".into(),
        Some(Runner::Failed(code)) => code.message(),
        _ => "Friends will see the games you play from Ferestre".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "11111111111111111111111111111111";
    const PLAYER: &str = "22222222222222222222222222222222";

    fn profiles(capabilities: &[&str], player_signed_in: bool) -> Profiles {
        let json = json!({"schema":1,"capabilities":capabilities,"default_profile":OWNER,
            "profiles":[{"id":OWNER,"label":"Owner","signed_in":true,"legacy_saves":true,"save_namespace":"a".repeat(64)},
                        {"id":PLAYER,"label":"Player","signed_in":player_signed_in,"legacy_saves":false,"save_namespace":"b".repeat(64)}]});
        crate::accounts::parse_profiles(&serde_json::to_vec(&json).unwrap()).unwrap()
    }

    #[test]
    fn sharing_is_on_until_switched_off_and_kept_per_player() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(load(root.path()).unwrap(), None);
        assert!(sharing(root.path(), PLAYER));
        set_sharing(root.path(), PLAYER, false).unwrap();
        assert!(!sharing(root.path(), PLAYER));
        assert!(sharing(root.path(), OWNER));
        set_sharing(root.path(), OWNER, true).unwrap();
        let saved = load(root.path()).unwrap().unwrap();
        assert_eq!(saved.schema, 1);
        assert_eq!(saved.players.len(), 2);
        assert!(!saved.players[PLAYER].share_games);
        assert!(set_sharing(root.path(), "../escape", true).is_err());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(preferences_path(root.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn unreadable_settings_count_as_off_and_are_replaced_on_the_next_choice() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(preferences_path(root.path()), b"{not json").unwrap();
        assert!(load(root.path()).is_err());
        assert!(!sharing(root.path(), PLAYER));
        std::fs::write(
            preferences_path(root.path()),
            br#"{"schema":2,"players":{}}"#,
        )
        .unwrap();
        assert!(load(root.path()).is_err());
        assert!(!sharing(root.path(), PLAYER));
        set_sharing(root.path(), PLAYER, true).unwrap();
        assert!(sharing(root.path(), PLAYER));
        // Fields a later version adds are read past, not refused.
        std::fs::write(
            preferences_path(root.path()),
            format!(r#"{{"schema":1,"players":{{"{PLAYER}":{{"share_games":false,"idle_online":true}}}}}}"#),
        )
        .unwrap();
        assert!(!sharing(root.path(), PLAYER));
    }

    #[test]
    fn every_runner_line_is_read_and_anything_else_ignored() {
        assert_eq!(
            parse_event(r#"{"schema":1,"event":"online","title":"Halo","heartbeat_after":300}"#),
            Some(Event::Online {
                title: "Halo".into(),
                heartbeat_after: Some(300)
            })
        );
        assert_eq!(
            parse_event(r#"{"event":"retrying","reason":"HTTP 503","in":30}"#),
            Some(Event::Retrying {
                reason: "HTTP 503".into(),
                in_secs: Some(30)
            })
        );
        for (code, parsed) in [
            ("unauthorized", ErrorCode::Unauthorized),
            ("forbidden", ErrorCode::Forbidden),
            ("signed-out", ErrorCode::SignedOut),
            ("network", ErrorCode::Network),
            ("no-title-identity", ErrorCode::NoTitleIdentity),
            ("teapot", ErrorCode::Other("teapot".into())),
        ] {
            let line = format!(r#"{{"event":"error","fatal":true,"code":"{code}"}}"#);
            assert_eq!(
                parse_event(&line),
                Some(Event::Error {
                    fatal: true,
                    code: parsed
                })
            );
        }
        assert_eq!(
            parse_event(r#"{"event":"error","fatal":false,"code":"network"}"#),
            Some(Event::Error {
                fatal: false,
                code: ErrorCode::Network
            })
        );
        assert_eq!(parse_event(r#"{"event":"offline"}"#), Some(Event::Offline));
        assert_eq!(parse_event(r#"{"event":"busy"}"#), Some(Event::Busy));
        for ignored in [
            "",
            "not json",
            "[1,2]",
            r#"{"event":"dance"}"#,
            r#"{"event":"online"}"#,
            r#"{"event":"online","title":""}"#,
            r#"{"schema":2,"event":"offline"}"#,
            r#"{"event":"disabled","reason":"bored"}"#,
            r#"{"event":7}"#,
        ] {
            assert_eq!(parse_event(ignored), None, "{ignored}");
        }
        // Text for a label loses control characters and its excess.
        let long = format!(
            r#"{{"event":"online","title":"A\u0007B{}"}}"#,
            "x".repeat(500)
        );
        let Some(Event::Online { title, .. }) = parse_event(&long) else {
            panic!("long title")
        };
        assert!(title.starts_with("AB") && title.chars().count() == MAX_TEXT);
    }

    #[test]
    fn relayed_lines_round_trip_and_carry_only_known_fields() {
        let events = [
            Event::Online {
                title: "Halo".into(),
                heartbeat_after: None,
            },
            Event::Retrying {
                reason: "timeout".into(),
                in_secs: Some(15),
            },
            Event::Error {
                fatal: true,
                code: ErrorCode::Forbidden,
            },
            Event::Offline,
            Event::Busy,
            Event::Starting,
            Event::Disabled(Disabled::Off),
            Event::Disabled(Disabled::SignedOut),
            Event::Ended,
        ];
        for event in events {
            let line = relay_line(&event);
            assert!(line.starts_with(":: presence {"), "{line}");
            assert_eq!(parse_relay(&line), Some(event));
        }
        let line = r#"{"event":"online","title":"Halo","token":"secret"}"#;
        let relayed = relay_line(&parse_event(line).unwrap());
        assert!(!relayed.contains("secret"), "{relayed}");
        assert_eq!(parse_relay("-- presence {\"event\":\"busy\"}"), None);
    }

    #[test]
    fn the_capability_and_the_player_come_from_the_profile_listing() {
        let old = profiles(&["store-player-accounts"], true);
        assert!(!supported(&old));
        let new = profiles(&["store-player-accounts", CLIENT_CAPABILITY], true);
        assert!(supported(&new));
        assert_eq!(player(&new, None).unwrap().id, OWNER);
        let roles = Selection {
            store: OWNER.into(),
            player: PLAYER.into(),
            generation: 1,
        };
        assert_eq!(player(&new, Some(&roles)).unwrap().id, PLAYER);
        assert!(
            !player(
                &profiles(&[CLIENT_CAPABILITY, "store-player-accounts"], false),
                Some(&roles)
            )
            .unwrap()
            .signed_in
        );
    }

    #[test]
    fn commands_name_the_player_and_never_inherit_the_store() {
        let run = run_command("/c/xodus-cli", PLAYER, Path::new("/games/halo"));
        let args: Vec<_> = run
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--profile",
                PLAYER,
                "presence",
                "run",
                "--json",
                "--game-dir",
                "/games/halo"
            ]
        );
        let removed: Vec<_> = run
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert!(removed.contains(&"XODUS_ACCOUNT_PROFILE".to_string()));
        assert!(removed.contains(&"XODUS_PLAYER_PROFILE".to_string()));
        let get = get_command("/c/xodus-cli", PLAYER);
        let args: Vec<_> = get
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--profile", PLAYER, "presence", "get", "--json"]);
    }

    #[test]
    fn the_server_answer_becomes_the_badge() {
        let status = parse_status(
            br#"{"schema":1,"state":"Online","devices":[{"type":"XboxSeries","titles":[{"id":"750323071","name":"Home"},{"id":"1","name":"Halo"}]},{"type":"PC","titles":[]}]}"#,
        )
        .unwrap();
        assert_eq!(status.state, State::Online);
        assert_eq!(status.titles.len(), 2);
        assert_eq!(status.titles[1].name, "Halo");
        assert!(parse_status(br#"{"schema":1,"state":"Busy","devices":[]}"#).is_err());
        assert!(parse_status(br#"{"schema":2,"state":"Online"}"#).is_err());
        assert!(parse_status(b"").is_err());
        let away = parse_status(br#"{"schema":1,"state":"Away"}"#).unwrap();
        let offline = parse_status(br#"{"schema":1,"state":"Offline","devices":[]}"#).unwrap();

        let ready = Availability::Ready;
        let known = |s: &Status| Server::Known(s.clone());
        let online = badge(ready, false, None, &known(&status));
        assert_eq!(online.label().as_deref(), Some("Online"));
        assert_eq!(online.css_class(), Some("online"));
        assert!(online.tooltip().unwrap().contains("Halo on XboxSeries"));
        assert_eq!(badge(ready, true, None, &known(&away)), Badge::Away);
        assert_eq!(badge(ready, true, None, &known(&offline)), Badge::Offline);
        assert_eq!(badge(ready, false, None, &known(&offline)), Badge::Off);
        assert_eq!(badge(ready, true, None, &Server::Unknown), Badge::Checking);
        assert_eq!(badge(ready, false, None, &Server::Unknown), Badge::Hidden);
        assert_eq!(badge(ready, true, None, &Server::Failed), Badge::Unknown);
        assert_eq!(badge(ready, false, None, &Server::Failed), Badge::Hidden);
        for availability in [
            Availability::Checking,
            Availability::Unsupported,
            Availability::SignedOut,
        ] {
            assert_eq!(
                badge(availability, true, None, &known(&status)),
                Badge::Hidden
            );
        }
        // The runner speaks for its game, ahead of a server answer that may
        // be minutes old.
        let playing = Runner::Online("Halo".into());
        let badge_now = badge(ready, true, Some(&playing), &known(&offline));
        assert_eq!(badge_now.label().as_deref(), Some("Playing Halo"));
        assert_eq!(badge_now.css_class(), Some("ingame"));
        let mut retrying = Runner::Idle;
        retrying.apply(&Event::Retrying {
            reason: "unavailable".into(),
            in_secs: Some(30),
        });
        let badge_now = badge(ready, true, Some(&retrying), &known(&status));
        assert_eq!(badge_now.label().as_deref(), Some("Reconnecting"));
        assert!(badge_now.tooltip().unwrap().contains("Xbox is unavailable"));
        assert_eq!(
            describe_retry("server error 500"),
            "Xbox answered: server error 500"
        );
        let failed = Runner::Failed(ErrorCode::NoTitleIdentity);
        let badge_now = badge(ready, true, Some(&failed), &known(&status));
        assert_eq!(badge_now.css_class(), Some("failed"));
        assert!(badge_now
            .tooltip()
            .unwrap()
            .contains("Online status unavailable for this game"));
        assert_eq!(
            badge(ready, true, Some(&Runner::Starting), &known(&away)),
            Badge::Away
        );
    }

    #[test]
    fn runner_state_follows_its_events_and_a_fatal_error_stays() {
        let mut runner = Runner::default();
        assert!(!runner.live());
        runner.apply(&Event::Starting);
        assert!(runner.live());
        runner.apply(&Event::Online {
            title: "Halo".into(),
            heartbeat_after: Some(60),
        });
        assert_eq!(runner, Runner::Online("Halo".into()));
        runner.apply(&Event::Error {
            fatal: false,
            code: ErrorCode::Network,
        });
        assert_eq!(runner, Runner::Retrying("Cannot reach Xbox".into()));
        runner.apply(&Event::Offline);
        assert_eq!(runner, Runner::Idle);
        runner.apply(&Event::Error {
            fatal: true,
            code: ErrorCode::Forbidden,
        });
        runner.apply(&Event::Ended);
        assert_eq!(runner, Runner::Failed(ErrorCode::Forbidden));
        assert!(!runner.live());
        runner.apply(&Event::Starting);
        runner.apply(&Event::Busy);
        assert_eq!(runner, Runner::Idle);

        let all = [
            Runner::Idle,
            Runner::Failed(ErrorCode::Network),
            Runner::Online("Halo".into()),
            Runner::Retrying(String::new()),
        ];
        assert_eq!(strongest(&all), Some(&Runner::Online("Halo".into())));
        assert_eq!(strongest(&[Runner::Idle]), None);
    }

    #[test]
    fn the_caption_says_why_the_switch_cannot_be_used() {
        assert_eq!(
            caption(Availability::Unsupported, true, None),
            "Update the Xodus client to share your status"
        );
        assert_eq!(
            caption(Availability::SignedOut, true, None),
            "Sign in the Player account to share your status"
        );
        assert_eq!(
            caption(Availability::Ready, false, None),
            "Friends see your status from other devices only"
        );
        assert_eq!(
            caption(Availability::Ready, true, None),
            "Friends will see the games you play from Ferestre"
        );
        assert_eq!(
            caption(
                Availability::Ready,
                true,
                Some(&Runner::Online("Halo".into()))
            ),
            "Playing Halo"
        );
    }
}

//! The command line and environment a title is launched with.
//!
//! `scripts/launch-gdk.sh` is the specification this reproduces, and most of it
//! is undoing environment that Steam injects. That list is not derivable from
//! first principles -- each name on it cost a failed launch to find -- so it is
//! copied here verbatim rather than reasoned out again.
//!
//! Nothing here runs anything. `plan()` is a pure function from a recipe and a
//! set of resolved directories to a [`Plan`], which is a description of a
//! process: what to exec, with what arguments, and exactly which environment
//! variables to set and to remove. Keeping it pure is what lets the whole
//! launch surface be tested without an account, a GPU, or a 2 GB download; the
//! binary does the exec, the `mkdir`, the log tee, and the one side effect that
//! is deliberately not modelled here (see below).
//!
//! ## What the caller still has to do
//!
//! A [`Plan`] is not the whole of `launch-gdk.sh`. Before running one, a caller
//! must also:
//!
//!   * start `xodus-service` if it is not already up, removing a stale
//!     `$XDG_RUNTIME_DIR/xodus.sock` first -- the GDK runtime's Xbox-side calls
//!     (sign-in, tokens) go through it and a title will sit on its sign-in
//!     screen without it;
//!   * create the prefix directory and the log's parent directory;
//!   * run any `[[setup]]` actions the recipe asks for, such as
//!     `substitute-xcurl` after a download.
//!
//! These are actions, not environment, so they are the binary's business. They
//! are listed here because the shell script did them in the same 75 lines and
//! nothing else would remind you.
//!
//! ## Why the process shape is what it is
//!
//! The program is always `xodus-cli run`, never the title's executable. The
//! executable is encrypted on disk and only exists decrypted inside a memfd
//! that the client passes to Wine as an inherited file descriptor, so it has to
//! stay a child of the process that opened it. That also rules out running any
//! of this through umu-launcher or pressure-vessel: inherited fds do not
//! survive the container.

use crate::recipe::Recipe;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The environment Steam injects into a shortcut, which has to go before we
/// start a Proton instance of our own.
///
/// In the order `scripts/launch-gdk.sh` unsets them, so the two can be diffed.
/// Each group is here for a different reason:
///
///   * `LD_PRELOAD` / `LD_LIBRARY_PATH` -- the Steam overlay is injected this
///     way and the library path points into the Steam runtime, not ours.
///   * `STEAM_COMPAT_*` / `Steam*` -- these describe *Steam's* idea of the app,
///     including a prefix belonging to a different appid.
///   * `WINE*` -- an inherited prefix or loader would fight the one Proton sets
///     up from `STEAM_COMPAT_DATA_PATH`.
///   * the Vulkan layer variables -- Steam attaches its overlay and Fossilize
///     pipeline capture through the environment. Fossilize records pipelines
///     for an appid that does not really own this process, and its capture
///     layer sits in the same vkd3d-proton path the game renders through: the
///     log fills with "pipeline handle is not registered" and the run dies.
///
/// Two of these, `STEAM_COMPAT_DATA_PATH` and `STEAM_COMPAT_CLIENT_INSTALL_PATH`,
/// are set again with our own values. The shell script unsets then re-exports
/// them; a [`Plan`] states the end result, so they appear in
/// [`Plan::env_set`] and not in [`Plan::env_unset`].
pub const STEAM_ENVIRONMENT: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "STEAM_COMPAT_DATA_PATH",
    "STEAM_COMPAT_CLIENT_INSTALL_PATH",
    "STEAM_COMPAT_TRANSCODED_MEDIA_PATH",
    "STEAM_COMPAT_MEDIA_PATH",
    "SteamAppId",
    "SteamGameId",
    "SteamAppUser",
    "SteamClientLaunch",
    "SteamEnv",
    "WINEPREFIX",
    "WINEDLLPATH",
    "WINELOADER",
    "WINESERVER",
    "ENABLE_VK_LAYER_VALVE_steam_fossilize_1",
    "ENABLE_VK_LAYER_VALVE_steam_overlay_1",
    "VK_LAYER_PATH",
    "VK_INSTANCE_LAYERS",
    "VK_LOADER_LAYERS_ENABLE",
    "STEAM_COMPAT_SHADER_PATH",
    "STEAM_FOSSILIZE_DUMP_PATH",
];

/// The resolved locations a launch needs, as data.
///
/// Resolving these from `XODUS_*` with `$HOME`-relative defaults is `paths.rs`'s
/// job -- it is the part that touches the environment and the filesystem. This
/// module takes the answers so that building a plan stays a pure function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// `XODUS_GAMES_DIR`: decrypted games, prefixes and logs.
    pub games_dir: PathBuf,
    /// `XODUS_CLI_DIR`: the directory holding `xodus-cli` and `xodus-service`.
    pub cli_dir: PathBuf,
    /// `XODUS_PROTON_DIR`: the installed patched Proton compat tool.
    pub proton_dir: PathBuf,
    /// `XODUS_STEAM_DIR`: a Steam install, only for
    /// `STEAM_COMPAT_CLIENT_INSTALL_PATH`. The games are not Steam games.
    pub steam_dir: PathBuf,
    /// Where the shell scripts live -- `scripts/` in a checkout,
    /// `usr/lib/xgdk/scripts` in a package. `proton-wine-shim.sh` is taken from
    /// here.
    pub scripts_dir: PathBuf,
}

impl Dirs {
    /// The client binary. The only thing that can start an encrypted title.
    pub fn xodus_cli(&self) -> PathBuf {
        self.cli_dir.join("xodus-cli")
    }

    /// The background service the GDK runtime's Xbox-side calls go through.
    /// Not part of a plan -- the caller starts it -- but it is found here.
    pub fn xodus_service(&self) -> PathBuf {
        self.cli_dir.join("xodus-service")
    }

    /// The wine stand-in handed to `xodus-cli run`.
    ///
    /// The client invokes its wine argument as `<wine> <exe>`; the shim turns
    /// that into `proton run <exe>` and sets up the compat environment Proton
    /// wants. It has to be a script rather than `proton` itself because of that
    /// argument shape, and it must `exec` so the inherited descriptors survive.
    pub fn shim(&self) -> PathBuf {
        self.scripts_dir.join("proton-wine-shim.sh")
    }
}

/// Everything about a launch that is the user's choice rather than the title's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Keep the environment Steam started us with.
    ///
    /// Off by default, which is what `launch-gdk.sh` does and what an isolated
    /// launch needs. On is for a non-Steam shortcut, where the whole point is
    /// that the overlay, the controller configuration and Remote Play keep
    /// working -- all of which are driven by exactly the variables the default
    /// strips. This is a per-title toggle rather than a policy because both
    /// answers are legitimate and the launcher is not in a position to guess.
    ///
    /// Turning it on brings back Steam's Fossilize capture, which has been seen
    /// to kill a run. If a Steam-integrated launch dies with "pipeline handle
    /// is not registered", that is this.
    pub steam_integration: bool,

    /// `XGR_XUID`: the 16-hex-digit prefix the Windows install's save folders
    /// are named with.
    ///
    /// Existing folders are matched by their SCID suffix whatever prefix they
    /// carry, so this changes nothing about reading saves. It only names *new*
    /// folders the same way, so they can be copied back to Windows as-is.
    pub xuid: Option<String>,

    /// Overrides the install directory, from the recipe's `install.dir-env`
    /// (`BEDROCK_DIR`, `EXP33_DIR`, `FH5_DIR`). Reading it is the caller's job;
    /// this stays pure.
    pub game_dir: Option<PathBuf>,

    /// Overrides the prefix, from the recipe's `install.prefix-env`.
    pub prefix_dir: Option<PathBuf>,
}

/// A process to start: everything, and nothing more.
///
/// `env_set` and `env_unset` are disjoint by construction, so a caller can
/// apply them in either order without changing the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env_set: BTreeMap<String, String>,
    pub env_unset: Vec<String>,
    /// The directory to start in. `None` inherits the caller's.
    pub cwd: Option<PathBuf>,
}

impl Plan {
    /// The value a variable will have, for tests and for an `xgdk env`-style
    /// command that shows a launch before running it.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.env_set.get(key).map(String::as_str)
    }

    /// Whether the variable is removed from the inherited environment.
    pub fn is_unset(&self, key: &str) -> bool {
        self.env_unset.iter().any(|k| k == key)
    }

    /// A ready-to-spawn `Command`. Still does not run anything: spawning it is
    /// the caller's decision, and the caller is also the one that has to stay
    /// the parent of the process, because the decrypted image reaches Wine as
    /// an inherited file descriptor.
    pub fn to_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        for (key, value) in &self.env_set {
            command.env(key, value);
        }
        for key in &self.env_unset {
            command.env_remove(key);
        }
        if let Some(dir) = &self.cwd {
            command.current_dir(dir);
        }
        command
    }
}

/// Where a title's package is installed.
///
/// `titles/SCHEMA.md`: the environment override if the caller found one, else
/// the games directory joined with `install.dir`, else with the slug.
pub fn game_dir(recipe: &Recipe, dirs: &Dirs, options: &Options) -> PathBuf {
    options
        .game_dir
        .clone()
        .unwrap_or_else(|| dirs.games_dir.join(recipe.install_dir()))
}

/// Where a title's Wine prefix lives. Same rule, defaulting to `<slug>-proton`.
///
/// One prefix per title on purpose: a shared prefix means one title's DLL
/// overrides and registry are another's problem.
pub fn prefix_dir(recipe: &Recipe, dirs: &Dirs, options: &Options) -> PathBuf {
    options
        .prefix_dir
        .clone()
        .unwrap_or_else(|| dirs.games_dir.join(recipe.prefix_dir()))
}

/// The log `launch-gdk.sh` tees to, `<games dir>/<slug>-launch.log`.
///
/// Named after the slug rather than the install directory so that a relocated
/// install still logs in a predictable place.
pub fn log_path(recipe: &Recipe, dirs: &Dirs) -> PathBuf {
    dirs.games_dir
        .join(format!("{}-launch.log", recipe.title.slug))
}

/// Build the plan for launching a title.
pub fn plan(recipe: &Recipe, dirs: &Dirs, options: &Options) -> Plan {
    let game_dir = game_dir(recipe, dirs, options);
    let prefix_dir = prefix_dir(recipe, dirs, options);

    let mut env_set: BTreeMap<String, String> = BTreeMap::new();

    if !options.steam_integration {
        // Belt and braces with the unsets above them: Steam's layers can also
        // be enabled by an implicit layer manifest on the system, which no
        // amount of unsetting reaches. DISABLE_* wins over both.
        env_set.insert("DISABLE_VK_LAYER_VALVE_steam_overlay_1".into(), "1".into());
        env_set.insert(
            "DISABLE_VK_LAYER_VALVE_steam_fossilize_1".into(),
            "1".into(),
        );
    }

    // gameinput.dll is behind an allow-list, and a GDK title that cannot load
    // the GameInput runtime says so and stops: "Critical Failure: GameInput
    // Runtime could not be loaded".
    env_set.insert("WINE_GAMEINPUT".into(), "1".into());

    // Proton derives the prefix from STEAM_COMPAT_DATA_PATH (pfx/ inside it),
    // so this is how a title gets its own prefix. Set in both modes: under
    // Steam integration the inherited value points at the shortcut's compatdata
    // directory, which is not where this title is installed.
    env_set.insert("STEAM_COMPAT_DATA_PATH".into(), string(&prefix_dir));
    env_set.insert(
        "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
        string(&dirs.steam_dir),
    );
    env_set.insert("PROTON_DIR".into(), string(&dirs.proton_dir));

    // The shim is a shell script that sources xodus-env.sh again, so it would
    // otherwise re-resolve these from scratch and could land somewhere else
    // than the launcher did -- $HOME/xbox-games by default, whatever the user
    // configured. Hand down what we resolved so the two cannot disagree.
    env_set.insert("XODUS_GAMES_DIR".into(), string(&dirs.games_dir));
    env_set.insert("XODUS_CLI_DIR".into(), string(&dirs.cli_dir));
    env_set.insert("XODUS_PROTON_DIR".into(), string(&dirs.proton_dir));
    env_set.insert("XODUS_STEAM_DIR".into(), string(&dirs.steam_dir));

    if let Some(xuid) = &options.xuid {
        env_set.insert("XGR_XUID".into(), xuid.clone());
    }

    // Last, so a recipe can override anything above it. That is deliberate: the
    // defaults are what every title needs, and a title that needs something
    // different is exactly what a recipe is for.
    for (key, value) in &recipe.launch.env {
        env_set.insert(key.clone(), value.clone());
    }

    let env_unset = if options.steam_integration {
        Vec::new()
    } else {
        STEAM_ENVIRONMENT
            .iter()
            .filter(|name| !env_set.contains_key(**name))
            .map(|name| name.to_string())
            .collect()
    };

    Plan {
        program: dirs.xodus_cli(),
        // `xodus-cli run -e <exe in package> <game dir> <wine>`. The executable
        // is a backslash path relative to the package, as Windows writes it,
        // and is passed through untouched.
        args: vec![
            "run".into(),
            "-e".into(),
            recipe.launch.executable.clone(),
            string(&game_dir),
            string(&dirs.shim()),
        ],
        env_set,
        env_unset,
        // launch-gdk.sh inherits whatever directory it was started from, which
        // means a title that opens a file relative to the working directory
        // behaves differently depending on where you launched it. Pin it to the
        // install directory, which is what Steam does for its own games.
        cwd: Some(game_dir),
    }
}

/// Paths reach the child as arguments and environment values, both of which are
/// `String` here. A non-UTF-8 games directory would be mangled rather than
/// refused; that is a trade for a `Plan` that is comparable and printable, and
/// every default path is derived from `$HOME`.
fn string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Dirs {
        Dirs {
            games_dir: PathBuf::from("/games"),
            cli_dir: PathBuf::from("/opt/xodus"),
            proton_dir: PathBuf::from("/compat/xodus"),
            steam_dir: PathBuf::from("/steam"),
            scripts_dir: PathBuf::from("/usr/lib/xgdk/scripts"),
        }
    }

    fn recipe(extra: &str) -> Recipe {
        let text = format!(
            r#"
schema = 1
[title]
product-id = "9PPT8K6GQHRZ"
name = "Example"
slug = "example"
[launch]
executable = 'Sandfall\Binaries\WinGDK\SandFall-WinGDK-Shipping.exe'
[status]
state = "playable"
summary = "Runs."
{extra}
"#
        );
        Recipe::parse(&text).expect("fixture should parse")
    }

    #[test]
    fn the_program_is_the_client_and_never_the_title() {
        // The executable is encrypted on disk; only `xodus-cli run` can start
        // it. If this ever becomes the game's own path, nothing launches.
        let plan = plan(&recipe(""), &dirs(), &Options::default());
        assert_eq!(plan.program, PathBuf::from("/opt/xodus/xodus-cli"));
        assert_eq!(
            plan.args,
            vec![
                "run",
                "-e",
                r"Sandfall\Binaries\WinGDK\SandFall-WinGDK-Shipping.exe",
                "/games/example",
                "/usr/lib/xgdk/scripts/proton-wine-shim.sh",
            ]
        );
    }

    #[test]
    fn the_default_plan_sets_what_every_title_needs() {
        let plan = plan(&recipe(""), &dirs(), &Options::default());
        assert_eq!(plan.get("WINE_GAMEINPUT"), Some("1"));
        assert_eq!(
            plan.get("STEAM_COMPAT_DATA_PATH"),
            Some("/games/example-proton")
        );
        assert_eq!(plan.get("STEAM_COMPAT_CLIENT_INSTALL_PATH"), Some("/steam"));
        assert_eq!(plan.get("PROTON_DIR"), Some("/compat/xodus"));
        assert_eq!(
            plan.get("DISABLE_VK_LAYER_VALVE_steam_overlay_1"),
            Some("1")
        );
        assert_eq!(
            plan.get("DISABLE_VK_LAYER_VALVE_steam_fossilize_1"),
            Some("1")
        );
    }

    #[test]
    fn the_default_plan_strips_the_environment_steam_injects() {
        // Every name here cost a failed launch to find. Losing one is a
        // regression that only shows up when someone launches from Steam.
        let plan = plan(&recipe(""), &dirs(), &Options::default());
        for name in [
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "STEAM_COMPAT_MEDIA_PATH",
            "STEAM_COMPAT_TRANSCODED_MEDIA_PATH",
            "SteamAppId",
            "SteamGameId",
            "SteamAppUser",
            "SteamClientLaunch",
            "SteamEnv",
            "WINEPREFIX",
            "WINEDLLPATH",
            "WINELOADER",
            "WINESERVER",
            "ENABLE_VK_LAYER_VALVE_steam_overlay_1",
            "ENABLE_VK_LAYER_VALVE_steam_fossilize_1",
            "VK_LAYER_PATH",
            "VK_INSTANCE_LAYERS",
            "VK_LOADER_LAYERS_ENABLE",
            "STEAM_COMPAT_SHADER_PATH",
            "STEAM_FOSSILIZE_DUMP_PATH",
        ] {
            assert!(plan.is_unset(name), "{name} should be unset");
        }
    }

    #[test]
    fn setting_and_unsetting_never_overlap() {
        // launch-gdk.sh unsets STEAM_COMPAT_DATA_PATH and re-exports it. A plan
        // states the end result, so it must not claim both, or applying it in
        // the wrong order would drop the prefix and Proton would invent one.
        let plan = plan(&recipe(""), &dirs(), &Options::default());
        for name in ["STEAM_COMPAT_DATA_PATH", "STEAM_COMPAT_CLIENT_INSTALL_PATH"] {
            assert!(plan.get(name).is_some(), "{name} should be set");
            assert!(!plan.is_unset(name), "{name} should not also be unset");
        }
        for name in &plan.env_unset {
            assert!(
                !plan.env_set.contains_key(name),
                "{name} is both set and unset"
            );
        }
    }

    #[test]
    fn steam_integration_leaves_steams_environment_alone() {
        // A non-Steam shortcut wants the overlay, the controller configuration
        // and Remote Play, and all of those ride on the variables the default
        // strips.
        let options = Options {
            steam_integration: true,
            ..Options::default()
        };
        let plan = plan(&recipe(""), &dirs(), &options);

        assert!(
            plan.env_unset.is_empty(),
            "nothing should be stripped: {:?}",
            plan.env_unset
        );
        assert_eq!(plan.get("DISABLE_VK_LAYER_VALVE_steam_overlay_1"), None);
        assert_eq!(plan.get("DISABLE_VK_LAYER_VALVE_steam_fossilize_1"), None);

        // But the prefix is still ours: Steam's value points at the shortcut's
        // compatdata directory, which is not where the title is installed.
        assert_eq!(
            plan.get("STEAM_COMPAT_DATA_PATH"),
            Some("/games/example-proton")
        );
        assert_eq!(plan.get("WINE_GAMEINPUT"), Some("1"));
        assert_eq!(plan.get("PROTON_DIR"), Some("/compat/xodus"));
    }

    #[test]
    fn a_recipes_own_environment_is_merged_and_wins() {
        let r = recipe("[launch.env]\nPROTON_NO_MEDIACONV = \"1\"\nWINE_GAMEINPUT = \"0\"\n");
        let plan = plan(&r, &dirs(), &Options::default());
        assert_eq!(plan.get("PROTON_NO_MEDIACONV"), Some("1"));
        // A title that must not have GameInput has to be able to say so; the
        // defaults are a floor, not a policy.
        assert_eq!(plan.get("WINE_GAMEINPUT"), Some("0"));
    }

    #[test]
    fn a_recipe_can_keep_a_variable_the_default_would_strip() {
        // Overriding a stripped name has to remove it from the unset list too,
        // or the value would be set and then deleted.
        let r = recipe("[launch.env]\nLD_PRELOAD = \"/opt/thing.so\"\n");
        let plan = plan(&r, &dirs(), &Options::default());
        assert_eq!(plan.get("LD_PRELOAD"), Some("/opt/thing.so"));
        assert!(!plan.is_unset("LD_PRELOAD"));
    }

    #[test]
    fn the_shim_is_told_where_the_launcher_resolved_things() {
        // proton-wine-shim.sh sources xodus-env.sh again. Without these it
        // would fall back to ~/xbox-games and put the shader cache and the
        // fallback prefix somewhere the launcher never looked.
        let plan = plan(&recipe(""), &dirs(), &Options::default());
        assert_eq!(plan.get("XODUS_GAMES_DIR"), Some("/games"));
        assert_eq!(plan.get("XODUS_CLI_DIR"), Some("/opt/xodus"));
        assert_eq!(plan.get("XODUS_PROTON_DIR"), Some("/compat/xodus"));
        assert_eq!(plan.get("XODUS_STEAM_DIR"), Some("/steam"));
    }

    #[test]
    fn the_xuid_is_passed_only_when_the_user_set_one() {
        let without = plan(&recipe(""), &dirs(), &Options::default());
        assert_eq!(without.get("XGR_XUID"), None);

        let options = Options {
            xuid: Some("0009abcd12345678".into()),
            ..Options::default()
        };
        let with = plan(&recipe(""), &dirs(), &options);
        assert_eq!(with.get("XGR_XUID"), Some("0009abcd12345678"));
    }

    #[test]
    fn the_game_dir_and_prefix_follow_the_documented_layout() {
        let d = dirs();

        // Default: <games>/<slug> and <games>/<slug>-proton.
        let r = recipe("");
        assert_eq!(
            game_dir(&r, &d, &Options::default()),
            PathBuf::from("/games/example")
        );
        assert_eq!(
            prefix_dir(&r, &d, &Options::default()),
            PathBuf::from("/games/example-proton")
        );
        assert_eq!(log_path(&r, &d), PathBuf::from("/games/example-launch.log"));

        // A recipe may move both, which is how Bedrock keeps its package one
        // level down.
        let r = recipe("[install]\ndir = \"bedrock/game\"\nprefix = \"bedrock-proton\"\n");
        assert_eq!(
            game_dir(&r, &d, &Options::default()),
            PathBuf::from("/games/bedrock/game")
        );
        assert_eq!(
            prefix_dir(&r, &d, &Options::default()),
            PathBuf::from("/games/bedrock-proton")
        );
    }

    #[test]
    fn an_environment_override_beats_the_layout() {
        // BEDROCK_DIR and friends, which existing installs already use. The
        // caller reads them; a plan just has to honour them.
        let options = Options {
            game_dir: Some(PathBuf::from("/mnt/big/exp33")),
            prefix_dir: Some(PathBuf::from("/mnt/big/exp33-pfx")),
            ..Options::default()
        };
        let plan = plan(&recipe(""), &dirs(), &options);
        assert_eq!(plan.args[3], "/mnt/big/exp33");
        assert_eq!(
            plan.get("STEAM_COMPAT_DATA_PATH"),
            Some("/mnt/big/exp33-pfx")
        );
        assert_eq!(plan.cwd, Some(PathBuf::from("/mnt/big/exp33")));
    }

    #[test]
    fn the_command_carries_the_whole_plan() {
        let plan = plan(&recipe(""), &dirs(), &Options::default());
        let command = plan.to_command();
        assert_eq!(
            command.get_program(),
            std::ffi::OsStr::new("/opt/xodus/xodus-cli")
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/games/example")));

        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        assert!(removed.contains(&"LD_PRELOAD".to_string()));
    }

    #[test]
    fn every_recipe_in_the_repository_produces_a_launchable_plan() {
        // The recipes are the contract. This catches a title whose executable
        // or layout stops surviving the trip into a command line.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles");
        if !dir.exists() {
            return; // packaged crate without the repository around it
        }
        let d = dirs();
        for r in Recipe::load_dir(&dir).expect("repository recipes should parse") {
            let plan = plan(&r, &d, &Options::default());
            assert_eq!(plan.args[0], "run");
            assert_eq!(plan.args[2], r.launch.executable);
            assert!(
                plan.args[3].starts_with("/games/"),
                "{} installs outside the games directory: {}",
                r.title.slug,
                plan.args[3]
            );
            assert_eq!(plan.get("WINE_GAMEINPUT"), Some("1"));
        }
    }
}

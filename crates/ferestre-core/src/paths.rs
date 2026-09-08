//! Where everything lives.
//!
//! A checkout, an AppImage and a packaged install put the same things in
//! different places, and someone who already has a working install has their
//! games, prefixes, client and runtime wherever `scripts/xodus-env.sh` put
//! them. So this resolves exactly the variables that script honours, with the
//! same defaults and the same probe order: switching from the shell scripts to
//! the launcher must not require moving a single directory.
//!
//! Resolution is a pure function of an [`Env`] -- a set of variables, the path
//! of the running binary, and a predicate that says whether a candidate exists.
//! [`Paths::from_env`] fills those from the process and the disk; a test builds
//! one by hand, so it does not matter whether the machine running the test has
//! Steam, a client or a games directory.

use crate::recipe::Recipe;
use anyhow::{anyhow, bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

/// Default games directory, relative to `$HOME`. Same as xodus-env.sh.
const DEFAULT_GAMES: &str = "xbox-games";
/// Default Proton build tree, relative to `$HOME`.
const DEFAULT_BUILD: &str = "src/xodus-build";
/// The runtime installs as a Steam compatibility tool under this name.
const COMPAT_TOOL: &str = "compatibilitytools.d/xodus";
/// Our directory inside the XDG state and cache homes.
const APP: &str = "ferestre";
/// What marks a directory as the installed tree: the AppImage, the AUR package
/// and a checkout all carry `scripts/`, and `install-xodus-proton.sh` reads
/// `patches/` out of the same place. Probing for it stops us pointing at some
/// unrelated ancestor of wherever the binary happens to sit.
const TREE_MARKER: &str = "scripts";

/// Steam installs, in the order xodus-env.sh probes them.
fn steam_candidates(home: &Path) -> [PathBuf; 4] {
    [
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        home.join(".steam/root"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
}

/// The environment path resolution reads.
///
/// The existence predicate is part of it because half of what xodus-env.sh
/// does is "the first of these directories that exists". Keeping it here
/// rather than calling `Path::exists` inline is what makes the resolution
/// testable without a fixture tree on disk.
pub struct Env {
    vars: BTreeMap<String, String>,
    exe: Option<PathBuf>,
    exists: Box<dyn Fn(&Path) -> bool>,
}

impl std::fmt::Debug for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env")
            .field("vars", &self.vars.len())
            .field("exe", &self.exe)
            .finish_non_exhaustive()
    }
}

impl Env {
    /// The real process environment, probing the real filesystem.
    pub fn from_process() -> Self {
        Env {
            vars: std::env::vars().collect(),
            exe: std::env::current_exe().ok(),
            exists: Box::new(|p| p.exists()),
        }
    }

    /// An explicit environment. Nothing exists unless [`Env::with_existing`]
    /// says so, so a result never depends on what the host happens to have
    /// installed.
    pub fn from_map<I, K, V>(vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Env {
            vars: vars
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            exe: None,
            exists: Box::new(|_| false),
        }
    }

    /// Declare the paths that exist. Replaces any previous set.
    pub fn with_existing<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let set: BTreeSet<PathBuf> = paths.into_iter().map(Into::into).collect();
        self.exists = Box::new(move |p| set.contains(p));
        self
    }

    /// Where the running binary is, used to find the tree it was installed
    /// with.
    pub fn with_exe(mut self, exe: impl Into<PathBuf>) -> Self {
        self.exe = Some(exe.into());
        self
    }

    /// A variable, with empty treated as unset -- `${VAR:-default}` in the
    /// shell does the same, and `XODUS_CLI_DIR=` is how xodus-env.sh itself
    /// records "looked, found nothing".
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.var(name).map(PathBuf::from)
    }

    fn exists(&self, p: &Path) -> bool {
        (self.exists)(p)
    }

    fn first_existing<I: IntoIterator<Item = PathBuf>>(&self, candidates: I) -> Option<PathBuf> {
        candidates.into_iter().find(|c| self.exists(c))
    }

    /// The directory of `name` on `PATH`, the way `command -v` finds it. An
    /// empty PATH element means the working directory, which is not somewhere
    /// we want to run a client from, so it is skipped.
    fn on_path(&self, name: &str) -> Option<PathBuf> {
        let path = self.var("PATH")?;
        path.split(':')
            .filter(|d| !d.is_empty())
            .map(Path::new)
            .find(|d| self.exists(&d.join(name)))
            .map(Path::to_path_buf)
    }
}

/// Every path the launcher needs, resolved once.
///
/// The variables are kept because a recipe names its own overrides -- Bedrock's
/// `dir-env = "BEDROCK_DIR"` -- so resolving a title's directories needs the
/// environment, not just the defaults derived from it.
#[derive(Debug, Clone)]
pub struct Paths {
    home: PathBuf,
    games: PathBuf,
    build: PathBuf,
    repo: Option<PathBuf>,
    scripts: Option<PathBuf>,
    titles: Option<PathBuf>,
    steam: Option<PathBuf>,
    runtime: Option<PathBuf>,
    cli_dir: Option<PathBuf>,
    state: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    vars: BTreeMap<String, String>,
}

impl Paths {
    /// Resolve from the process environment and the real filesystem.
    pub fn from_env() -> Result<Self> {
        Self::resolve(&Env::from_process())
    }

    /// Resolve from an explicit environment.
    pub fn resolve(env: &Env) -> Result<Self> {
        let home = env.path("HOME").ok_or_else(|| {
            anyhow!(
                "HOME is not set and every default path is relative to it; \
                 set HOME, or set XODUS_GAMES_DIR, XODUS_CLI_DIR and XODUS_PROTON_DIR explicitly"
            )
        })?;

        let games = env
            .path("XODUS_GAMES_DIR")
            .unwrap_or_else(|| home.join(DEFAULT_GAMES));
        let build = env
            .path("XODUS_BUILD_DIR")
            .unwrap_or_else(|| home.join(DEFAULT_BUILD));

        let repo = resolve_tree(env);
        let scripts = env
            .path("FERESTRE_SCRIPTS_DIR")
            .or_else(|| repo.as_ref().map(|r| r.join("scripts")));
        // The AUR package does not ship titles/ yet, so an installed tree can
        // have scripts and no recipes. Report that honestly rather than handing
        // out a path that is not there.
        let titles = env.path("FERESTRE_TITLES").or_else(|| {
            repo.as_ref()
                .map(|r| r.join("titles"))
                .filter(|t| env.exists(t))
        });

        let steam = env
            .path("XODUS_STEAM_DIR")
            .or_else(|| env.first_existing(steam_candidates(&home)));

        // The resolved Steam directory comes first, which is how a Flatpak
        // Steam gets found: it is the fourth Steam candidate but not one of the
        // three compat-tool candidates xodus-env.sh lists.
        let runtime = env
            .path("XODUS_PROTON_DIR")
            // Release archives keep the runtime beside the packaged scripts.
            // Prefer it over a Steam compatibility-tool install: the archive
            // is a coherent client/runtime pair and needs no post-install step.
            .or_else(|| {
                repo.as_ref()
                    .map(|r| r.join("runtime"))
                    .filter(|r| env.exists(r))
            })
            .or_else(|| {
                let mut candidates: Vec<PathBuf> =
                    steam.iter().map(|s| s.join(COMPAT_TOOL)).collect();
                candidates.extend([
                    home.join(".steam/steam").join(COMPAT_TOOL),
                    home.join(".local/share/Steam").join(COMPAT_TOOL),
                    home.join(".steam/root").join(COMPAT_TOOL),
                ]);
                env.first_existing(candidates)
            });

        let cli_dir = env
            .path("XODUS_CLI_DIR")
            // The AppImage exports this explicitly. The archive cannot, so
            // discover its client from the packaged tree as well.
            .or_else(|| {
                repo.as_ref()
                    .map(|r| r.join("client"))
                    .filter(|r| env.exists(r))
            })
            .or_else(|| env.on_path("xodus-cli"))
            .or_else(|| {
                let mut candidates = vec![home.join("src/xodus-cli/target/release")];
                // A checkout beside this one, which is where it is when both
                // are being worked on at once.
                if let Some(beside) = repo.as_ref().and_then(|r| r.parent()) {
                    candidates.push(beside.join("xodus-cli/target/release"));
                }
                env.first_existing(candidates)
            });

        Ok(Paths {
            home,
            games,
            build,
            repo,
            scripts,
            titles,
            steam,
            runtime,
            cli_dir,
            state: xdg(env, "XDG_STATE_HOME", ".local/state").join(APP),
            cache: xdg(env, "XDG_CACHE_HOME", ".cache").join(APP),
            config: xdg(env, "XDG_CONFIG_HOME", ".config").join(APP),
            vars: env.vars.clone(),
        })
    }

    /// A variable as resolution saw it, empty treated as unset. The launcher
    /// needs this for the pass-through variables a recipe or a title cares
    /// about, `XGR_XUID` among them.
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `XODUS_GAMES_DIR`: decrypted games, prefixes and launch logs.
    pub fn games_dir(&self) -> &Path {
        &self.games
    }

    /// `XODUS_BUILD_DIR`: the Proton build tree, for building the runtime.
    pub fn build_dir(&self) -> &Path {
        &self.build
    }

    /// The installed tree: `scripts/`, `patches/` and (usually) `titles/`.
    pub fn tree_dir(&self) -> Option<&Path> {
        self.repo.as_deref()
    }

    pub fn scripts_dir(&self) -> Option<&Path> {
        self.scripts.as_deref()
    }

    /// Where the title recipes are, if this install carries any.
    pub fn titles_dir(&self) -> Option<&Path> {
        self.titles.as_deref()
    }

    /// A Steam install, for `STEAM_COMPAT_CLIENT_INSTALL_PATH`.
    pub fn steam_dir(&self) -> Option<&Path> {
        self.steam.as_deref()
    }

    /// The patched Proton, installed as a Steam compatibility tool.
    pub fn runtime_dir(&self) -> Option<&Path> {
        self.runtime.as_deref()
    }

    /// The capability list a runtime build is expected to publish
    /// (`titles/SCHEMA.md`, "Capabilities, not versions"). No build writes one
    /// yet; the path is fixed here so the runtime and the check agree when one
    /// does.
    pub fn runtime_capabilities_file(&self) -> Option<PathBuf> {
        self.runtime
            .as_ref()
            .map(|r| r.join("ferestre-capabilities.txt"))
    }

    pub fn cli_dir(&self) -> Option<&Path> {
        self.cli_dir.as_deref()
    }

    /// The client binary. It is run as a child process, never linked: it is
    /// GPL-3.0, and the decrypted executable reaches Wine as a file descriptor
    /// that has to stay inherited from whoever opened it.
    pub fn xodus_cli(&self) -> Option<PathBuf> {
        self.cli_dir.as_ref().map(|d| d.join("xodus-cli"))
    }

    /// The service the runtime's Xbox-side calls go through.
    pub fn xodus_service(&self) -> Option<PathBuf> {
        self.cli_dir.as_ref().map(|d| d.join("xodus-service"))
    }

    /// XDG state: things worth keeping across runs.
    pub fn state_dir(&self) -> &Path {
        &self.state
    }

    /// XDG cache: things that can be thrown away and fetched again.
    /// Where the launcher keeps what a person changed, as opposed to what it
    /// shipped with.
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// Recipes someone wrote or edited here, which win over the packaged ones.
    ///
    /// A separate directory rather than edits in place, for three reasons: the
    /// packaged `titles/` is read-only in an AppImage or a system package, an
    /// upgrade must not silently revert someone's fix, and "what did I change"
    /// has to be answerable -- it is the thing they will paste into an issue.
    pub fn user_titles_dir(&self) -> PathBuf {
        self.config.join("titles")
    }

    /// Every directory recipes are read from, least specific first. Loading in
    /// this order and letting later ones win is what makes an edit an override
    /// rather than a fork.
    pub fn title_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.titles.iter().map(PathBuf::from).collect();
        dirs.push(self.user_titles_dir());
        dirs
    }

    /// The market and language the catalog is asked in.
    ///
    /// Only decides which name and art come back; every market has the same
    /// product ids. Lives here rather than in each caller so the CLI and the
    /// window cannot ask for different ones and cache over each other.
    pub fn market(&self) -> (String, String) {
        (
            self.var("FERESTRE_MARKET")
                .unwrap_or(crate::catalog::DEFAULT_MARKET)
                .to_string(),
            self.var("FERESTRE_LANGUAGE")
                .unwrap_or(crate::catalog::DEFAULT_LANGUAGE)
                .to_string(),
        )
    }

    /// Where catalog answers and art are kept.
    pub fn catalog_cache(&self) -> crate::catalog::Cache {
        crate::catalog::Cache::new(self.cache.join("catalog"))
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    /// Where a title's package is installed.
    ///
    /// `install.dir-env` wins if it is set, and is taken as given -- that
    /// variable is how an existing install says "the game is on the other
    /// drive", so it is deliberately not confined to the games directory. The
    /// path from the recipe is, because a recipe is data from a file that may
    /// not be ours.
    pub fn install_dir(&self, recipe: &Recipe) -> Result<PathBuf> {
        if let Some(dir) = self.recipe_env(recipe.install.dir_env.as_deref()) {
            return Ok(dir);
        }
        self.under_games(recipe.install_dir(), "install dir", recipe)
    }

    /// The title's Wine prefix (`STEAM_COMPAT_DATA_PATH`).
    pub fn prefix_dir(&self, recipe: &Recipe) -> Result<PathBuf> {
        if let Some(dir) = self.recipe_env(recipe.install.prefix_env.as_deref()) {
            return Ok(dir);
        }
        self.under_games(&recipe.prefix_dir(), "prefix", recipe)
    }

    /// The launch log, named as the shell scripts name it so an existing log
    /// keeps growing instead of a second one appearing beside it.
    pub fn log_file(&self, recipe: &Recipe) -> Result<PathBuf> {
        let name = format!("{}-launch.log", recipe.title.slug);
        self.under_games(&name, "log file", recipe)
    }

    fn recipe_env(&self, name: Option<&str>) -> Option<PathBuf> {
        self.var(name?).map(PathBuf::from)
    }

    /// Join a recipe-supplied relative path to the games directory, refusing
    /// anything that would land outside it. A recipe is data, and `..` or a
    /// leading `/` in it would otherwise let a downloaded or contributed
    /// recipe write wherever it liked.
    fn under_games(&self, relative: &str, what: &str, recipe: &Recipe) -> Result<PathBuf> {
        let path = Path::new(relative);
        let mut parts = 0;
        for component in path.components() {
            match component {
                Component::Normal(_) => parts += 1,
                // Harmless, and keeps "./thing" from being a hard error.
                Component::CurDir => {}
                _ => bail!(
                    "{}: {what} \"{relative}\" must be a relative path inside the games directory \
                     ({}), with no leading \"/\" and no \"..\"",
                    recipe.title.product_id,
                    self.games.display()
                ),
            }
        }
        if parts == 0 {
            bail!(
                "{}: {what} is empty; it must name a directory inside {}",
                recipe.title.product_id,
                self.games.display()
            );
        }
        Ok(self.games.join(path))
    }
}

/// An XDG base directory. The spec says a relative value is invalid and must be
/// ignored, which matters here because ignoring it lands the launcher in the
/// same place as every other tool rather than somewhere relative to whatever
/// directory it was started in.
fn xdg(env: &Env, var: &str, fallback: &str) -> PathBuf {
    let home = env.path("HOME").unwrap_or_default();
    match env.var(var) {
        Some(v) if Path::new(v).is_absolute() => PathBuf::from(v),
        _ => home.join(fallback),
    }
}

/// Find the installed tree -- scripts, patches and recipes -- however this
/// build was installed.
///
/// xodus-env.sh derives it from `$0`, which a compiled binary has no equivalent
/// of, so the candidates are: what the environment says, where the AppImage
/// mounts, then the binary's own neighbourhood (`/usr/bin/ferestre` next to
/// `/usr/lib/ferestre`, or `target/debug/deps/...` inside a checkout).
fn resolve_tree(env: &Env) -> Option<PathBuf> {
    if let Some(explicit) = env.path("XODUS_REPO_DIR") {
        return Some(explicit);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    // AppRun exports this, and the placeholder command honours it.
    if let Some(scripts) = env.var("FERESTRE_SCRIPTS_DIR") {
        if let Some(parent) = Path::new(scripts).parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    if let Some(appdir) = env.var("APPDIR") {
        candidates.push(Path::new(appdir).join("usr/lib/ferestre"));
    }
    if let Some(exe_dir) = env.exe.as_deref().and_then(Path::parent) {
        if let Some(parent) = exe_dir.parent() {
            candidates.push(parent.join("lib/ferestre"));
        }
        // Four levels covers target/debug/deps/<test binary> in a checkout.
        candidates.extend(exe_dir.ancestors().take(4).map(Path::to_path_buf));
    }
    candidates.push(PathBuf::from("/usr/lib/ferestre"));
    candidates
        .into_iter()
        .find(|c| env.exists(&c.join(TREE_MARKER)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/tester";

    fn env(pairs: &[(&str, &str)]) -> Env {
        let mut vars = vec![("HOME", HOME)];
        vars.extend_from_slice(pairs);
        Env::from_map(vars)
    }

    fn paths(pairs: &[(&str, &str)]) -> Paths {
        Paths::resolve(&env(pairs)).expect("should resolve")
    }

    fn recipe(extra: &str) -> Recipe {
        let text = format!(
            r#"
schema = 1
[title]
product-id = "9NBLGGH2JHXJ"
name = "Example"
slug = "example"
[launch]
executable = 'Example.exe'
[status]
state = "playable"
summary = "Runs."
{extra}
"#
        );
        Recipe::parse(&text).expect("test recipe should parse")
    }

    #[test]
    fn defaults_are_the_ones_the_shell_scripts_use() {
        let p = paths(&[]);
        assert_eq!(p.games_dir(), Path::new("/home/tester/xbox-games"));
        assert_eq!(p.build_dir(), Path::new("/home/tester/src/xodus-build"));
        assert_eq!(
            p.state_dir(),
            Path::new("/home/tester/.local/state/ferestre")
        );
        assert_eq!(p.cache_dir(), Path::new("/home/tester/.cache/ferestre"));
        // Nothing exists in this environment, so nothing may be claimed to.
        assert_eq!(p.steam_dir(), None);
        assert_eq!(p.runtime_dir(), None);
        assert_eq!(p.cli_dir(), None);
        assert_eq!(p.xodus_cli(), None);
    }

    #[test]
    fn no_home_is_an_error_that_says_what_to_set() {
        let err = Paths::resolve(&Env::from_map([("XODUS_GAMES_DIR", "/games")]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("HOME"), "{err}");
        assert!(err.contains("XODUS_GAMES_DIR"), "{err}");
    }

    #[test]
    fn every_variable_overrides_its_default() {
        let p = paths(&[
            ("XODUS_GAMES_DIR", "/mnt/games"),
            ("XODUS_BUILD_DIR", "/mnt/build"),
            ("XODUS_STEAM_DIR", "/mnt/steam"),
            ("XODUS_PROTON_DIR", "/mnt/proton"),
            ("XODUS_CLI_DIR", "/mnt/cli"),
            ("XODUS_REPO_DIR", "/mnt/repo"),
            ("XDG_STATE_HOME", "/mnt/state"),
            ("XDG_CACHE_HOME", "/mnt/cache"),
        ]);
        assert_eq!(p.games_dir(), Path::new("/mnt/games"));
        assert_eq!(p.build_dir(), Path::new("/mnt/build"));
        assert_eq!(p.steam_dir(), Some(Path::new("/mnt/steam")));
        assert_eq!(p.runtime_dir(), Some(Path::new("/mnt/proton")));
        assert_eq!(p.cli_dir(), Some(Path::new("/mnt/cli")));
        assert_eq!(p.xodus_cli().unwrap(), Path::new("/mnt/cli/xodus-cli"));
        assert_eq!(
            p.xodus_service().unwrap(),
            Path::new("/mnt/cli/xodus-service")
        );
        assert_eq!(p.tree_dir(), Some(Path::new("/mnt/repo")));
        assert_eq!(p.scripts_dir(), Some(Path::new("/mnt/repo/scripts")));
        assert_eq!(p.state_dir(), Path::new("/mnt/state/ferestre"));
        assert_eq!(p.cache_dir(), Path::new("/mnt/cache/ferestre"));
        assert_eq!(
            p.runtime_capabilities_file().unwrap(),
            Path::new("/mnt/proton/ferestre-capabilities.txt")
        );
    }

    #[test]
    fn an_empty_variable_means_unset() {
        // xodus-env.sh exports XODUS_CLI_DIR= when it finds nothing, and uses
        // ${VAR:-default} everywhere. Inheriting that must not pin a path to "".
        let p = paths(&[("XODUS_GAMES_DIR", ""), ("XODUS_CLI_DIR", "")]);
        assert_eq!(p.games_dir(), Path::new("/home/tester/xbox-games"));
        assert_eq!(p.cli_dir(), None);
    }

    #[test]
    fn a_relative_xdg_home_is_ignored_as_the_spec_requires() {
        let p = paths(&[("XDG_STATE_HOME", "state")]);
        assert_eq!(
            p.state_dir(),
            Path::new("/home/tester/.local/state/ferestre")
        );
    }

    #[test]
    fn steam_and_the_runtime_are_probed_in_order() {
        // ~/.steam/steam does not exist here, so the second candidate wins and
        // the compat tool is looked for inside it.
        let e = env(&[]).with_existing([
            "/home/tester/.local/share/Steam",
            "/home/tester/.local/share/Steam/compatibilitytools.d/xodus",
        ]);
        let p = Paths::resolve(&e).unwrap();
        assert_eq!(
            p.steam_dir(),
            Some(Path::new("/home/tester/.local/share/Steam"))
        );
        assert_eq!(
            p.runtime_dir(),
            Some(Path::new(
                "/home/tester/.local/share/Steam/compatibilitytools.d/xodus"
            ))
        );
    }

    #[test]
    fn a_flatpak_steam_still_finds_the_runtime() {
        // The Flatpak path is a Steam candidate but not one of the three
        // compat-tool candidates in the shell script; it is reached through the
        // resolved Steam directory, so that has to be probed first.
        let flatpak = "/home/tester/.var/app/com.valvesoftware.Steam/.local/share/Steam";
        let e = env(&[]).with_existing([
            flatpak.to_string(),
            format!("{flatpak}/compatibilitytools.d/xodus"),
        ]);
        let p = Paths::resolve(&e).unwrap();
        assert_eq!(
            p.runtime_dir(),
            Some(Path::new(&format!("{flatpak}/compatibilitytools.d/xodus")) as &Path)
        );
    }

    #[test]
    fn the_client_is_found_on_path_before_a_checkout() {
        let e = env(&[("PATH", "/opt/bin:/usr/bin")]).with_existing([
            "/usr/bin/xodus-cli",
            "/home/tester/src/xodus-cli/target/release",
        ]);
        let p = Paths::resolve(&e).unwrap();
        assert_eq!(p.cli_dir(), Some(Path::new("/usr/bin")));

        // With nothing on PATH the checkout is the fallback, as in the script.
        let e = env(&[]).with_existing(["/home/tester/src/xodus-cli/target/release"]);
        let p = Paths::resolve(&e).unwrap();
        assert_eq!(
            p.cli_dir(),
            Some(Path::new("/home/tester/src/xodus-cli/target/release"))
        );
    }

    #[test]
    fn the_tree_is_found_the_same_way_from_a_package_an_appimage_and_a_checkout() {
        let packaged = Paths::resolve(
            &env(&[])
                .with_exe("/usr/bin/ferestre")
                .with_existing(["/usr/lib/ferestre/scripts"]),
        )
        .unwrap();
        assert_eq!(packaged.tree_dir(), Some(Path::new("/usr/lib/ferestre")));
        assert_eq!(
            packaged.scripts_dir(),
            Some(Path::new("/usr/lib/ferestre/scripts"))
        );
        // The AUR package ships no titles/ yet; say so rather than point at it.
        assert_eq!(packaged.titles_dir(), None);

        let appimage = Paths::resolve(
            &env(&[("APPDIR", "/tmp/.mount_ferestreAB")])
                .with_exe("/tmp/.mount_ferestreAB/usr/bin/ferestre")
                .with_existing([
                    "/tmp/.mount_ferestreAB/usr/lib/ferestre/scripts",
                    "/tmp/.mount_ferestreAB/usr/lib/ferestre/titles",
                ]),
        )
        .unwrap();
        assert_eq!(
            appimage.titles_dir(),
            Some(Path::new("/tmp/.mount_ferestreAB/usr/lib/ferestre/titles"))
        );

        // A test binary lives three directories below the checkout root.
        let checkout = Paths::resolve(
            &env(&[])
                .with_exe("/home/tester/src/ferestre/target/debug/deps/ferestre_core-1234")
                .with_existing([
                    "/home/tester/src/ferestre/scripts",
                    "/home/tester/src/ferestre/titles",
                ]),
        )
        .unwrap();
        assert_eq!(
            checkout.tree_dir(),
            Some(Path::new("/home/tester/src/ferestre"))
        );
    }

    #[test]
    fn a_release_archive_uses_its_bundled_client_and_runtime() {
        let root = "/opt/ferestre/usr/lib/ferestre";
        let p = Paths::resolve(
            &env(&[("PATH", "/usr/bin")])
                .with_exe("/opt/ferestre/usr/bin/ferestre")
                .with_existing([
                    format!("{root}/scripts"),
                    format!("{root}/client"),
                    format!("{root}/runtime"),
                    "/usr/bin/xodus-cli".to_string(),
                ]),
        )
        .unwrap();

        assert_eq!(p.tree_dir(), Some(Path::new(root)));
        assert_eq!(
            p.cli_dir(),
            Some(Path::new("/opt/ferestre/usr/lib/ferestre/client"))
        );
        assert_eq!(
            p.runtime_dir(),
            Some(Path::new("/opt/ferestre/usr/lib/ferestre/runtime"))
        );
    }

    #[test]
    fn a_title_installs_under_the_games_directory() {
        let p = paths(&[]);
        let r = recipe("");
        assert_eq!(
            p.install_dir(&r).unwrap(),
            Path::new("/home/tester/xbox-games/example")
        );
        assert_eq!(
            p.prefix_dir(&r).unwrap(),
            Path::new("/home/tester/xbox-games/example-proton")
        );
        assert_eq!(
            p.log_file(&r).unwrap(),
            Path::new("/home/tester/xbox-games/example-launch.log")
        );
    }

    #[test]
    fn bedrock_installs_where_its_recipe_says_not_where_its_slug_does() {
        // The historic layout keeps the package one level down. This is the
        // case that makes install.dir exist at all.
        let p = paths(&[]);
        let r = recipe("[install]\ndir = \"bedrock/game\"\nprefix = \"bedrock-proton\"");
        assert_eq!(
            p.install_dir(&r).unwrap(),
            Path::new("/home/tester/xbox-games/bedrock/game")
        );
        assert_eq!(
            p.prefix_dir(&r).unwrap(),
            Path::new("/home/tester/xbox-games/bedrock-proton")
        );
    }

    #[test]
    fn the_recipes_env_override_wins_and_may_point_off_the_games_drive() {
        // Someone with a working install has BEDROCK_DIR pointing at another
        // disk; that must keep working, so the override is taken as given.
        let p = paths(&[
            ("BEDROCK_DIR", "/run/media/user/Games/bedrock"),
            ("BEDROCK_PREFIX", "/run/media/user/Games/bedrock-proton"),
        ]);
        let r = recipe(
            "[install]\ndir = \"bedrock/game\"\ndir-env = \"BEDROCK_DIR\"\n\
             prefix = \"bedrock-proton\"\nprefix-env = \"BEDROCK_PREFIX\"",
        );
        assert_eq!(
            p.install_dir(&r).unwrap(),
            Path::new("/run/media/user/Games/bedrock")
        );
        assert_eq!(
            p.prefix_dir(&r).unwrap(),
            Path::new("/run/media/user/Games/bedrock-proton")
        );
    }

    #[test]
    fn an_unset_or_empty_env_override_falls_back_to_the_recipe() {
        let p = paths(&[("BEDROCK_DIR", "")]);
        let r = recipe("[install]\ndir = \"bedrock/game\"\ndir-env = \"BEDROCK_DIR\"");
        assert_eq!(
            p.install_dir(&r).unwrap(),
            Path::new("/home/tester/xbox-games/bedrock/game")
        );
    }

    #[test]
    fn a_recipe_cannot_escape_the_games_directory() {
        let p = paths(&[]);
        for dir in ["../etc", "/etc", "game/../../etc", ".."] {
            let r = recipe(&format!("[install]\ndir = {dir:?}"));
            let err = p
                .install_dir(&r)
                .expect_err("{dir} should be refused")
                .to_string();
            assert!(err.contains(dir), "error should quote the path: {err}");
            assert!(
                err.contains("9NBLGGH2JHXJ"),
                "error should name the title: {err}"
            );
        }
        // The prefix is data from the same file and gets the same treatment.
        let r = recipe("[install]\nprefix = \"../../elsewhere\"");
        assert!(p.prefix_dir(&r).is_err());

        // An empty value is not a way to land on the games directory itself.
        let r = recipe("[install]\ndir = \"\"");
        assert!(p.install_dir(&r).is_err());

        // A leading "./" is harmless and stays allowed.
        let r = recipe("[install]\ndir = \"./example\"");
        assert_eq!(
            p.install_dir(&r).unwrap(),
            Path::new("/home/tester/xbox-games/example")
        );
    }

    #[test]
    fn the_repository_recipes_resolve_to_what_the_launch_scripts_use() {
        // titles/SCHEMA.md states this equivalence and scripts/launch-*.sh are
        // what people are running today; if the two drift apart, an existing
        // install silently gets a second copy of the game.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles");
        if !dir.exists() {
            return; // packaged crate without the repository around it
        }
        let p = paths(&[]);
        let games = Path::new("/home/tester/xbox-games");
        for r in Recipe::load_dir(&dir).expect("repository recipes should parse") {
            let expected = match r.title.slug.as_str() {
                "bedrock" => games.join("bedrock/game"),
                slug => games.join(slug),
            };
            assert_eq!(p.install_dir(&r).unwrap(), expected, "{}", r.title.slug);
            assert_eq!(
                p.prefix_dir(&r).unwrap(),
                games.join(format!("{}-proton", r.title.slug))
            );
            assert_eq!(
                p.log_file(&r).unwrap(),
                games.join(format!("{}-launch.log", r.title.slug))
            );
        }
    }
}

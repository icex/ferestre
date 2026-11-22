//! Steam's non-Steam shortcuts, read as local games.
//!
//! A shortcut's launch options are a line Steam hands to a shell with
//! `%command%` replaced by the program. Ferestre does not run a shell, so the
//! line is split into words the way `sh` would split it -- quotes and
//! backslashes honoured, nothing expanded -- and anything that only a shell
//! could carry out (`;`, `|`, redirections, command substitution) is refused
//! rather than guessed at.
//!
//! [`plan`] reads every Steam user's `shortcuts.vdf`, Steam's
//! `config.vdf` (which Proton each shortcut uses) and its libraries (where
//! each prefix is), and says what would become of each shortcut. [`apply`]
//! writes user recipes, art and runtime choices -- never anything under a
//! Steam root -- and leaves every recipe already there as it is, so a second
//! import keeps the person's edits.

use crate::anticheat::{self, Detection};
use crate::paths::Paths;
use crate::recipe::{Recipe, TitleKind};
use crate::steam::{self, ArtworkFiles, Value};
use crate::{local, runtime};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A shortcut's launch options, taken apart.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    /// `NAME=value` words before the first wrapper.
    pub env: BTreeMap<String, String>,
    /// The programs before `%command%` (`gamemoderun`, `mangohud`).
    pub wrapper: Vec<String>,
    /// The game's arguments: words after `%command%`, or every word when
    /// there is no `%command%` (Steam appends them).
    pub args: Vec<String>,
}

const COMMAND: &str = "%command%";

/// Split Steam launch options into environment, wrappers and arguments.
pub fn parse_launch_options(text: &str) -> Result<LaunchOptions> {
    let words = split_words(text)?;
    let mut options = LaunchOptions::default();
    let Some(at) = words.iter().position(|w| w == COMMAND) else {
        options.args = words;
        return Ok(options);
    };
    let mut before = words[..at].iter();
    for word in before.by_ref() {
        match env_assignment(word) {
            Some((name, value)) if options.wrapper.is_empty() => {
                options.env.insert(name.to_owned(), value.to_owned());
            }
            _ => {
                options.wrapper.push(word.clone());
                break;
            }
        }
    }
    options.wrapper.extend(before.cloned());
    options.args = words[at + 1..].to_vec();
    Ok(options)
}

/// `NAME=value`, where `NAME` is `[A-Za-z_][A-Za-z0-9_]*`.
fn env_assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let mut bytes = name.bytes();
    let first = bytes.next()?;
    (first.is_ascii_alphabetic() || first == b'_')
        .then_some(())
        .filter(|_| bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .map(|_| (name, value))
}

/// Split `text` into words like a POSIX shell, expanding nothing. Shell
/// operators outside single quotes are an error naming the operator.
fn split_words(text: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if let Some(done) = word.take() {
                    words.push(done);
                }
            }
            '\'' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => bail!("a ' is never closed"),
                    }
                }
            }
            '"' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => current.push(c),
                            Some('\n') => {}
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => bail!("a \" is never closed"),
                        },
                        // A shell would run these even inside double quotes.
                        Some('`') => bail!("the shell operator ` is not supported"),
                        Some('$') if chars.peek() == Some(&'(') => {
                            bail!("the shell operator $( is not supported")
                        }
                        Some(c) => current.push(c),
                        None => bail!("a \" is never closed"),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(c) => word.get_or_insert_with(String::new).push(c),
                None => bail!("the line ends with a lone \\"),
            },
            ';' | '&' | '|' | '<' | '>' | '`' => {
                bail!("the shell operator {c} is not supported")
            }
            '$' if chars.peek() == Some(&'(') => {
                bail!("the shell operator $( is not supported")
            }
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    if let Some(done) = word {
        words.push(done);
    }
    Ok(words)
}

/// Words written so that [`parse_launch_options`] splits them back the same:
/// bare when made only of `[A-Za-z0-9_./=:,+@%-]`, single-quoted otherwise.
pub fn quote_words(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let bare = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_./=:,+@%-".contains(c));
            if bare {
                word.clone()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `value` with a leading `~/`, `$HOME/` or `${HOME}/` (or the whole value
/// being one of those without the slash) replaced by `home`. Nothing else is
/// expanded.
pub fn expand_home(value: &str, home: &Path) -> String {
    for spelling in ["~", "${HOME}", "$HOME"] {
        if value == spelling {
            return home.display().to_string();
        }
        if let Some(rest) = value
            .strip_prefix(spelling)
            .and_then(|r| r.strip_prefix('/'))
        {
            return home.join(rest).display().to_string();
        }
    }
    value.to_owned()
}

/// What an import does with one shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportStatus {
    /// A recipe will be written.
    New,
    /// A recipe with this id is already in the user's titles; it stays.
    Kept,
    /// Not imported, and why.
    Skipped(String),
}

/// The Proton Steam uses for a shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeFound {
    /// Steam's tool, found at this directory.
    Tool(PathBuf),
    /// Steam names a tool that is not installed.
    Missing(String),
    /// Steam has no tool set for it.
    Unset,
}

/// One shortcut and what importing it would do.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub app_id: u32,
    pub name: String,
    /// The Steam user whose shortcut it is.
    pub user: String,
    pub status: ImportStatus,
    /// `None` when skipped.
    pub recipe: Option<Recipe>,
    pub runtime: RuntimeFound,
    pub artwork: ArtworkFiles,
    pub anticheat: Vec<Detection>,
    pub warnings: Vec<String>,
}

/// What an import did: ids added and kept, and names skipped with why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub added: Vec<String>,
    pub kept: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

const SUMMARY: &str = "Imported from Steam";

/// A path Steam stores, quoted or not.
fn unquote(value: &str) -> &str {
    value.trim().trim_matches('"')
}

/// `dir` without trailing slashes, except `/` itself.
fn without_trailing_slash(dir: &str) -> String {
    let trimmed = dir.trim_end_matches('/');
    if trimmed.is_empty() && dir.starts_with('/') {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// `CompatToolMapping` of one Steam install: app id to tool name.
fn compat_tool_mapping(root: &Path) -> BTreeMap<String, String> {
    let Some(doc) = std::fs::read_to_string(root.join("config/config.vdf"))
        .ok()
        .and_then(|text| crate::vdf::parse(&text).ok())
    else {
        return BTreeMap::new();
    };
    let mapping = ["Software", "Valve", "Steam", "CompatToolMapping"]
        .iter()
        .try_fold(doc.get("InstallConfigStore"), |at, key| {
            Some(at.and_then(|v| v.get(key)))
        })
        .flatten();
    mapping
        .and_then(Value::as_map)
        .unwrap_or_default()
        .iter()
        .filter_map(|(id, entry)| {
            let name = entry.get("name").and_then(Value::as_str)?;
            (!name.is_empty()).then(|| (id.clone(), name.to_owned()))
        })
        .collect()
}

/// The first of `<slot><ext>` in `grid` for each Steam art slot, else the
/// shortcut's own icon.
fn shortcut_artwork(grid: &Path, app_id: u32, icon: &str) -> ArtworkFiles {
    let find = |suffix: &str| {
        ["png", "jpg", "jpeg"]
            .iter()
            .map(|ext| grid.join(format!("{app_id}{suffix}.{ext}")))
            .find(|p| p.is_file())
    };
    let own_icon = Some(PathBuf::from(unquote(icon))).filter(|p| {
        p.is_file()
            && p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ["png", "jpg", "jpeg"].contains(&e.to_ascii_lowercase().as_str()))
    });
    ArtworkFiles {
        portrait: find("p"),
        landscape: find(""),
        hero: find("_hero"),
        logo: find("_logo"),
        icon: find("_icon").or(own_icon),
    }
}

fn fill_missing_art(art: &mut ArtworkFiles, more: ArtworkFiles) {
    art.portrait = art.portrait.take().or(more.portrait);
    art.landscape = art.landscape.take().or(more.landscape);
    art.hero = art.hero.take().or(more.hero);
    art.logo = art.logo.take().or(more.logo);
    art.icon = art.icon.take().or(more.icon);
}

/// Whether `prefix` is Steam's own `steamapps/compatdata/<app id>` folder.
fn is_steam_prefix(prefix: &Path, app_id: u32) -> bool {
    let name = |p: Option<&Path>| {
        p.and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .map(str::to_owned)
    };
    let parent = prefix.parent();
    name(Some(prefix)).as_deref() == Some(app_id.to_string().as_str())
        && name(parent).as_deref() == Some("compatdata")
        && name(parent.and_then(Path::parent)).as_deref() == Some("steamapps")
}

/// Every shortcut of every Steam user (or only `user`), and what importing it
/// would do. Reads only.
pub fn plan(paths: &Paths, user: Option<&str>) -> Result<Vec<Candidate>> {
    let roots = paths.steam_roots();
    let tools = runtime::steam_tools(paths);
    let libraries: Vec<PathBuf> = roots.iter().flat_map(|r| steam::libraries(r)).collect();
    let mappings: Vec<BTreeMap<String, String>> =
        roots.iter().map(|r| compat_tool_mapping(r)).collect();
    let user_titles = paths.user_titles_dir();
    // App id to its candidate's index.
    let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
    let mut candidates: Vec<Candidate> = Vec::new();

    for (root_index, root) in roots.iter().enumerate() {
        for file in steam::shortcut_files(root) {
            let config = file.parent().unwrap_or(root);
            let owner = config
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            if user.is_some_and(|u| u != owner) {
                continue;
            }
            let doc = steam::load(&file).with_context(|| format!("{}", file.display()))?;
            let entries = doc
                .get("shortcuts")
                .and_then(Value::as_map)
                .unwrap_or_default();
            for (_, entry) in entries {
                let text = |key: &str| {
                    entry
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                let name = text("AppName");
                let raw_exe = text("Exe");
                let app_id = match entry.get("appid") {
                    Some(Value::Int(id)) if *id != 0 => *id as u32,
                    _ => steam::shortcut_app_id(&raw_exe, &name),
                };
                if let Some(&first) = seen.get(&app_id) {
                    // The same shortcut in another user or install: the first
                    // one found is imported, and this one only lends it any
                    // art it lacks.
                    let first: &mut Candidate = &mut candidates[first];
                    if first.recipe.is_some() {
                        let more = shortcut_artwork(&config.join("grid"), app_id, &text("icon"));
                        fill_missing_art(&mut first.artwork, more);
                    }
                    continue;
                }
                seen.insert(app_id, candidates.len());
                // The tool Steam set for this shortcut: its own install's
                // mapping first, then any other's.
                let mapped = std::iter::once(root_index)
                    .chain((0..roots.len()).filter(|i| *i != root_index))
                    .find_map(|i| mappings[i].get(&app_id.to_string()).cloned());
                let mut candidate = Candidate {
                    id: local::steam_id(app_id),
                    app_id,
                    name,
                    user: owner.clone(),
                    status: ImportStatus::New,
                    recipe: None,
                    runtime: RuntimeFound::Unset,
                    artwork: ArtworkFiles::default(),
                    anticheat: Vec::new(),
                    warnings: Vec::new(),
                };
                let skip = |mut c: Candidate, why: String| {
                    c.status = ImportStatus::Skipped(why);
                    c
                };
                if steam::is_ferestre_shortcut(entry) {
                    candidates.push(skip(candidate, "a Ferestre shortcut".into()));
                    continue;
                }
                let exe = unquote(&raw_exe).to_owned();
                let lower = exe.to_ascii_lowercase();
                if !(lower.ends_with(".exe") || lower.ends_with(".bat")) {
                    candidates.push(skip(candidate, "not a Windows program".into()));
                    continue;
                }
                let exe_path = Path::new(&exe);
                let start = without_trailing_slash(unquote(&text("StartDir")));
                let dir = if start.is_empty() {
                    exe_path
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                } else {
                    start
                };
                let executable = match exe_path.strip_prefix(&dir) {
                    Ok(inside) if !dir.is_empty() => inside.display().to_string(),
                    _ => exe.clone(),
                };
                if let Some(why) = local::too_broad(paths, Path::new(&dir)) {
                    candidates.push(skip(candidate, why));
                    continue;
                }

                let options = match parse_launch_options(&text("LaunchOptions")) {
                    Ok(options) => options,
                    Err(e) => {
                        candidate.warnings.push(format!(
                            "{}: launch options not imported ({e}); add them in Edit launch \
                             settings",
                            candidate.name
                        ));
                        LaunchOptions::default()
                    }
                };
                let mut env = options.env;
                let prefix = match env.remove("STEAM_COMPAT_DATA_PATH") {
                    Some(value) => {
                        let expanded = expand_home(&value, paths.home());
                        if expanded.contains('$') {
                            candidate.warnings.push(format!(
                                "{}: STEAM_COMPAT_DATA_PATH {expanded} names a variable \
                                 Ferestre does not expand; it is used as written",
                                candidate.name
                            ));
                        }
                        Some(PathBuf::from(without_trailing_slash(&expanded)))
                    }
                    None => libraries
                        .iter()
                        .map(|l| l.join("steamapps/compatdata").join(app_id.to_string()))
                        .find(|p| p.is_dir()),
                };
                if let Some(why) = prefix
                    .as_deref()
                    .filter(|p| p.is_absolute())
                    .and_then(|p| local::too_broad(paths, p))
                {
                    candidates.push(skip(candidate, why));
                    continue;
                }

                candidate.runtime = match mapped {
                    Some(tool) => match runtime::steam_tool(&tools, &tool) {
                        Some(found) => RuntimeFound::Tool(found.dir.clone()),
                        None => {
                            candidate.warnings.push(format!(
                                "Steam's {tool} is not installed; Ferestre's runtime will be used"
                            ));
                            RuntimeFound::Missing(tool)
                        }
                    },
                    None => RuntimeFound::Unset,
                };
                if prefix
                    .as_deref()
                    .is_some_and(|p| is_steam_prefix(p, app_id))
                    && !matches!(candidate.runtime, RuntimeFound::Tool(_))
                {
                    candidate.warnings.push(format!(
                        "{}: its Steam prefix will be started by a different Proton than Steam \
                         used, which may upgrade it; choose Steam's Proton for it in Choose \
                         runtime, or give it its own prefix in Edit launch settings",
                        candidate.name
                    ));
                }

                let mut recipe = Recipe::blank(&candidate.id, &candidate.name);
                recipe.title.kind = TitleKind::Local;
                recipe.title.slug = candidate.id.to_ascii_lowercase();
                recipe.install.dir = Some(dir.clone());
                recipe.install.prefix = prefix.map(|p| p.display().to_string());
                recipe.launch.executable = executable.clone();
                recipe.launch.env = env;
                recipe.launch.wrapper = options.wrapper;
                recipe.launch.args = options.args;
                recipe.status.summary = SUMMARY.into();
                // The loader's own checks: a recipe it would refuse is not written.
                if let Err(e) = recipe.to_toml().and_then(|text| Recipe::parse(&text)) {
                    candidates.push(skip(candidate, format!("{e}")));
                    continue;
                }

                candidate.artwork = shortcut_artwork(&config.join("grid"), app_id, &text("icon"));
                candidate.anticheat = anticheat::detect(Path::new(&dir), &executable);
                if user_titles.join(format!("{}.toml", candidate.id)).exists() {
                    candidate.status = ImportStatus::Kept;
                }
                candidate.recipe = Some(recipe);
                candidates.push(candidate);
            }
        }
    }
    Ok(candidates)
}

/// Write what [`plan`] found: a user recipe, its art and its runtime choice
/// for each new shortcut. A recipe already there is kept as it is, and a
/// runtime already chosen stays chosen. Nothing under a Steam root is written.
pub fn apply(paths: &Paths, candidates: &[Candidate]) -> Result<Report> {
    let mut report = Report::default();
    let user_titles = paths.user_titles_dir();
    for candidate in candidates {
        match (&candidate.status, &candidate.recipe) {
            (ImportStatus::Skipped(why), _) => {
                report.skipped.push((candidate.name.clone(), why.clone()));
            }
            (ImportStatus::Kept, _) | (ImportStatus::New, None) => {
                report.kept.push(candidate.id.clone());
            }
            (ImportStatus::New, Some(recipe)) => {
                if recipe.save_new_to(&user_titles)?.is_none() {
                    report.kept.push(candidate.id.clone());
                    continue;
                }
                local::save_artwork(paths, &candidate.id, &candidate.artwork)?;
                if let RuntimeFound::Tool(dir) = &candidate.runtime {
                    if runtime::selected(paths.config_dir(), &candidate.id).is_none() {
                        runtime::set_selected(paths.config_dir(), &candidate.id, Some(dir))?;
                    }
                }
                report.added.push(candidate.id.clone());
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local;
    use crate::paths::{Env, Paths};
    use crate::runtime;
    use crate::steam::{self, Value};
    use std::path::{Path, PathBuf};

    const CYBERPUNK_APP_ID: u32 = 3_123_456_789;
    const WITCHER_APP_ID: u32 = 2_987_654_321;

    struct SteamHome {
        _home: tempfile::TempDir,
        paths: Paths,
        steam: PathBuf,
        cyberpunk_dir: PathBuf,
        cyberpunk_app_id: u32,
    }

    impl SteamHome {
        fn compatdata(&self, app_id: u32) -> PathBuf {
            self.steam
                .join("steamapps/compatdata")
                .join(app_id.to_string())
        }

        /// Every file and folder under the Steam root, with each file's bytes.
        fn snapshot_steam(&self) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
            fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
                for entry in std::fs::read_dir(dir).unwrap().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        out.insert(path.clone(), None);
                        walk(&path, out);
                    } else {
                        out.insert(path.clone(), Some(std::fs::read(&path).unwrap()));
                    }
                }
            }
            let mut out = BTreeMap::new();
            walk(&self.steam, &mut out);
            out
        }
    }

    fn entry(fields: &[(&str, &str)], app_id: u32) -> Value {
        let mut map: Vec<(String, Value)> = vec![("appid".into(), Value::Int(app_id as i32))];
        map.extend(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), Value::Str(v.to_string()))),
        );
        Value::Map(map)
    }

    fn write_shortcuts(file: &Path, entries: Vec<Value>) {
        let doc = Value::Map(vec![(
            "shortcuts".into(),
            Value::Map(
                entries
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| (i.to_string(), e))
                    .collect(),
            ),
        )]);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, steam::serialise(&doc).unwrap()).unwrap();
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    /// A HOME with one Steam install, two Steam users, a custom Proton,
    /// prefixes, grid art and four game folders.
    fn steam_home() -> SteamHome {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let steam = h.join(".local/share/Steam");
        let games = h.join("Games");
        let cyberpunk_dir = games.join("Cyberpunk 2077");
        touch(&cyberpunk_dir.join("bin/x64/Cyberpunk2077.exe"));
        touch(&games.join("COD/sp26/sp26-cod.exe"));
        touch(&games.join("COD/Randgrid.sys"));
        touch(&games.join("Witcher/witcher3.exe"));

        let ge = steam.join("compatibilitytools.d/GE-Proton10-25");
        touch(&ge.join("proton"));
        std::fs::write(
            ge.join("compatibilitytool.vdf"),
            "\"compatibilitytools\" { \"compat_tools\" { \"GE-Proton10-25\" { \"install_path\" \".\" } } }",
        )
        .unwrap();
        touch(&steam.join(format!(
            "steamapps/compatdata/{CYBERPUNK_APP_ID}/pfx/system.reg"
        )));
        touch(&steam.join(format!(
            "steamapps/compatdata/{WITCHER_APP_ID}/pfx/system.reg"
        )));
        std::fs::create_dir_all(steam.join("config")).unwrap();
        std::fs::write(
            steam.join("config/config.vdf"),
            format!(
                "\"InstallConfigStore\" {{ \"Software\" {{ \"Valve\" {{ \"Steam\" {{\n\
                 \"CompatToolMapping\" {{\n\
                 \"0\" {{ \"name\" \"proton_experimental\" \"config\" \"\" \"priority\" \"75\" }}\n\
                 \"{CYBERPUNK_APP_ID}\" {{ \"name\" \"GE-Proton10-25\" \"config\" \"\" \"priority\" \"250\" }}\n\
                 }} }} }} }} }}\n"
            ),
        )
        .unwrap();

        let cyberpunk = entry(
            &[
                ("AppName", "Cyberpunk 2077"),
                (
                    "Exe",
                    &format!("\"{}/bin/x64/Cyberpunk2077.exe\"", cyberpunk_dir.display()),
                ),
                ("StartDir", &format!("\"{}/\"", cyberpunk_dir.display())),
                ("icon", ""),
                ("LaunchOptions", "DXVK_HDR=1 gamemoderun %command%"),
            ],
            CYBERPUNK_APP_ID,
        );
        let cod_exe = format!("{}/COD/sp26/sp26-cod.exe", games.display());
        let cod = entry(
            &[
                ("AppName", "COD MW4"),
                ("Exe", &cod_exe),
                ("StartDir", ""),
                (
                    "LaunchOptions",
                    "STEAM_COMPAT_DATA_PATH=~/Games/prefixes/cod-mw4 %command% -windowed",
                ),
            ],
            0,
        );
        let ferestre = entry(
            &[
                ("AppName", "Halo"),
                ("Exe", "\"/usr/bin/ferestre\""),
                ("LaunchOptions", "run 9NBLGGH2JHXJ"),
            ],
            0,
        );
        let heroic = entry(&[("AppName", "Heroic"), ("Exe", "\"/usr/bin/heroic\"")], 0);
        let witcher = entry(
            &[
                ("AppName", "Witcher 3"),
                (
                    "Exe",
                    &format!("\"{}/Witcher/witcher3.exe\"", games.display()),
                ),
                ("StartDir", &format!("\"{}/Witcher\"", games.display())),
            ],
            WITCHER_APP_ID,
        );
        let user = steam.join("userdata/12345678/config");
        write_shortcuts(
            &user.join("shortcuts.vdf"),
            vec![cyberpunk.clone(), cod, ferestre, heroic, witcher],
        );
        touch(&user.join(format!("grid/{CYBERPUNK_APP_ID}p.png")));
        touch(&user.join(format!("grid/{CYBERPUNK_APP_ID}_hero.jpg")));
        // The same game in a second Steam user.
        write_shortcuts(
            &steam.join("userdata/111/config/shortcuts.vdf"),
            vec![cyberpunk],
        );

        let paths = Paths::resolve(&Env::from_map([("HOME", h.to_str().unwrap())])).unwrap();
        SteamHome {
            _home: home,
            paths,
            steam,
            cyberpunk_dir,
            cyberpunk_app_id: CYBERPUNK_APP_ID,
        }
    }

    #[test]
    fn a_shortcut_becomes_a_local_recipe_with_its_prefix_runtime_and_art() {
        let h = steam_home();
        let found = plan(&h.paths, None).unwrap();
        let cp = found.iter().find(|c| c.name == "Cyberpunk 2077").unwrap();
        let r = cp.recipe.as_ref().unwrap();
        assert_eq!(cp.id, format!("LOCAL-STEAM-{}", cp.app_id));
        assert!(r.is_local());
        assert_eq!(r.status.summary, "Imported from Steam");
        assert_eq!(
            r.install.dir.as_deref(),
            Some(h.cyberpunk_dir.to_str().unwrap())
        );
        assert_eq!(r.launch.executable, "bin/x64/Cyberpunk2077.exe");
        assert_eq!(r.launch.wrapper, ["gamemoderun"]);
        assert_eq!(r.launch.env.get("DXVK_HDR").map(String::as_str), Some("1"));
        assert!(!r.launch.env.contains_key("STEAM_COMPAT_DATA_PATH"));
        assert_eq!(
            r.install.prefix.as_deref(),
            Some(h.compatdata(cp.app_id).to_str().unwrap())
        );
        assert!(matches!(&cp.runtime, RuntimeFound::Tool(dir) if dir.ends_with("GE-Proton10-25")));
        assert!(cp.artwork.portrait.is_some() && cp.artwork.hero.is_some());
        assert!(matches!(cp.status, ImportStatus::New));
        // Both Steam users have it; the first found is imported, and the
        // other's grid art fills it in.
        assert_eq!(cp.user, "111");
        // The recipe is one the loader accepts.
        Recipe::parse(&r.to_toml().unwrap()).unwrap();
    }

    #[test]
    fn an_explicit_prefix_in_the_options_wins_and_ferestres_own_shortcuts_are_skipped() {
        let h = steam_home();
        let found = plan(&h.paths, None).unwrap();
        let cod = found.iter().find(|c| c.name == "COD MW4").unwrap();
        let r = cod.recipe.as_ref().unwrap();
        let exe = h.paths.home().join("Games/COD/sp26/sp26-cod.exe");
        assert_eq!(
            cod.app_id,
            steam::shortcut_app_id(exe.to_str().unwrap(), "COD MW4")
        );
        assert_eq!(
            r.install.prefix.as_deref(),
            Some(
                h.paths
                    .home()
                    .join("Games/prefixes/cod-mw4")
                    .to_str()
                    .unwrap()
            )
        );
        assert_eq!(r.install.dir.as_deref(), exe.parent().unwrap().to_str());
        assert_eq!(r.launch.executable, "sp26-cod.exe");
        assert_eq!(r.launch.args, ["-windowed"]);
        assert!(r.launch.env.is_empty() && r.launch.wrapper.is_empty());
        assert!(matches!(cod.runtime, RuntimeFound::Unset));
        assert!(!cod.warnings.iter().any(|w| w.contains("different Proton")));
        assert!(
            cod.anticheat.is_empty(),
            "only the exe's folder and the start dir are looked at"
        );

        let halo = found.iter().find(|c| c.name == "Halo").unwrap();
        assert!(matches!(halo.status, ImportStatus::Skipped(_)) && halo.recipe.is_none());
        let heroic = found.iter().find(|c| c.name == "Heroic").unwrap();
        assert!(
            matches!(&heroic.status, ImportStatus::Skipped(why) if why == "not a Windows program")
        );
    }

    #[test]
    fn a_shortcut_in_two_steam_users_is_imported_once() {
        let h = steam_home();
        let found = plan(&h.paths, None).unwrap();
        assert_eq!(
            found
                .iter()
                .filter(|c| c.app_id == h.cyberpunk_app_id)
                .count(),
            1
        );
        let only = plan(&h.paths, Some("111")).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].user, "111");
    }

    #[test]
    fn a_steam_prefix_without_steams_proton_is_flagged() {
        let h = steam_home();
        let found = plan(&h.paths, None).unwrap();
        let witcher = found.iter().find(|c| c.name == "Witcher 3").unwrap();
        assert!(matches!(witcher.runtime, RuntimeFound::Unset));
        assert_eq!(
            witcher.recipe.as_ref().unwrap().install.prefix.as_deref(),
            Some(h.compatdata(WITCHER_APP_ID).to_str().unwrap())
        );
        assert_eq!(
            witcher
                .warnings
                .iter()
                .filter(|w| w.contains("different Proton"))
                .count(),
            1
        );
        let cp = found.iter().find(|c| c.name == "Cyberpunk 2077").unwrap();
        assert!(!cp.warnings.iter().any(|w| w.contains("different Proton")));
    }

    #[test]
    fn a_proton_steam_names_but_does_not_have_is_reported_missing() {
        let h = steam_home();
        std::fs::remove_dir_all(h.steam.join("compatibilitytools.d")).unwrap();
        let found = plan(&h.paths, None).unwrap();
        let cp = found.iter().find(|c| c.name == "Cyberpunk 2077").unwrap();
        assert!(matches!(&cp.runtime, RuntimeFound::Missing(name) if name == "GE-Proton10-25"));
        assert!(cp
            .warnings
            .iter()
            .any(|w| w.contains("GE-Proton10-25 is not installed")));
        assert!(cp.warnings.iter().any(|w| w.contains("different Proton")));
    }

    #[test]
    fn import_writes_recipes_and_never_touches_steam() {
        let h = steam_home();
        let before = h.snapshot_steam();
        let report = apply(&h.paths, &plan(&h.paths, None).unwrap()).unwrap();
        assert!(!report.added.is_empty());
        assert_eq!(report.skipped.len(), 2);
        assert_eq!(h.snapshot_steam(), before);
        for id in &report.added {
            assert!(h
                .paths
                .user_titles_dir()
                .join(format!("{id}.toml"))
                .is_file());
        }
        let cyberpunk = local::steam_id(h.cyberpunk_app_id);
        assert!(local::artwork(&h.paths, &cyberpunk).portrait.is_some());
        assert_eq!(
            runtime::selected(h.paths.config_dir(), &cyberpunk),
            Some(h.steam.join("compatibilitytools.d/GE-Proton10-25"))
        );
        let (loaded, problems) = Recipe::load_layered_report(&h.paths.title_dirs());
        assert!(problems.is_empty(), "{problems:?}");
        assert!(loaded.iter().any(|r| r.title.product_id == cyberpunk));
    }

    #[test]
    fn a_second_import_keeps_edits_and_runtime_choices() {
        let h = steam_home();
        let first = apply(&h.paths, &plan(&h.paths, None).unwrap()).unwrap();
        let id = first.added[0].clone();
        let file = h.paths.user_titles_dir().join(format!("{id}.toml"));
        std::fs::write(
            &file,
            std::fs::read_to_string(&file)
                .unwrap()
                .replace("Imported from Steam", "Mine"),
        )
        .unwrap();
        runtime::set_selected(h.paths.config_dir(), &id, Some(Path::new("/elsewhere"))).unwrap();
        let again = plan(&h.paths, None).unwrap();
        assert!(again
            .iter()
            .any(|c| c.id == id && matches!(c.status, ImportStatus::Kept)));
        let second = apply(&h.paths, &again).unwrap();
        assert!(second.added.is_empty() && second.kept.contains(&id));
        assert!(std::fs::read_to_string(&file).unwrap().contains("Mine"));
        assert_eq!(
            runtime::selected(h.paths.config_dir(), &id).as_deref(),
            Some(Path::new("/elsewhere"))
        );
    }

    #[test]
    fn environment_wrappers_and_arguments_split_at_command() {
        let o = parse_launch_options(
            r#"PROTON_ENABLE_WAYLAND=1 DXVK_HDR=1 STEAM_COMPAT_DATA_PATH="/home/me/Games/prefixes/cod mw4" gamemoderun %command% -dx12 "--profile=a b""#,
        )
        .unwrap();
        assert_eq!(
            o.env["STEAM_COMPAT_DATA_PATH"],
            "/home/me/Games/prefixes/cod mw4"
        );
        assert_eq!(o.env.len(), 3);
        assert_eq!(o.wrapper, ["gamemoderun"]);
        assert_eq!(o.args, ["-dx12", "--profile=a b"]);
    }

    #[test]
    fn an_environment_word_after_a_wrapper_stays_in_the_wrapper() {
        let o = parse_launch_options("A=1 env B=2 mangohud %command%").unwrap();
        assert_eq!(o.env.len(), 1);
        assert_eq!(o.wrapper, ["env", "B=2", "mangohud"]);
        assert!(o.args.is_empty());
    }

    #[test]
    fn without_command_every_word_is_an_argument() {
        assert_eq!(
            parse_launch_options("-skipintro 'two words'").unwrap(),
            LaunchOptions {
                args: vec!["-skipintro".into(), "two words".into()],
                ..Default::default()
            }
        );
        assert_eq!(parse_launch_options("").unwrap(), LaunchOptions::default());
    }

    #[test]
    fn shell_operators_are_refused_not_guessed() {
        for text in [
            "A=1 %command% ; rm x",
            "%command% | tee log",
            "X=$(id) %command%",
            "%command% > log",
            "%command% && x",
            "%command% `id`",
            "%command% 'unterminated",
        ] {
            assert!(parse_launch_options(text).is_err(), "{text}");
        }
        let e = parse_launch_options("%command% | tee log").unwrap_err();
        assert!(e.to_string().contains('|'), "{e}");
        assert!(parse_launch_options(r#"%command% "a;b""#).is_ok());
        assert_eq!(
            parse_launch_options(r#"%command% a\;b "x\"y" "\$HOME""#)
                .unwrap()
                .args,
            ["a;b", "x\"y", "$HOME"]
        );
    }

    #[test]
    fn quoted_words_split_back_to_the_same_words() {
        let words: Vec<String> = ["-dx12", "a b", "it's", ""].map(String::from).to_vec();
        assert_eq!(quote_words(&words), r#"-dx12 'a b' 'it'\''s' ''"#);
        assert_eq!(
            parse_launch_options(&quote_words(&words)).unwrap().args,
            words
        );
    }

    #[test]
    fn only_the_home_directory_is_expanded() {
        let home = Path::new("/home/me");
        assert_eq!(expand_home("~/Games/p", home), "/home/me/Games/p");
        assert_eq!(expand_home("${HOME}/p", home), "/home/me/p");
        assert_eq!(expand_home("$HOME/p", home), "/home/me/p");
        assert_eq!(expand_home("~", home), "/home/me");
        assert_eq!(expand_home("$XDG_DATA_HOME/p", home), "$XDG_DATA_HOME/p");
        assert_eq!(expand_home("/a/~/b", home), "/a/~/b");
    }
}

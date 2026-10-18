//! A title's recipe: everything that differs between one game and the next.
//!
//! One TOML per Microsoft Store product id, because that is the identifier a
//! person actually has -- it is in the Store URL, and it is what the collections
//! API returns for an owned title, so a recipe joins directly to a library
//! listing. `titles/SCHEMA.md` is the normative description; this mirrors it.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// How well a title is known to run. The compatibility matrix is generated from
/// this, so the values are deliberately few and unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TitleState {
    /// Runs, with no known caveats worth listing.
    Playable,
    /// Runs, but with issues a person should know about before starting.
    PlayableWithIssues,
    /// Starts and reaches its menus, but has not been taken into play. Separate
    /// from `Playable` because "it launched" and "it plays" are different
    /// claims, and separate from `Broken` because it is a long way from not
    /// running at all.
    Menus,
    /// Starts and does not get far enough to play.
    Broken,
    /// Has a recipe but nobody has run it.
    Untested,
}

impl TitleState {
    /// Whether launching is worth attempting at all.
    pub fn is_runnable(self) -> bool {
        matches!(self, TitleState::Playable | TitleState::PlayableWithIssues)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TitleState::Playable => "playable",
            TitleState::PlayableWithIssues => "playable-with-issues",
            TitleState::Menus => "menus",
            TitleState::Broken => "broken",
            TitleState::Untested => "untested",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Title {
    pub product_id: String,
    pub name: String,
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_size_gb: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Install {
    /// Where the package lands, relative to the games directory. Defaults to the
    /// slug; Bedrock is the exception, which is why this is overridable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// The environment variable the existing shell scripts honour for that path.
    /// Kept so an existing install keeps working after someone switches to the
    /// launcher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Launch {
    /// The executable inside the package, as the client expects it.
    pub executable: String,
    /// Extra environment the title needs, beyond what every title gets.
    #[serde(
        default,
        alias = "environment",
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub env: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Runtime {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wants: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Status {
    pub state: TitleState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stops_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verified: Option<toml::value::Datetime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_with: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Issue {
    pub symptom: String,
    pub cause: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// A regular expression that spots this issue in a launch log, so a failure
    /// can be recognised rather than re-diagnosed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_match: Option<String>,
}

/// A step that has to happen around install or launch, beyond unpacking.
///
/// Kept as data rather than code so a title can describe what it needs without
/// anyone adding a special case to the launcher. `substitute-xcurl` is the one
/// that exists today: Microsoft's XCurl.dll loads under Wine but never gets a
/// request onto the wire, and an update puts it back, so the swap has to run
/// again after every download.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Setup {
    pub action: String,
    /// When it runs, e.g. `after-install`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Why it is needed. Prose, for a person reading the recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub so: Option<String>,
}

/// Where a title keeps its saves, on Windows and inside the prefix, so they can
/// be found, backed up, or carried across from a Windows install.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Saves {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: u32,
    /// Written by the launcher from a package manifest, not by a person.
    ///
    /// A guessed recipe is a stopgap until somebody writes a tested one, so it
    /// ranks below a packaged recipe for the same title, where an edited one
    /// ranks above it. The editor clears this when someone saves their own
    /// version: from then on it is theirs.
    #[serde(default, skip_serializing_if = "is_false")]
    pub generated: bool,
    pub title: Title,
    #[serde(default)]
    pub install: Install,
    pub launch: Launch,
    #[serde(default)]
    pub runtime: Runtime,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<Issue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<Setup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saves: Vec<Saves>,
    /// The prefix, relative to the games directory, of a generated recipe this
    /// one replaced in [`Recipe::load_layered`]. Never read from or written to
    /// a file. The two recipes rarely share a slug, and the prefix holds the
    /// saves, so [`crate::paths::Paths::prefix_dir`] keeps using this one until
    /// the replacement's own prefix exists.
    #[serde(skip)]
    pub superseded_prefix: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One recipe file that could not be used, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadProblem {
    pub path: PathBuf,
    pub error: String,
}

impl std::fmt::Display for LoadProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.error)
    }
}

/// The one variable a recipe with this slug may take a directory from:
/// `<SLUG>_DIR` for `install.dir-env`, `<SLUG>_PREFIX` for
/// `install.prefix-env`, with the slug uppercased and its dashes made
/// underscores (`halo-infinite` gives `HALO_INFINITE_DIR`).
pub fn location_variable(slug: &str, suffix: &str) -> String {
    format!("{}{suffix}", slug.to_ascii_uppercase().replace('-', "_"))
}

/// Whether `name` is a variable a recipe with this slug may take a directory
/// from, `suffix` being `_DIR` (`install.dir-env`) or `_PREFIX`
/// (`install.prefix-env`).
///
/// Those exist so an install the shell scripts made keeps working, and every
/// one of them is named after its title: `BEDROCK_DIR`, `FH5_PREFIX`. So the
/// name has to be exactly the title's own, [`location_variable`]. A pattern
/// with a list of exceptions would still let a recipe name `CARGO_TARGET_DIR`
/// or `PULSE_RUNTIME_DIR`, and whatever that holds would become the title's
/// folder -- the thing an uninstall deletes. The launcher's own variables and
/// the system's are refused even when a slug spells one (`xdg-runtime`), for
/// the same reason. `titles/validate.py` applies the same rule.
pub fn is_location_variable(name: &str, slug: &str, suffix: &str) -> bool {
    const RESERVED: [&str; 7] = [
        "XODUS_",
        "FERESTRE_",
        "XDG_",
        "STEAM_",
        "PROTON_",
        "WINE",
        "APPDIR",
    ];
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_uppercase())
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && matches!(suffix, "_DIR" | "_PREFIX")
        && name == location_variable(slug, suffix)
        && !RESERVED.iter().any(|reserved| name.starts_with(reserved))
}

/// Whether `slug` is lowercase letters, digits and dashes, starting with a
/// letter or digit. It names the install directory, the prefix and the log
/// file, so anything else -- a `/`, a `..` -- would put those somewhere the
/// recipe chose rather than in the games directory.
pub fn is_slug(slug: &str) -> bool {
    let mut bytes = slug.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The schema version this build understands. A recipe from the future is
/// refused rather than half-read.
pub const SCHEMA: u32 = 1;

impl Recipe {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let recipe: Recipe = toml::from_str(text)?;
        if recipe.schema != SCHEMA {
            anyhow::bail!(
                "recipe schema {} is not supported (this build understands {})",
                recipe.schema,
                SCHEMA
            );
        }
        if !is_slug(&recipe.title.slug) {
            anyhow::bail!(
                "[title].slug \"{}\" must be lowercase letters, digits and dashes",
                recipe.title.slug
            );
        }
        for (key, name, suffix) in [
            ("dir-env", &recipe.install.dir_env, "_DIR"),
            ("prefix-env", &recipe.install.prefix_env, "_PREFIX"),
        ] {
            if let Some(name) = name {
                if !is_location_variable(name, &recipe.title.slug, suffix) {
                    anyhow::bail!(
                        "[install].{key} \"{name}\" is not a variable this recipe may take a \
                         directory from: it must be {}, named after the slug, and not one of \
                         the launcher's or the system's own (XODUS_*, FERESTRE_*, XDG_*, \
                         STEAM_*, PROTON_*, WINE*)",
                        location_variable(&recipe.title.slug, suffix)
                    );
                }
            }
        }
        Ok(recipe)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    /// Every recipe in a directory, sorted by product id so output is stable.
    /// `capabilities.toml` is the runtime registry, not a title.
    ///
    /// Strict: the first file that does not parse fails the whole call. That
    /// is right for the repository's own recipes, which CI checks; a launch
    /// reads through [`Recipe::load_layered`], which is not.
    pub fn load_dir(dir: &Path) -> anyhow::Result<Vec<Recipe>> {
        let (found, problems) = Self::load_dir_report(dir);
        if let Some(problem) = problems.into_iter().next() {
            anyhow::bail!("{problem}");
        }
        Ok(found)
    }

    /// Every recipe in a directory that parses, and a problem for each file
    /// that does not.
    pub fn load_dir_report(dir: &Path) -> (Vec<Recipe>, Vec<LoadProblem>) {
        let mut found = Vec::new();
        let mut problems = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                problems.push(LoadProblem {
                    path: dir.to_path_buf(),
                    error: e.to_string(),
                });
                return (found, problems);
            }
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(e) => problems.push(LoadProblem {
                    path: dir.to_path_buf(),
                    error: e.to_string(),
                }),
            }
        }
        paths.sort();
        for path in paths {
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if path.file_stem().and_then(|s| s.to_str()) == Some("capabilities") {
                continue;
            }
            let loaded = std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|text| Self::parse(&text));
            match loaded {
                Ok(recipe) => found.push(recipe),
                Err(e) => problems.push(LoadProblem {
                    path,
                    error: e.to_string(),
                }),
            }
        }
        found.sort_by(|a, b| a.title.product_id.cmp(&b.title.product_id));
        (found, problems)
    }

    /// Recipes from several directories, later ones winning.
    ///
    /// That is what makes an edit an override rather than a fork: the packaged
    /// `titles/` is read-only in an AppImage or a system package, and a person
    /// who fixed a launch must not lose it to the next upgrade. A directory
    /// that does not exist is not an error -- most machines have no edits.
    ///
    /// A file that does not parse is skipped and named on stderr, never fatal.
    /// One typo, a recipe written by a newer build, or a file cut short by a
    /// crash would otherwise take every other title with it, and an override
    /// that cannot be read leaves the packaged recipe for that title in place.
    /// [`Recipe::load_layered_report`] hands the problems to the caller
    /// instead. The `Result` stays so the signature does not change; it is
    /// never an error.
    pub fn load_layered(dirs: &[std::path::PathBuf]) -> anyhow::Result<Vec<Recipe>> {
        let (recipes, problems) = Self::load_layered_report(dirs);
        for problem in problems {
            eprintln!("-- skipped a recipe that could not be read: {problem}");
        }
        Ok(recipes)
    }

    /// [`Recipe::load_layered`], with the files that could not be used
    /// returned rather than printed.
    ///
    /// The order is packaged, then generated, then edited: a later directory
    /// wins, except that a recipe the launcher generated never replaces one
    /// that somebody wrote. A guess made from a package manifest before a
    /// tested recipe existed would otherwise hide that recipe -- its
    /// environment, its setup steps, its requirements -- for good.
    pub fn load_layered_report(dirs: &[std::path::PathBuf]) -> (Vec<Recipe>, Vec<LoadProblem>) {
        let mut by_product: BTreeMap<String, Recipe> = BTreeMap::new();
        let mut problems = Vec::new();
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            let (recipes, mut found) = Recipe::load_dir_report(dir);
            problems.append(&mut found);
            for mut recipe in recipes {
                let key = recipe.title.product_id.to_ascii_uppercase();
                match by_product.get_mut(&key) {
                    Some(existing) if recipe.generated && !existing.generated => {
                        existing.superseded_prefix = Some(recipe.prefix_dir());
                    }
                    Some(existing) => {
                        if existing.generated && !recipe.generated {
                            recipe.superseded_prefix = Some(existing.prefix_dir());
                        }
                        *existing = recipe;
                    }
                    None => {
                        by_product.insert(key, recipe);
                    }
                }
            }
        }
        for recipe in by_product.values_mut() {
            if recipe.superseded_prefix.as_deref() == Some(recipe.prefix_dir().as_str()) {
                recipe.superseded_prefix = None;
            }
        }
        (by_product.into_values().collect(), problems)
    }

    /// A recipe with nothing filled in but the identity, for a title nobody has
    /// described yet.
    ///
    /// It is deliberately `untested` and deliberately has an empty executable:
    /// those are the two things the person adopting it has to supply, and a
    /// plausible-looking guess would be worse than a blank, because it would be
    /// reported as the launcher's failure rather than as an unfinished recipe.
    pub fn blank(product_id: &str, name: &str) -> Recipe {
        let slug = slugify(name, product_id);
        Recipe {
            schema: SCHEMA,
            generated: false,
            title: Title {
                product_id: product_id.to_ascii_uppercase(),
                name: name.to_string(),
                slug,
                publisher: None,
                package_identity: None,
                title_id: None,
                install_size_gb: None,
            },
            install: Install::default(),
            launch: Launch {
                executable: String::new(),
                env: Default::default(),
            },
            runtime: Runtime::default(),
            status: Status {
                state: TitleState::Untested,
                blocked_by: None,
                summary: "Nobody has run this yet".to_string(),
                stops_at: None,
                last_verified: None,
                verified_with: None,
            },
            issues: Vec::new(),
            setup: Vec::new(),
            saves: Vec::new(),
            superseded_prefix: None,
        }
    }

    /// The recipe as a file, ready to write.
    pub fn to_toml(&self) -> anyhow::Result<String> {
        let body = toml::to_string_pretty(self)?;
        // Concatenated rather than written as an indented literal: the source
        // indentation ends up in the file, and every recipe this wrote had
        // thirteen spaces before each comment line and before `schema = 1`.
        Ok(format!(
            "{}{}{}{}{}{body}",
            "# Written by the ferestre launcher. Edits here win over the packaged\n",
            "# recipe of the same product id, and survive an upgrade.\n",
            "#\n",
            "# If this makes a title work, the most useful thing you can do with\n",
            "# it is open an issue with this file attached.\n",
        ))
    }

    /// Write this recipe into a directory, named by product id, replacing any
    /// recipe already there.
    ///
    /// Written to a temporary file and renamed into place, so a crash leaves
    /// the previous recipe or the new one and never half of either.
    pub fn save_to(&self, dir: &Path) -> anyhow::Result<PathBuf> {
        let path = self.file_in(dir)?;
        crate::cache::write(&path, self.to_toml()?.as_bytes())?;
        Ok(path)
    }

    /// Write this recipe into a directory only if no recipe for the product
    /// is there yet; `Ok(None)` when one is.
    ///
    /// For recipes the launcher writes by itself. Whatever is already there
    /// was either written by a person or by an earlier detection, and in
    /// neither case is a fresh guess a reason to replace it.
    pub fn save_new_to(&self, dir: &Path) -> anyhow::Result<Option<PathBuf>> {
        let path = self.file_in(dir)?;
        let temporary = dir.join(format!(
            ".{}.{}.new",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        crate::cache::write(&temporary, self.to_toml()?.as_bytes())?;
        // A hard link fails when the name is taken, which makes "only if
        // absent" one step rather than a check and a write with a gap between.
        let linked = std::fs::hard_link(&temporary, &path);
        let _ = std::fs::remove_file(&temporary);
        match linked {
            Ok(()) => Ok(Some(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(e) => Err(anyhow::anyhow!("{}: {e}", path.display())),
        }
    }

    fn file_in(&self, dir: &Path) -> anyhow::Result<PathBuf> {
        if self.title.product_id.is_empty()
            || !self
                .title
                .product_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            anyhow::bail!("not a usable product id: {:?}", self.title.product_id);
        }
        std::fs::create_dir_all(dir)?;
        Ok(dir.join(format!(
            "{}.toml",
            self.title.product_id.to_ascii_uppercase()
        )))
    }

    /// Match a recipe by product id or slug, case-insensitively, because people
    /// will type either and the product id is not memorable.
    pub fn matches(&self, needle: &str) -> bool {
        self.title.product_id.eq_ignore_ascii_case(needle)
            || self.title.slug.eq_ignore_ascii_case(needle)
    }

    /// Where the package lives, relative to the games directory.
    pub fn install_dir(&self) -> &str {
        self.install.dir.as_deref().unwrap_or(&self.title.slug)
    }

    /// The Wine prefix directory name, relative to the games directory.
    pub fn prefix_dir(&self) -> String {
        self.install
            .prefix
            .clone()
            .unwrap_or_else(|| format!("{}-proton", self.title.slug))
    }

    pub fn required(&self) -> BTreeSet<String> {
        self.runtime.requires.iter().cloned().collect()
    }

    pub fn wanted(&self) -> BTreeSet<String> {
        self.runtime.wants.iter().cloned().collect()
    }
}

/// A directory name for a title: lowercase, words joined by dashes, nothing
/// that needs quoting in a shell or a path.
///
/// Falls back to the product id, because a name can be entirely non-ASCII and
/// an empty slug would put the install at the root of the games directory.
fn slugify(name: &str, product_id: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.extend(c.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if slug.is_empty() {
        product_id.to_ascii_lowercase()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_recipe_is_honest_about_being_blank() {
        let recipe = Recipe::blank("9zztestnew1", "Clair Obscur: Expedition 33");
        assert_eq!(
            recipe.title.product_id, "9ZZTESTNEW1",
            "ids are stored uppercase"
        );
        assert_eq!(recipe.title.slug, "clair-obscur-expedition-33");
        assert_eq!(recipe.status.state, TitleState::Untested);
        assert!(
            recipe.launch.executable.is_empty(),
            "a guessed executable would be reported as our failure, not an unfinished recipe"
        );
    }

    /// A name can be entirely non-ASCII, and an empty slug would install at the
    /// root of the games directory.
    #[test]
    fn a_name_with_nothing_sluggable_falls_back_to_the_id() {
        assert_eq!(Recipe::blank("9ZZTESTNEW1", "").title.slug, "9zztestnew1");
        assert_eq!(
            Recipe::blank("9ZZTESTNEW1", "  ---  ").title.slug,
            "9zztestnew1"
        );
        assert_eq!(
            Recipe::blank("9ZZTESTNEW1", "Halo: CE").title.slug,
            "halo-ce"
        );
    }

    #[test]
    fn documented_environment_alias_matches_the_existing_env_key() {
        for key in ["env", "environment"] {
            let mut text = Recipe::blank("9ZZTESTNEW1", "Test Title")
                .to_toml()
                .unwrap();
            text = text.replace(
                "executable = \"\"",
                &format!("executable = \"Game.exe\"\n{key} = {{ TEST_VALUE = \"enabled\" }}"),
            );
            let parsed = Recipe::parse(&text).unwrap();
            assert_eq!(
                parsed.launch.env.get("TEST_VALUE").map(String::as_str),
                Some("enabled")
            );
        }
    }

    #[test]
    fn a_recipe_survives_being_written_and_read_back() {
        let mut recipe = Recipe::blank("9ZZTESTNEW1", "Test Title");
        recipe.launch.executable = "Game/Test.exe".into();
        recipe.launch.env.insert("FERESTRE_TEST".into(), "1".into());
        recipe
            .runtime
            .requires
            .push("loader.memfd-main-image".into());

        let text = recipe.to_toml().expect("serialises");
        let parsed = Recipe::parse(&text).expect("and parses back");
        assert_eq!(parsed.title.product_id, "9ZZTESTNEW1");
        assert_eq!(parsed.launch.executable, "Game/Test.exe");
        assert_eq!(
            parsed.launch.env.get("FERESTRE_TEST").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            parsed.runtime.requires,
            vec!["loader.memfd-main-image".to_string()]
        );
        // Every line is either a comment at column zero, a key at column zero,
        // or blank. The header used to be written as an indented literal, so
        // the source's own indentation ended up in the file.
        for line in text.lines() {
            assert!(
                !line.starts_with(' '),
                "a written recipe has no leading whitespace: {line:?}"
            );
        }
        assert!(
            text.contains("open an issue"),
            "the file says what to do with it"
        );
    }

    /// An edit is an override, not a fork: the packaged recipe stays, and the
    /// user's copy of the same product id wins.
    #[test]
    fn a_later_directory_wins_and_a_missing_one_is_not_an_error() {
        let root = std::env::temp_dir().join(format!("ferestre-layer-test-{}", std::process::id()));
        let packaged = root.join("packaged");
        let user = root.join("user");
        let _ = std::fs::remove_dir_all(&root);

        let mut shipped = Recipe::blank("9ZZTESTNEW1", "Test Title");
        shipped.launch.executable = "shipped.exe".into();
        shipped.save_to(&packaged).expect("writes");
        let other = Recipe::blank("9ZZTESTNEW2", "Other Title");
        other.save_to(&packaged).expect("writes");

        let mut edited = Recipe::blank("9ZZTESTNEW1", "Test Title");
        edited.launch.executable = "edited.exe".into();
        edited.save_to(&user).expect("writes");

        let layered =
            Recipe::load_layered(&[packaged.clone(), root.join("does-not-exist"), user.clone()])
                .expect("loads");
        assert_eq!(layered.len(), 2, "an override replaces, it does not add");
        assert_eq!(layered[0].launch.executable, "edited.exe");
        assert_eq!(layered[1].title.product_id, "9ZZTESTNEW2");

        std::fs::remove_dir_all(&root).expect("cleans up");
    }

    fn scratch(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("ferestre-recipe-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    /// One unreadable file used to make every title disappear. Now it is
    /// skipped and named, and the packaged recipe it would have overridden
    /// stays.
    #[test]
    fn one_bad_file_does_not_take_the_other_recipes_with_it() {
        let root = scratch("lenient");
        let packaged = root.join("packaged");
        let user = root.join("user");

        let mut shipped = Recipe::blank("9ZZTESTNEW1", "Test Title");
        shipped.launch.executable = "shipped.exe".into();
        shipped.save_to(&packaged).expect("writes");
        Recipe::blank("9ZZTESTNEW2", "Other Title")
            .save_to(&packaged)
            .expect("writes");

        std::fs::create_dir_all(&user).unwrap();
        // An override cut short, a typo, and a recipe from a newer schema.
        std::fs::write(
            user.join("9ZZTESTNEW1.toml"),
            "schema = 1\n[title]\nproduct-",
        )
        .unwrap();
        std::fs::write(user.join("typo.toml"), MINIMAL.replace("slug =", "slugg =")).unwrap();
        std::fs::write(
            user.join("future.toml"),
            MINIMAL.replace("schema = 1", "schema = 2"),
        )
        .unwrap();

        let (layered, problems) = Recipe::load_layered_report(&[packaged.clone(), user.clone()]);
        assert_eq!(layered.len(), 2, "{layered:?}");
        assert_eq!(layered[0].launch.executable, "shipped.exe");
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems
            .iter()
            .any(|p| p.path == user.join("9ZZTESTNEW1.toml")));
        assert!(problems
            .iter()
            .any(|p| p.path == user.join("future.toml") && p.error.contains('2')));

        // The plain call keeps its signature and does not fail either.
        assert_eq!(
            Recipe::load_layered(&[packaged, user.clone()])
                .expect("never an error")
                .len(),
            2
        );
        // The strict reader is still strict, for CI.
        assert!(Recipe::load_dir(&user).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A recipe guessed from a package manifest ranks below a tested one, and
    /// an edited one above it.
    #[test]
    fn a_generated_recipe_never_hides_a_packaged_one() {
        let root = scratch("generated");
        let packaged = root.join("packaged");
        let user = root.join("user");

        let mut shipped = Recipe::blank("9ZZTESTNEW1", "Test Title");
        shipped.launch.executable = "shipped.exe".into();
        shipped.save_to(&packaged).expect("writes");

        let mut guessed = Recipe::blank("9ZZTESTNEW1", "9ZZTESTNEW1");
        guessed.generated = true;
        guessed.launch.executable = "guessed.exe".into();
        guessed.save_to(&user).expect("writes");
        // A title only the launcher has seen keeps its generated recipe.
        let mut lone = Recipe::blank("9ZZTESTNEW3", "Lone");
        lone.generated = true;
        lone.save_to(&user).expect("writes");

        let dirs = [packaged.clone(), user.clone()];
        let layered = Recipe::load_layered(&dirs).unwrap();
        assert_eq!(layered.len(), 2);
        assert_eq!(layered[0].launch.executable, "shipped.exe");
        assert!(!layered[0].generated);
        // The generated recipe's prefix is remembered, because it holds the
        // saves and its slug was not the shipped one's.
        assert_eq!(
            layered[0].superseded_prefix.as_deref(),
            Some("9zztestnew1-proton")
        );
        assert!(layered[1].generated);

        // Saved from the editor, it is the person's own and wins.
        guessed.generated = false;
        guessed.launch.executable = "edited.exe".into();
        guessed.save_to(&user).expect("writes");
        let layered = Recipe::load_layered(&dirs).unwrap();
        assert_eq!(layered[0].launch.executable, "edited.exe");
        assert_eq!(layered[0].superseded_prefix, None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_generated_mark_survives_a_round_trip_and_is_absent_otherwise() {
        let mut recipe = Recipe::blank("9ZZTESTNEW1", "Test Title");
        recipe.launch.executable = "Game.exe".into();
        let text = recipe.to_toml().unwrap();
        assert!(!text.contains("generated"), "{text}");
        assert!(!Recipe::parse(&text).unwrap().generated);
        recipe.generated = true;
        let text = recipe.to_toml().unwrap();
        assert!(Recipe::parse(&text).unwrap().generated, "{text}");
    }

    #[test]
    fn writing_a_recipe_replaces_it_whole_and_a_detection_never_replaces_one() {
        let root = scratch("save");
        let mut recipe = Recipe::blank("9ZZTESTNEW1", "Test Title");
        recipe.launch.executable = "first.exe".into();
        let path = recipe.save_to(&root).expect("writes");
        recipe.launch.executable = "second.exe".into();
        assert_eq!(recipe.save_to(&root).unwrap(), path);
        assert_eq!(Recipe::load(&path).unwrap().launch.executable, "second.exe");

        // Someone's own recipe is there: a fresh guess leaves it alone.
        let mut guess = Recipe::blank("9ZZTESTNEW1", "Guess");
        guess.generated = true;
        assert_eq!(guess.save_new_to(&root).unwrap(), None);
        assert_eq!(Recipe::load(&path).unwrap().launch.executable, "second.exe");
        // A title with nothing yet gets it.
        let mut other = Recipe::blank("9ZZTESTNEW2", "Other");
        other.generated = true;
        let written = other.save_new_to(&root).unwrap().expect("written");
        assert!(Recipe::load(&written).unwrap().generated);
        // Nothing but the two recipes is left behind.
        let mut names: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["9ZZTESTNEW1.toml", "9ZZTESTNEW2.toml"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_slug_that_would_leave_the_games_directory_is_refused() {
        for slug in [
            "../../.config/autostart/x",
            "a/b",
            "Upper",
            "",
            "-dash",
            "a b",
        ] {
            let text = MINIMAL.replace("slug = \"example\"", &format!("slug = {slug:?}"));
            let err = Recipe::parse(&text).expect_err(slug).to_string();
            assert!(err.contains("slug"), "{err}");
        }
        assert!(Recipe::parse(&MINIMAL.replace("\"example\"", "\"9x-2\"")).is_ok());
    }

    #[test]
    fn only_a_titles_own_directory_variables_may_be_named() {
        for (slug, dir, prefix) in [
            ("bedrock", "BEDROCK_DIR", "BEDROCK_PREFIX"),
            ("fh5", "FH5_DIR", "FH5_PREFIX"),
            ("exp33", "EXP33_DIR", "EXP33_PREFIX"),
            ("halo-infinite", "HALO_INFINITE_DIR", "HALO_INFINITE_PREFIX"),
        ] {
            assert!(is_location_variable(dir, slug, "_DIR"), "{dir}");
            assert!(is_location_variable(prefix, slug, "_PREFIX"), "{prefix}");
            // Each key takes its own one, not the other's.
            assert!(!is_location_variable(prefix, slug, "_DIR"), "{prefix}");
            assert!(!is_location_variable(dir, slug, "_PREFIX"), "{dir}");
            let text = MINIMAL.replace("slug = \"example\"", &format!("slug = {slug:?}"));
            let text = format!("{text}\n[install]\ndir-env = {dir:?}\nprefix-env = {prefix:?}\n");
            assert!(Recipe::parse(&text).is_ok(), "{slug}");
        }
        for name in [
            "XODUS_GAMES_DIR",
            "XDG_RUNTIME_DIR",
            "XDG_STATE_HOME",
            "HOME",
            "PWD",
            "TMPDIR",
            "FERESTRE_SCRIPTS_DIR",
            "STEAM_DIR",
            "PROTON_DIR",
            "WINEPREFIX",
            "_DIR",
            "lower_dir",
            "A-B_DIR",
            // Shaped like a title's variable, but somebody else's.
            "CARGO_TARGET_DIR",
            "PULSE_RUNTIME_DIR",
            "PIPEWIRE_RUNTIME_DIR",
            "BEDROCK_DIR",
            "EXAMPLE_PREFIX",
        ] {
            for (key, suffix) in [("dir-env", "_DIR"), ("prefix-env", "_PREFIX")] {
                if name == location_variable("example", suffix) {
                    continue;
                }
                assert!(!is_location_variable(name, "example", suffix), "{name}");
                let text = format!("{MINIMAL}\n[install]\n{key} = {name:?}\n");
                let err = Recipe::parse(&text).expect_err(name).to_string();
                assert!(err.contains(&location_variable("example", suffix)), "{err}");
            }
        }
        // A slug that spells one of the launcher's or the system's own
        // variables does not make it the title's.
        for slug in ["xdg-runtime", "xodus-games", "steam", "proton"] {
            let name = location_variable(slug, "_DIR");
            assert!(!is_location_variable(&name, slug, "_DIR"), "{name}");
        }
    }

    /// Every key SCHEMA.md documents has to be one this parser takes, or a
    /// recipe written from the documentation passes the validator and then
    /// cannot be loaded. `[launch].arguments` was exactly that.
    #[test]
    fn every_key_the_schema_documents_is_accepted() {
        let schema = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles/SCHEMA.md");
        let Ok(text) = std::fs::read_to_string(&schema) else {
            return; // packaged crate without the repository around it
        };
        let mut table = String::new();
        let mut checked = 0;
        for line in text.lines() {
            if let Some(heading) = line.strip_prefix("### `") {
                table = heading.split('`').next().unwrap_or_default().to_string();
                continue;
            }
            if line.starts_with("## ") {
                table.clear();
            }
            if table.is_empty() || !line.starts_with("| `") {
                continue;
            }
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let kind = cells.get(2).copied().unwrap_or_default();
            // `env` (alias `environment`): every backticked name in the key
            // column is a key.
            let keys: Vec<&str> = cells[1]
                .split('`')
                .enumerate()
                .filter(|(i, _)| i % 2 == 1)
                .map(|(_, k)| k)
                .collect();
            for key in keys {
                let value = match kind {
                    "string" => "\"x\"",
                    "number" => "1.5",
                    "date" => "2026-01-01",
                    "table" => "{ A_B = \"1\" }",
                    k if k.starts_with("array") => "[]",
                    other => panic!("{table}.{key}: undocumented type {other:?}"),
                };
                let value = match (table.as_str(), key) {
                    ("[title]", "slug") => "\"example\"",
                    ("[install]", "dir-env") => "\"EXAMPLE_DIR\"",
                    ("[install]", "prefix-env") => "\"EXAMPLE_PREFIX\"",
                    ("[status]", "state") => "\"broken\"",
                    _ => value,
                };
                let recipe = with_key(&table, key, value);
                if let Err(e) = Recipe::parse(&recipe) {
                    panic!("{table} {key} is documented but refused: {e}\n{recipe}");
                }
                checked += 1;
            }
        }
        assert!(checked > 30, "only {checked} documented keys were found");
    }

    /// The minimal recipe with one key set, in whichever table it belongs to.
    fn with_key(table: &str, key: &str, value: &str) -> String {
        let mut tables: Vec<(String, Vec<String>)> = vec![
            ("".into(), vec!["schema = 1".into()]),
            (
                "[title]".into(),
                vec![
                    "product-id = \"9NBLGGH2JHXJ\"".into(),
                    "name = \"Example\"".into(),
                    "slug = \"example\"".into(),
                ],
            ),
            ("[launch]".into(), vec!["executable = 'Example.exe'".into()]),
            (
                "[status]".into(),
                vec!["state = \"playable\"".into(), "summary = \"Runs.\"".into()],
            ),
        ];
        let required = [
            ("[[setup]]", vec!["action = \"substitute-xcurl\""]),
            ("[[saves]]", vec!["kind = \"wgs\""]),
            ("[[issues]]", vec!["symptom = \"s\"", "cause = \"c\""]),
        ];
        let line = format!("{key} = {value}");
        match tables.iter_mut().find(|(name, _)| name == table) {
            Some((_, lines)) => {
                lines.retain(|l| !l.starts_with(&format!("{key} =")));
                lines.push(line);
            }
            None => {
                let mut lines: Vec<String> = required
                    .iter()
                    .find(|(name, _)| *name == table)
                    .map(|(_, l)| l.iter().map(|s| s.to_string()).collect())
                    .unwrap_or_default();
                lines.retain(|l| !l.starts_with(&format!("{key} =")));
                lines.push(line);
                tables.push((table.to_string(), lines));
            }
        }
        tables
            .into_iter()
            .map(|(name, lines)| format!("{name}\n{}\n", lines.join("\n")))
            .collect()
    }

    #[test]
    fn a_product_id_that_is_not_one_is_never_written() {
        let mut recipe = Recipe::blank("9ZZTESTNEW1", "Test Title");
        recipe.title.product_id = "../../etc/passwd".into();
        assert!(recipe.save_to(Path::new("/tmp")).is_err());
    }

    const MINIMAL: &str = r#"
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
"#;

    #[test]
    fn parses_a_minimal_recipe() {
        let r = Recipe::parse(MINIMAL).expect("should parse");
        assert_eq!(r.title.product_id, "9NBLGGH2JHXJ");
        assert_eq!(r.status.state, TitleState::Playable);
        assert!(r.issues.is_empty());
    }

    #[test]
    fn install_paths_default_to_the_slug() {
        let r = Recipe::parse(MINIMAL).unwrap();
        assert_eq!(r.install_dir(), "example");
        assert_eq!(r.prefix_dir(), "example-proton");
    }

    #[test]
    fn install_paths_can_be_overridden() {
        // Bedrock keeps the package one level down, which is why this exists.
        let text = MINIMAL.to_string()
            + "\n[install]\ndir = \"bedrock/game\"\nprefix = \"bedrock-proton\"\n";
        let r = Recipe::parse(&text).unwrap();
        assert_eq!(r.install_dir(), "bedrock/game");
        assert_eq!(r.prefix_dir(), "bedrock-proton");
    }

    #[test]
    fn matches_by_product_id_or_slug_ignoring_case() {
        let r = Recipe::parse(MINIMAL).unwrap();
        assert!(r.matches("9NBLGGH2JHXJ"));
        assert!(r.matches("9nblggh2jhxj"));
        assert!(r.matches("EXAMPLE"));
        assert!(!r.matches("something-else"));
    }

    #[test]
    fn a_future_schema_is_refused_rather_than_half_read() {
        let text = MINIMAL.replace("schema = 1", "schema = 99");
        let err = Recipe::parse(&text).unwrap_err().to_string();
        assert!(err.contains("99"), "error should name the version: {err}");
    }

    #[test]
    fn an_unknown_field_is_an_error_not_a_silent_drop() {
        // A typo in a recipe should fail the validator, not quietly do nothing.
        let text = MINIMAL.replace("slug = \"example\"", "slug = \"example\"\nslugg = \"typo\"");
        assert!(Recipe::parse(&text).is_err());
    }

    #[test]
    fn state_decides_whether_launching_is_worth_trying() {
        assert!(TitleState::Playable.is_runnable());
        assert!(TitleState::PlayableWithIssues.is_runnable());
        assert!(!TitleState::Broken.is_runnable());
        assert!(!TitleState::Untested.is_runnable());
    }

    #[test]
    fn every_recipe_in_the_repository_parses() {
        // The real recipes are the contract; if the schema and the data drift
        // apart this catches it in CI rather than at someone's first launch.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles");
        if !dir.exists() {
            return; // packaged crate without the repository around it
        }
        let recipes = Recipe::load_dir(&dir).expect("repository recipes should parse");
        assert!(!recipes.is_empty(), "expected at least one recipe");
        for r in &recipes {
            assert!(!r.title.name.is_empty());
            assert!(!r.launch.executable.is_empty());
        }
    }
}

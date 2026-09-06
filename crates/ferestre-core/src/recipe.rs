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
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
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
        Ok(recipe)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    }

    /// Every recipe in a directory, sorted by product id so output is stable.
    /// `capabilities.toml` is the runtime registry, not a title.
    pub fn load_dir(dir: &Path) -> anyhow::Result<Vec<Recipe>> {
        let mut found = Vec::new();
        for entry in
            std::fs::read_dir(dir).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?
        {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if path.file_stem().and_then(|s| s.to_str()) == Some("capabilities") {
                continue;
            }
            found.push(Recipe::load(&path)?);
        }
        found.sort_by(|a, b| a.title.product_id.cmp(&b.title.product_id));
        Ok(found)
    }

    /// Recipes from several directories, later ones winning.
    ///
    /// That is what makes an edit an override rather than a fork: the packaged
    /// `titles/` is read-only in an AppImage or a system package, and a person
    /// who fixed a launch must not lose it to the next upgrade. A directory
    /// that does not exist is not an error -- most machines have no edits.
    pub fn load_layered(dirs: &[std::path::PathBuf]) -> anyhow::Result<Vec<Recipe>> {
        let mut by_product: BTreeMap<String, Recipe> = BTreeMap::new();
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            for recipe in Recipe::load_dir(dir)? {
                by_product.insert(recipe.title.product_id.to_ascii_uppercase(), recipe);
            }
        }
        Ok(by_product.into_values().collect())
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

    /// Write this recipe into a directory, named by product id.
    pub fn save_to(&self, dir: &Path) -> anyhow::Result<PathBuf> {
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
        let path = dir.join(format!(
            "{}.toml",
            self.title.product_id.to_ascii_uppercase()
        ));
        std::fs::write(&path, self.to_toml()?)?;
        Ok(path)
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

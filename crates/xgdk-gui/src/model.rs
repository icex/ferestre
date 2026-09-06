//! What the window shows, worked out without a toolkit.
//!
//! Every decision the GUI makes lives here: which button a row gets, what its
//! subtitle says, why a title cannot be launched. None of it touches GTK, so it
//! is tested the way the CLI's decisions are -- with fixtures, on a machine
//! with no display and no runtime installed. `main.rs` is then only wiring:
//! turn a [`TitleView`] into an `AdwActionRow`, turn a click into a command.
//!
//! The rules themselves come from `xgdk-core`, the same code the CLI runs, so
//! the window and the terminal cannot disagree about what a recipe says.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use xgdk_core::library::Entry;
use xgdk_core::recipe::{Recipe, TitleState};
use xgdk_core::runtime::{self, InstalledRuntime, Registry};

/// What the row's button does, and when it does nothing, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Installed and nothing is in the way: `xgdk run <product>`.
    Play,
    /// Owned, with a recipe, but not on disk yet: `xgdk install <product>`.
    Install,
    /// Nothing to press. The string is shown as the tooltip and the subtitle,
    /// so it has to read as a sentence to someone who has never seen the CLI.
    Blocked(String),
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::Play => "Play",
            Action::Install => "Install",
            Action::Blocked(_) => "Play",
        }
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Action::Blocked(_))
    }

    /// The `xgdk` arguments this action runs, or `None` when it runs nothing.
    pub fn command<'a>(&self, product_id: &'a str) -> Option<[&'a str; 2]> {
        match self {
            Action::Play => Some(["run", product_id]),
            Action::Install => Some(["install", product_id]),
            Action::Blocked(_) => None,
        }
    }
}

/// One title, ready to be drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleView {
    pub product_id: String,
    pub name: String,
    pub state: TitleState,
    /// Badge text and the libadwaita CSS class that colours it.
    pub badge: &'static str,
    pub badge_css: &'static str,
    pub subtitle: String,
    pub action: Action,
}

/// What is known about the account's library.
///
/// `Unknown` is the startup state and is not a failure: the window has to be
/// useful before anyone signs in, so an unasked library never blocks a launch.
/// It only stops the window claiming a title is *not* owned on no evidence.
#[derive(Debug, Clone, Default)]
pub enum Ownership {
    #[default]
    Unknown,
    Known(Vec<Entry>),
}

impl Ownership {
    fn owns(&self, product_id: &str) -> Option<bool> {
        match self {
            Ownership::Unknown => None,
            Ownership::Known(entries) => Some(
                entries
                    .iter()
                    .any(|e| e.is_title() && e.product_id.eq_ignore_ascii_case(product_id)),
            ),
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, Ownership::Known(_))
    }

    /// Owned titles with no recipe: the list that says what to work on next,
    /// and the reason someone would file an issue.
    pub fn without_recipe(&self, recipes: &[Recipe]) -> Vec<String> {
        let known: BTreeSet<String> = recipes
            .iter()
            .map(|r| r.title.product_id.to_ascii_uppercase())
            .collect();
        match self {
            Ownership::Unknown => Vec::new(),
            Ownership::Known(entries) => {
                let mut out: BTreeSet<String> = BTreeSet::new();
                for entry in entries.iter().filter(|e| e.is_title()) {
                    if !known.contains(&entry.product_id.to_ascii_uppercase()) {
                        out.insert(entry.product_id.clone());
                    }
                }
                out.into_iter().collect()
            }
        }
    }
}

pub fn state_label(state: TitleState) -> (&'static str, &'static str) {
    match state {
        TitleState::Playable => ("Playable", "success"),
        TitleState::PlayableWithIssues => ("Issues", "warning"),
        TitleState::Broken => ("Does not run", "error"),
        TitleState::Untested => ("Untested", "dim-label"),
    }
}

/// Everything a row needs that the pure code cannot find out for itself.
pub struct Inputs<'a> {
    pub recipes: &'a [Recipe],
    pub runtime: Option<&'a InstalledRuntime>,
    /// `titles/capabilities.toml`, when it was found. Without it a refusal
    /// still names the missing capabilities; with it, it also names the symptom.
    pub registry: Option<&'a Registry>,
    pub ownership: &'a Ownership,
    /// Whether the title's install directory is on disk. A closure because the
    /// answer is a filesystem probe, and keeping it out of here is what lets a
    /// test describe a half-installed machine in one line.
    pub installed: &'a dyn Fn(&Recipe) -> bool,
}

/// Work out the button and the words for every recipe.
///
/// Order matters: the first thing that stops a launch is the thing to say. A
/// broken title stays broken whether or not the runtime is installed, and
/// telling someone to install a runtime for a title that will not run either
/// way wastes their evening.
pub fn rows(inputs: &Inputs) -> Vec<TitleView> {
    inputs.recipes.iter().map(|r| row(inputs, r)).collect()
}

fn row(inputs: &Inputs, recipe: &Recipe) -> TitleView {
    let (badge, badge_css) = state_label(recipe.status.state);
    let (action, caution) = action_for(inputs, recipe);
    let subtitle = match (&action, caution) {
        (Action::Blocked(reason), _) => reason.clone(),
        (_, Some(caution)) => caution,
        // Not the state text: the badge already carries that, and the summary
        // is the line someone actually wants before pressing the button.
        _ => recipe.status.summary.clone(),
    };
    TitleView {
        product_id: recipe.title.product_id.clone(),
        name: recipe.title.name.clone(),
        state: recipe.status.state,
        badge,
        badge_css,
        subtitle,
        action,
    }
}

/// The action, and a line of doubt to show beside it when there is one.
fn action_for(inputs: &Inputs, recipe: &Recipe) -> (Action, Option<String>) {
    let blocked = |why: String| (Action::Blocked(why), None);

    // `Untested` is deliberately not blocked. The whole point of shipping the
    // catalog is that people try titles nobody has tried and say what happened;
    // a launcher that refuses to attempt them cannot collect that.
    if recipe.status.state == TitleState::Broken {
        let why = recipe
            .status
            .blocked_by
            .as_deref()
            .map(|b| format!("does not run here: {b}"))
            .unwrap_or_else(|| format!("does not run here: {}", recipe.status.summary));
        return blocked(why);
    }

    if inputs.ownership.owns(&recipe.title.product_id) == Some(false) {
        return blocked("this account does not own it".into());
    }

    let Some(runtime) = inputs.runtime else {
        return blocked("install the patched runtime first".into());
    };

    let mut caution = None;
    let assessment = runtime::assess(recipe, runtime, inputs.registry);
    if !assessment.is_satisfied() {
        // `assess` writes a paragraph for a terminal. A row gets one line.
        let line = first_line(&assessment.explanation).to_string();
        // A capability list a runtime declares is authoritative, so a miss
        // against it is a refusal. A list we had to guess at by looking for
        // symbols is not: most of what a build provides is invisible to a
        // probe, so a miss there means "we could not tell", and greying out
        // the button on that would stop people trying titles that work.
        if assessment.source.is_published() {
            return blocked(line);
        }
        caution = Some(line);
    }

    let action = if (inputs.installed)(recipe) {
        Action::Play
    } else {
        Action::Install
    };
    (action, caution)
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// How to describe the runtime at the top of the window.
pub fn runtime_summary(runtime: Option<&InstalledRuntime>) -> (String, String) {
    match runtime {
        Some(rt) => (
            rt.label(),
            format!(
                "{} {} · {}",
                rt.provides.len(),
                if rt.provides.len() == 1 {
                    "capability"
                } else {
                    "capabilities"
                },
                if rt.source.is_published() {
                    "declared by the build"
                } else {
                    "probed, so the list may be incomplete"
                }
            ),
        ),
        None => (
            "No patched runtime installed".into(),
            "Nothing can launch until it is built and installed".into(),
        ),
    }
}

/// Find the `xgdk` binary. Next to this one first, so an AppImage or a package
/// runs its own CLI rather than whichever one happens to be on `PATH`.
pub fn cli_binary(exe_dir: Option<&Path>, is_file: &dyn Fn(&Path) -> bool) -> PathBuf {
    if let Some(dir) = exe_dir {
        let sibling = dir.join("xgdk");
        if is_file(&sibling) {
            return sibling;
        }
    }
    PathBuf::from("xgdk")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use xgdk_core::runtime::CapabilitySource;

    fn recipe(product: &str, state: &str, requires: &[&str]) -> Recipe {
        let requires = requires
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        Recipe::parse(&format!(
            r#"
            schema = 1
            [title]
            product-id = "{product}"
            name = "Test Title"
            slug = "test-title"
            [launch]
            executable = "Game.exe"
            [runtime]
            requires = [{requires}]
            [status]
            state = "{state}"
            summary = "a summary"
            "#
        ))
        .expect("fixture recipe parses")
    }

    fn broken_recipe() -> Recipe {
        Recipe::parse(
            r#"
            schema = 1
            [title]
            product-id = "9ZZTESTBRK1"
            name = "Broken Title"
            slug = "broken-title"
            [launch]
            executable = "Game.exe"
            [status]
            state = "broken"
            summary = "crashes at the splash"
            blocked-by = "title-protection"
            "#,
        )
        .expect("fixture recipe parses")
    }

    fn runtime_with(caps: &[&str], source: CapabilitySource) -> InstalledRuntime {
        InstalledRuntime {
            path: PathBuf::from("/nonexistent/runtime"),
            version: Some("test".into()),
            provides: caps.iter().map(|c| c.to_string()).collect::<BTreeSet<_>>(),
            source,
        }
    }

    fn entries(products: &[&str]) -> Vec<Entry> {
        let items = products
            .iter()
            .map(|p| {
                format!(r#"{{"productId":"{p}","productType":"Game","status":"Active"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        xgdk_core::library::parse(&format!("[{items}]")).expect("fixture entries parse")
    }

    fn inputs<'a>(
        recipes: &'a [Recipe],
        runtime: Option<&'a InstalledRuntime>,
        ownership: &'a Ownership,
        installed: &'a dyn Fn(&Recipe) -> bool,
    ) -> Inputs<'a> {
        Inputs {
            recipes,
            runtime,
            registry: None,
            ownership,
            installed,
        }
    }

    const NOTHING_INSTALLED: &dyn Fn(&Recipe) -> bool = &|_| false;
    const ALL_INSTALLED: &dyn Fn(&Recipe) -> bool = &|_| true;

    #[test]
    fn a_playable_installed_title_plays() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play);
        assert_eq!(rows[0].action.command("9ZZTESTGAME1"), Some(["run", "9ZZTESTGAME1"]));
        assert_eq!(rows[0].subtitle, "a summary");
    }

    #[test]
    fn a_playable_title_that_is_not_on_disk_offers_to_install_it() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, Some(&rt), &own, NOTHING_INSTALLED));
        assert_eq!(rows[0].action, Action::Install);
        assert_eq!(
            rows[0].action.command("9ZZTESTGAME1"),
            Some(["install", "9ZZTESTGAME1"])
        );
    }

    /// The catalog exists so people try things nobody has tried. A launcher
    /// that refuses to attempt an untested title cannot collect that report.
    #[test]
    fn an_untested_title_is_still_launchable() {
        let recipes = vec![recipe("9ZZTESTGAME1", "untested", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play);
        assert_eq!(rows[0].badge, "Untested");
    }

    #[test]
    fn a_broken_title_says_what_blocks_it_and_offers_no_button() {
        let recipes = vec![broken_recipe()];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(
            rows[0].action,
            Action::Blocked("does not run here: title-protection".into())
        );
        assert!(!rows[0].action.is_enabled());
        assert_eq!(rows[0].action.command("9ZZTESTBRK1"), None);
        assert_eq!(rows[0].subtitle, "does not run here: title-protection");
    }

    /// A broken title is broken whether or not a runtime is installed. Telling
    /// someone to spend an hour building one first would be a lie.
    #[test]
    fn broken_beats_a_missing_runtime() {
        let recipes = vec![broken_recipe()];
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, None, &own, ALL_INSTALLED));
        assert!(matches!(&rows[0].action, Action::Blocked(w) if w.contains("title-protection")));
    }

    #[test]
    fn without_a_runtime_nothing_launches() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, None, &own, ALL_INSTALLED));
        assert_eq!(
            rows[0].action,
            Action::Blocked("install the patched runtime first".into())
        );
    }

    /// A list the build declares is authoritative, so a miss against it is a
    /// refusal.
    #[test]
    fn a_declared_runtime_missing_a_required_capability_blocks_with_one_line() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &["loader.memfd-main-image"])];
        let rt = runtime_with(&["appmodel.package-identity"], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        let Action::Blocked(why) = &rows[0].action else {
            panic!("expected a block, got {:?}", rows[0].action);
        };
        assert!(!why.contains('\n'), "a row gets one line, got {why:?}");
        assert!(why.contains("will not"), "{why}");
    }

    /// A probe sees almost nothing of what a build provides, so a miss there
    /// means "we could not tell". Greying the button out on that evidence would
    /// stop people trying titles that in fact work -- which is the whole point.
    #[test]
    fn a_probed_runtime_cautions_instead_of_refusing() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &["loader.memfd-main-image"])];
        let rt = runtime_with(&["appmodel.package-identity"], CapabilitySource::Probed);
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play);
        assert!(
            rows[0].subtitle.contains("could not be found"),
            "the doubt still has to be visible: {:?}",
            rows[0].subtitle
        );
        assert!(!rows[0].subtitle.contains('\n'));
    }

    /// A satisfied runtime shows the recipe's own summary, not a caution.
    #[test]
    fn a_satisfied_runtime_shows_the_recipe_summary() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &["loader.memfd-main-image"])];
        let rt = runtime_with(&["loader.memfd-main-image"], CapabilitySource::Probed);
        let own = Ownership::Unknown;
        let rows = rows(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play);
        assert_eq!(rows[0].subtitle, "a summary");
    }

    /// An unasked library must not read as "you do not own this".
    #[test]
    fn ownership_only_blocks_once_the_library_is_known() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);

        let unknown = Ownership::Unknown;
        assert_eq!(
            rows(&inputs(&recipes, Some(&rt), &unknown, ALL_INSTALLED))[0].action,
            Action::Play
        );

        let elsewhere = Ownership::Known(entries(&["9ZZTESTOTHR"]));
        assert_eq!(
            rows(&inputs(&recipes, Some(&rt), &elsewhere, ALL_INSTALLED))[0].action,
            Action::Blocked("this account does not own it".into())
        );

        let owned = Ownership::Known(entries(&["9zztestgame1"]));
        assert_eq!(
            rows(&inputs(&recipes, Some(&rt), &owned, ALL_INSTALLED))[0].action,
            Action::Play,
            "the join is case-insensitive, as it is in the CLI"
        );
    }

    #[test]
    fn owned_titles_with_no_recipe_are_listed_for_reporting() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let own = Ownership::Known(entries(&["9ZZTESTGAME1", "9ZZTESTNEW2", "9ZZTESTNEW1"]));
        assert_eq!(
            own.without_recipe(&recipes),
            vec!["9ZZTESTNEW1".to_string(), "9ZZTESTNEW2".to_string()],
            "sorted, and the one with a recipe is not in the list"
        );
        assert!(Ownership::Unknown.without_recipe(&recipes).is_empty());
    }

    #[test]
    fn a_probed_runtime_says_its_list_may_be_incomplete() {
        let probed = runtime_with(&["a"], CapabilitySource::Probed);
        let (_, subtitle) = runtime_summary(Some(&probed));
        assert!(subtitle.contains("incomplete"), "{subtitle}");
        assert!(subtitle.contains("1 capability"), "{subtitle}");

        let declared = runtime_with(&["a", "b"], CapabilitySource::Manifest);
        let (_, subtitle) = runtime_summary(Some(&declared));
        assert!(subtitle.contains("2 capabilities"), "{subtitle}");

        let (title, _) = runtime_summary(None);
        assert!(title.contains("No patched runtime"), "{title}");
    }

    #[test]
    fn the_cli_next_to_the_gui_wins_over_the_path() {
        let dir = Path::new("/opt/xgdk/bin");
        assert_eq!(
            cli_binary(Some(dir), &|p| p == Path::new("/opt/xgdk/bin/xgdk")),
            PathBuf::from("/opt/xgdk/bin/xgdk")
        );
        assert_eq!(cli_binary(Some(dir), &|_| false), PathBuf::from("xgdk"));
        assert_eq!(cli_binary(None, &|_| true), PathBuf::from("xgdk"));
    }
}

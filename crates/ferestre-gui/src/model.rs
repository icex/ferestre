//! What the window shows, worked out without a toolkit.
//!
//! Every decision the GUI makes lives here: which button a row gets, what its
//! subtitle says, why a title cannot be launched. None of it touches GTK, so it
//! is tested the way the CLI's decisions are -- with fixtures, on a machine
//! with no display and no runtime installed. `main.rs` is then only wiring:
//! turn a [`TitleView`] into an `AdwActionRow`, turn a click into a command.
//!
//! The rules themselves come from `ferestre-core`, the same code the CLI runs, so
//! the window and the terminal cannot disagree about what a recipe says.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ferestre_core::catalog::Product;
use ferestre_core::install::Record;
use ferestre_core::library::Entry;
use ferestre_core::recipe::{Recipe, TitleState};
use ferestre_core::runtime::{self, InstalledRuntime, Registry};

/// What the row's button does, and when it does nothing, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Installed and nothing is in the way: `ferestre run <product>`.
    Play,
    /// Owned, with a recipe, but not on disk yet: `ferestre install <product>`.
    Install,
    /// Installed, and the catalog is offering a build this one does not have.
    Update,
    /// Owned, but nobody has written a recipe. Opens the editor rather than
    /// running anything: the missing piece is a description, not a download.
    Adopt,
    /// Nothing to press. The string is shown as the tooltip and the subtitle,
    /// so it has to read as a sentence to someone who has never seen the CLI.
    Blocked(String),
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::Play => "Play",
            Action::Install => "Install",
            Action::Update => "Update",
            Action::Adopt => "Set up",
            Action::Blocked(_) => "Play",
        }
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Action::Blocked(_))
    }

    /// The `ferestre` arguments this action runs, or `None` when it runs nothing.
    /// `Adopt` runs nothing on purpose -- it opens the editor -- so this being
    /// `None` is not the same question as [`Action::is_enabled`].
    pub fn command<'a>(&self, product_id: &'a str) -> Option<[&'a str; 2]> {
        match self {
            Action::Play => Some(["run", product_id]),
            // Updating is a reinstall today. Fetching only what changed is a
            // separate piece of work and this is the honest version of it.
            Action::Install | Action::Update => Some(["install", product_id]),
            Action::Adopt | Action::Blocked(_) => None,
        }
    }
}

/// One title, ready to be drawn. Internal: [`LibraryRow`] is what the window
/// gets, and it is built from this plus what the catalog knows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TitleView {
    product_id: String,
    pub name: String,
    pub state: TitleState,
    /// Badge text and the libadwaita CSS class that colours it.
    pub badge: &'static str,
    pub badge_css: &'static str,
    pub subtitle: String,
    /// The runtime's doubt about this title, when it has one. Kept apart from
    /// the subtitle so a caller that has something else to say can say both --
    /// losing "this may not start" to make room for "up to date" would be an
    /// unfortunate trade.
    pub caution: Option<String>,
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
    /// Names, art and download sizes, keyed by uppercase product id. Empty
    /// until the catalog has been asked, which is not an error: a row falls
    /// back to its product id.
    pub catalog: &'a BTreeMap<String, Product>,
    /// What the launcher recorded at install time, keyed the same way.
    pub records: &'a BTreeMap<String, Record>,
    /// Whether the title's install directory is on disk. A closure because the
    /// answer is a filesystem probe, and keeping it out of here is what lets a
    /// test describe a half-installed machine in one line.
    pub installed: &'a dyn Fn(&Recipe) -> bool,
    /// The version in the package manifest on disk, when there is one. Read
    /// separately from the record because a title installed outside this
    /// launcher has a version and no record, and showing "Installed" when the
    /// tree plainly says 1.26.4501.0 is throwing away an answer we have.
    pub installed_version: &'a dyn Fn(&Recipe) -> Option<String>,
}

/// Work out the button and the words for one recipe.
///
/// Order matters: the first thing that stops a launch is the thing to say. A
/// broken title stays broken whether or not the runtime is installed, and
/// telling someone to install a runtime for a title that will not run either
/// way wastes their evening.
fn row(inputs: &Inputs, recipe: &Recipe) -> TitleView {
    let (badge, badge_css) = state_label(recipe.status.state);
    let (action, caution) = action_for(inputs, recipe);
    let subtitle = match (&action, &caution) {
        (Action::Blocked(reason), _) => reason.clone(),
        (_, Some(caution)) => caution.clone(),
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
        caution,
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

    // Absence from the library listing is a caution, not a refusal.
    //
    // The listing is not exhaustive, and that is not a guess: Clair Obscur:
    // Expedition 33 is installed, licensed and launches on this machine, and
    // does not appear in the 124 entitlements collections returns for the
    // account that owns it. Refusing to launch an installed title on that
    // evidence would be the launcher calling its owner a liar and being wrong.
    // A download for something genuinely unowned fails at the licence step
    // anyway, which teaches the same lesson at the right moment.
    let mut caution = match inputs.ownership.owns(&recipe.title.product_id) {
        Some(false) => Some("not in this account's library listing".to_string()),
        _ => None,
    };

    let Some(runtime) = inputs.runtime else {
        return blocked("install the patched runtime first".into());
    };

    let assessment = runtime::assess(recipe, runtime, inputs.registry);
    if !assessment.is_satisfied() {
        // `assess` writes a paragraph for a terminal. A row gets one line.
        let line = row_sized(&assessment.explanation, &recipe.title.name);
        // A capability list a runtime declares is authoritative, so a miss
        // against it is a refusal. A list we had to guess at by looking for
        // symbols is not: most of what a build provides is invisible to a
        // probe, so a miss there means "we could not tell", and greying out
        // the button on that would stop people trying titles that work.
        if assessment.source.is_published() {
            return blocked(line);
        }
        caution = Some(match caution {
            Some(first) => format!("{first}  ·  {line}"),
            None => line,
        });
    }

    let action = if (inputs.installed)(recipe) {
        Action::Play
    } else {
        Action::Install
    };
    (action, caution)
}

/// The first line of an explanation `assess` wrote for a terminal, cut down to
/// something a row can hold.
///
/// It opens with the title's name, which the row already shows two centimetres
/// to the left. Dropping it is most of the difference between a subtitle that
/// fits and one that ends in an ellipsis.
fn row_sized(explanation: &str, title: &str) -> String {
    let line = explanation.lines().next().unwrap_or("").trim();
    let trimmed = line
        .strip_prefix(title)
        .map(str::trim_start)
        .unwrap_or(line);
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// One row of the library: something owned, something described, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRow {
    pub product_id: String,
    /// The catalog's name when there is one, the product id when there is not.
    pub name: String,
    pub subtitle: String,
    /// Absent for a title with no recipe: there is no state to report about
    /// something nobody has described.
    pub badge: Option<(&'static str, &'static str)>,
    /// The art URL, for a caller that fetches and caches it.
    pub image: Option<String>,
    pub owned: bool,
    pub has_recipe: bool,
    pub installed: bool,
    /// `None` means "cannot tell", which is different from "up to date" -- see
    /// [`Record::update_available`].
    pub update: Option<bool>,
    pub action: Action,
}

impl LibraryRow {
    /// Whether the catalog has been asked about this row yet. A row still
    /// showing its product id is waiting, not broken.
    pub fn is_named(&self) -> bool {
        self.name != self.product_id
    }
}

/// Every title worth a row: everything owned, plus everything described.
///
/// The union rather than the intersection, deliberately. A recipe for something
/// this account does not own still belongs on screen -- it is how someone finds
/// out the launcher supports a game before buying it -- and an owned title with
/// no recipe is the most useful row in the list, because it is the one someone
/// can do something about.
pub fn library(inputs: &Inputs) -> Vec<LibraryRow> {
    let mut rows: BTreeMap<String, LibraryRow> = BTreeMap::new();

    for recipe in inputs.recipes {
        let key = recipe.title.product_id.to_ascii_uppercase();
        rows.insert(key.clone(), described_row(inputs, recipe, &key));
    }

    if let Ownership::Known(entries) = inputs.ownership {
        for entry in entries.iter().filter(|e| e.is_title()) {
            let key = entry.product_id.to_ascii_uppercase();
            match rows.get_mut(&key) {
                Some(row) => row.owned = true,
                None => {
                    rows.insert(
                        key.clone(),
                        undescribed_row(inputs, &entry.product_id, &key),
                    );
                }
            }
        }
    }

    let mut rows: Vec<LibraryRow> = rows.into_values().collect();
    // Named rows alphabetically, then the ones still showing a product id.
    // Sorting them together would scatter unnamed rows through the list purely
    // because a Store id happens to start with a digit.
    rows.sort_by(|a, b| {
        a.is_named()
            .cmp(&b.is_named())
            .reverse()
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

fn described_row(inputs: &Inputs, recipe: &Recipe, key: &str) -> LibraryRow {
    let view = row(inputs, recipe);
    let product = inputs.catalog.get(key);
    let installed = (inputs.installed)(recipe);
    let record = inputs.records.get(key);
    let update = record
        .zip(product)
        .and_then(|(record, product)| record.update_available(&product.content_ids));

    let action = match (&view.action, update) {
        // An update is worth offering even when the runtime has doubts about
        // the title: the download is the same either way.
        (Action::Play, Some(true)) => Action::Update,
        (action, _) => action.clone(),
    };

    // An installed title says what it is and whether that is current. Without
    // this, "we cannot tell" draws exactly like "up to date" -- the same row,
    // the same button -- and silence reads as reassurance.
    let subtitle = match (&action, installed) {
        (Action::Blocked(_), _) => view.subtitle,
        (_, true) => {
            let on_disk = (inputs.installed_version)(recipe);
            let status = installed_status(record, product, update, on_disk.as_deref());
            // Both, not either. The caution says whether it will start; the
            // status says whether it is current. Neither answers the other.
            match &view.caution {
                Some(caution) => format!("{status}  ·  {caution}"),
                None => status,
            }
        }
        _ => view.subtitle,
    };

    LibraryRow {
        product_id: recipe.title.product_id.clone(),
        name: product
            .map(|p| p.name.clone())
            .unwrap_or_else(|| recipe.title.name.clone()),
        subtitle,
        badge: Some((view.badge, view.badge_css)),
        image: product.and_then(|p| p.image.clone()),
        owned: false,
        has_recipe: true,
        installed,
        update,
        action,
    }
}

/// What to say under an installed title's name.
///
/// The three states are deliberately distinguishable in words, because they are
/// not distinguishable in the button: only "update available" changes it, and
/// the other two must not both read as "you are fine".
fn installed_status(
    record: Option<&Record>,
    product: Option<&Product>,
    update: Option<bool>,
    on_disk: Option<&str>,
) -> String {
    // The record first, because it is what the comparison is against; the disk
    // second, because a title installed outside this launcher still has one.
    let version = record
        .and_then(|r| r.package_version.as_deref())
        .or(on_disk)
        .map(|v| format!("Version {v}"))
        .unwrap_or_else(|| "Installed".to_string());

    match update {
        Some(true) => format!("{version}  ·  update available"),
        Some(false) => format!("{version}  ·  up to date"),
        None if record.is_none() => {
            format!("{version}  ·  not recorded, so updates cannot be checked")
        }
        None if product.is_none() => format!("{version}  ·  check the library to see updates"),
        // The catalog answered and listed no desktop package to compare
        // against -- an Xbox-only title, or one no longer sold.
        None => format!("{version}  ·  no desktop package listed, so updates cannot be checked"),
    }
}

fn undescribed_row(inputs: &Inputs, product_id: &str, key: &str) -> LibraryRow {
    let product = inputs.catalog.get(key);
    let name = product
        .map(|p| p.name.clone())
        .unwrap_or_else(|| product_id.to_string());
    let subtitle = match product {
        Some(p) => {
            let size = p.size_label();
            match (p.publisher.as_deref(), size.is_empty()) {
                (Some(publisher), false) => format!("{publisher}  ·  {size}  ·  no recipe yet"),
                (Some(publisher), true) => format!("{publisher}  ·  no recipe yet"),
                (None, false) => format!("{size}  ·  no recipe yet"),
                (None, true) => "No recipe yet".to_string(),
            }
        }
        None => "No recipe yet".to_string(),
    };
    LibraryRow {
        product_id: product_id.to_string(),
        name,
        subtitle,
        badge: None,
        image: product.and_then(|p| p.image.clone()),
        owned: true,
        has_recipe: false,
        installed: false,
        update: None,
        action: Action::Adopt,
    }
}

/// Rows matching a search box. Empty query matches everything.
///
/// Matches the product id as well as the name, because the id is what an issue
/// report, a Store URL and a recipe filename all carry, and someone pasting one
/// in should find the row.
pub fn search<'a>(rows: &'a [LibraryRow], query: &str) -> Vec<&'a LibraryRow> {
    let needle = query.trim().to_lowercase();
    rows.iter()
        .filter(|row| {
            needle.is_empty()
                || row.name.to_lowercase().contains(&needle)
                || row.product_id.to_lowercase().contains(&needle)
        })
        .collect()
}

/// One page of rows, and enough to describe the pager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<'a> {
    pub rows: Vec<&'a LibraryRow>,
    /// Zero-based, and always within range: a page number that no longer exists
    /// -- because a search shortened the list -- is clamped rather than shown
    /// empty.
    pub page: usize,
    pub pages: usize,
    pub total: usize,
    /// One-based, inclusive, for "showing 21-40 of 108". Both zero when empty.
    pub first: usize,
    pub last: usize,
}

impl Page<'_> {
    pub fn has_previous(&self) -> bool {
        self.page > 0
    }

    pub fn has_next(&self) -> bool {
        self.page + 1 < self.pages
    }

    pub fn label(&self) -> String {
        if self.total == 0 {
            return "Nothing to show".into();
        }
        format!("{}-{} of {}", self.first, self.last, self.total)
    }
}

pub fn paginate<'a>(rows: &[&'a LibraryRow], page: usize, per_page: usize) -> Page<'a> {
    let per_page = per_page.max(1);
    let total = rows.len();
    let pages = total.div_ceil(per_page).max(1);
    let page = page.min(pages - 1);
    let start = page * per_page;
    let end = (start + per_page).min(total);
    Page {
        rows: rows[start..end].to_vec(),
        page,
        pages,
        total,
        first: if total == 0 { 0 } else { start + 1 },
        last: end,
    }
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

/// Find the `ferestre` binary. Next to this one first, so an AppImage or a package
/// runs its own CLI rather than whichever one happens to be on `PATH`.
pub fn cli_binary(exe_dir: Option<&Path>, is_file: &dyn Fn(&Path) -> bool) -> PathBuf {
    if let Some(dir) = exe_dir {
        let sibling = dir.join("ferestre");
        if is_file(&sibling) {
            return sibling;
        }
    }
    PathBuf::from("ferestre")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferestre_core::runtime::CapabilitySource;
    use std::collections::BTreeSet;

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
            .map(|p| format!(r#"{{"productId":"{p}","productType":"Game","status":"Active"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        ferestre_core::library::parse(&format!("[{items}]")).expect("fixture entries parse")
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
            catalog: empty_catalog(),
            records: empty_records(),
            installed,
            installed_version: NO_VERSION_ON_DISK,
        }
    }

    fn empty_catalog() -> &'static BTreeMap<String, Product> {
        static EMPTY: std::sync::OnceLock<BTreeMap<String, Product>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(BTreeMap::new)
    }

    fn empty_records() -> &'static BTreeMap<String, Record> {
        static EMPTY: std::sync::OnceLock<BTreeMap<String, Record>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(BTreeMap::new)
    }

    fn product(product_id: &str, name: &str, content_ids: &[&str]) -> Product {
        Product {
            product_id: product_id.into(),
            name: name.into(),
            publisher: Some("Test Studios".into()),
            image: Some("https://img/logo".into()),
            download_bytes: Some(2_490_064_896),
            last_update: None,
            content_ids: content_ids.iter().map(|s| s.to_string()).collect(),
            has_packages: !content_ids.is_empty(),
        }
    }

    fn catalog(products: Vec<Product>) -> BTreeMap<String, Product> {
        products
            .into_iter()
            .map(|p| (p.product_id.to_ascii_uppercase(), p))
            .collect()
    }

    fn record(product_id: &str, content_ids: &[&str]) -> Record {
        Record {
            product_id: product_id.into(),
            dir: PathBuf::from("/games/test"),
            content_ids: content_ids.iter().map(|s| s.to_string()).collect(),
            installed_at: None,
            package_version: Some("1.26.4501.0".into()),
        }
    }

    const NOTHING_INSTALLED: &dyn Fn(&Recipe) -> bool = &|_| false;
    const ALL_INSTALLED: &dyn Fn(&Recipe) -> bool = &|_| true;
    /// Most tests describe machines where nothing wrote a manifest.
    const NO_VERSION_ON_DISK: &dyn Fn(&Recipe) -> Option<String> = &|_| None;

    #[test]
    fn a_playable_installed_title_plays() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play);
        assert_eq!(
            rows[0].action.command("9ZZTESTGAME1"),
            Some(["run", "9ZZTESTGAME1"])
        );
        assert!(
            rows[0].subtitle.contains("cannot be checked"),
            "an installed title with nothing recorded about it says so, rather than \
             looking identical to one known to be current: {}",
            rows[0].subtitle
        );
    }

    #[test]
    fn a_playable_title_that_is_not_on_disk_offers_to_install_it() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, Some(&rt), &own, NOTHING_INSTALLED));
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
        let rows = library(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play);
        assert_eq!(rows[0].badge.map(|b| b.0), Some("Untested"));
    }

    #[test]
    fn a_broken_title_says_what_blocks_it_and_offers_no_button() {
        let recipes = vec![broken_recipe()];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
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
        let rows = library(&inputs(&recipes, None, &own, ALL_INSTALLED));
        assert!(matches!(&rows[0].action, Action::Blocked(w) if w.contains("title-protection")));
    }

    #[test]
    fn without_a_runtime_nothing_launches() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, None, &own, ALL_INSTALLED));
        assert_eq!(
            rows[0].action,
            Action::Blocked("install the patched runtime first".into())
        );
    }

    /// A list the build declares is authoritative, so a miss against it is a
    /// refusal.
    #[test]
    fn a_declared_runtime_missing_a_required_capability_blocks_with_one_line() {
        let recipes = vec![recipe(
            "9ZZTESTGAME1",
            "playable",
            &["loader.memfd-main-image"],
        )];
        let rt = runtime_with(&["appmodel.package-identity"], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        let Action::Blocked(why) = &rows[0].action else {
            panic!("expected a block, got {:?}", rows[0].action);
        };
        assert!(!why.contains('\n'), "a row gets one line, got {why:?}");
        assert!(
            !why.contains("Test Title"),
            "the row already shows the name; repeating it is what makes the subtitle overflow: {why}"
        );
        assert!(why.starts_with("Will not"), "reads as a sentence: {why}");
    }

    /// A probe sees almost nothing of what a build provides, so a miss there
    /// means "we could not tell". Greying the button out on that evidence would
    /// stop people trying titles that in fact work -- which is the whole point.
    #[test]
    fn a_probed_runtime_cautions_instead_of_refusing() {
        let recipes = vec![recipe(
            "9ZZTESTGAME1",
            "playable",
            &["loader.memfd-main-image"],
        )];
        let rt = runtime_with(&["appmodel.package-identity"], CapabilitySource::Probed);
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play);
        assert!(
            rows[0].subtitle.contains("could not be found"),
            "the doubt still has to be visible: {:?}",
            rows[0].subtitle
        );
        assert!(!rows[0].subtitle.contains('\n'));
    }

    /// A satisfied runtime raises no caution, so nothing is appended to what the
    /// row would otherwise say.
    #[test]
    fn a_satisfied_runtime_adds_no_caution() {
        let recipes = vec![recipe(
            "9ZZTESTGAME1",
            "playable",
            &["loader.memfd-main-image"],
        )];
        let rt = runtime_with(&["loader.memfd-main-image"], CapabilitySource::Probed);
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play);
        assert!(
            !rows[0].subtitle.contains("could not be found"),
            "{}",
            rows[0].subtitle
        );

        // Not installed: no version to report, so the recipe's summary shows.
        let rows = library(&inputs(&recipes, Some(&rt), &own, NOTHING_INSTALLED));
        assert_eq!(rows[0].subtitle, "a summary");
    }

    /// An installed title can be both out of date and doubtful about starting.
    /// Reporting one and dropping the other loses what the other cannot supply.
    #[test]
    fn a_cautioned_install_reports_its_version_and_the_caution() {
        let recipes = vec![recipe(
            "9ZZTESTGAME1",
            "playable",
            &["loader.memfd-main-image"],
        )];
        let rt = runtime_with(&[], CapabilitySource::Probed);
        let own = Ownership::Unknown;
        let cat = catalog(vec![product("9ZZTESTGAME1", "Test Game", &["content-b"])]);
        let recs: BTreeMap<String, Record> = [(
            "9ZZTESTGAME1".to_string(),
            record("9ZZTESTGAME1", &["content-a"]),
        )]
        .into();
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
        });
        assert_eq!(rows[0].action, Action::Update);
        assert!(
            rows[0].subtitle.contains("update available"),
            "{}",
            rows[0].subtitle
        );
        assert!(
            rows[0].subtitle.contains("could not be found"),
            "{}",
            rows[0].subtitle
        );
    }

    /// An unasked library must not read as "you do not own this", and a library
    /// that was asked and came back without the title must not either.
    #[test]
    fn absence_from_the_library_cautions_rather_than_refuses() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);

        let unknown = Ownership::Unknown;
        assert_eq!(
            library(&inputs(&recipes, Some(&rt), &unknown, ALL_INSTALLED))[0].action,
            Action::Play
        );

        // Not a refusal: the collections listing is demonstrably not
        // exhaustive -- a title installed and licensed on the development
        // machine is absent from it -- so blocking a launch on that evidence
        // would be wrong about something the owner can see is wrong.
        let elsewhere = Ownership::Known(entries(&["9ZZTESTOTHR"]));
        let row = &library(&inputs(&recipes, Some(&rt), &elsewhere, NOTHING_INSTALLED))[0];
        assert_eq!(row.action, Action::Install, "still offered, not refused");
        assert!(
            row.subtitle
                .contains("not in this account's library listing"),
            "but it says so: {}",
            row.subtitle
        );

        let owned = Ownership::Known(entries(&["9zztestgame1"]));
        let row = &library(&inputs(&recipes, Some(&rt), &owned, ALL_INSTALLED))[0];
        assert_eq!(
            row.action,
            Action::Play,
            "the join is case-insensitive, as it is in the CLI"
        );
        assert!(
            !row.subtitle.contains("library listing"),
            "{}",
            row.subtitle
        );
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

    // --- the library view -------------------------------------------------

    #[test]
    fn the_library_is_the_union_of_what_is_owned_and_what_is_described() {
        let recipes = vec![
            recipe("9ZZTESTGAME1", "playable", &[]),
            recipe("9ZZTESTGAME2", "playable", &[]),
        ];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Known(entries(&["9ZZTESTGAME1", "9ZZTESTNEW3"]));
        let cat = catalog(vec![
            product("9ZZTESTGAME1", "Aardvark", &[]),
            product("9ZZTESTNEW3", "Zebra", &[]),
        ]);
        let recs = BTreeMap::new();
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
        });

        let by_id: BTreeMap<&str, &LibraryRow> =
            rows.iter().map(|r| (r.product_id.as_str(), r)).collect();
        assert_eq!(by_id.len(), 3, "owned, described, and both");

        let owned_and_described = by_id["9ZZTESTGAME1"];
        assert!(owned_and_described.owned && owned_and_described.has_recipe);
        assert_eq!(owned_and_described.name, "Aardvark", "the catalog names it");

        let described_not_owned = by_id["9ZZTESTGAME2"];
        assert!(!described_not_owned.owned && described_not_owned.has_recipe);
        assert!(
            !matches!(described_not_owned.action, Action::Adopt),
            "a recipe for something unowned still belongs on screen"
        );

        let owned_not_described = by_id["9ZZTESTNEW3"];
        assert!(owned_not_described.owned && !owned_not_described.has_recipe);
        assert_eq!(owned_not_described.action, Action::Adopt);
        assert!(owned_not_described.badge.is_none(), "no state to report");
        assert!(
            owned_not_described.subtitle.contains("no recipe"),
            "{}",
            owned_not_described.subtitle
        );
    }

    /// A row still showing a twelve-character code is waiting for the catalog,
    /// and burying it among the named ones just because a Store id starts with
    /// a digit makes the list look broken.
    #[test]
    fn named_rows_sort_first_then_alphabetically() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Known(entries(&["9ZZTESTGAME1", "9ZZTESTNEW3", "9ZZTESTNEW4"]));
        let cat = catalog(vec![
            product("9ZZTESTGAME1", "Zebra", &[]),
            product("9ZZTESTNEW4", "aardvark", &[]),
        ]);
        let recs = BTreeMap::new();
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
        });
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["aardvark", "Zebra", "9ZZTESTNEW3"]);
    }

    #[test]
    fn a_rebuilt_package_turns_play_into_update() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let cat = catalog(vec![product("9ZZTESTGAME1", "Test Game", &["content-b"])]);
        let recs: BTreeMap<String, Record> = [(
            "9ZZTESTGAME1".to_string(),
            record("9ZZTESTGAME1", &["content-a"]),
        )]
        .into();
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
        });
        assert_eq!(rows[0].action, Action::Update);
        assert_eq!(rows[0].update, Some(true));
        assert_eq!(
            rows[0].action.command("9ZZTESTGAME1"),
            Some(["install", "9ZZTESTGAME1"]),
            "updating is a reinstall today, and says so"
        );
    }

    #[test]
    fn an_install_with_nothing_recorded_is_not_reported_either_way() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let cat = catalog(vec![product("9ZZTESTGAME1", "Test Game", &["content-b"])]);
        let recs = BTreeMap::new();
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
        });
        assert_eq!(rows[0].update, None);
        assert_eq!(rows[0].action, Action::Play, "not nagged, not hidden");
    }

    /// The failure this guards against: "we cannot tell" and "up to date" draw
    /// the same row and the same button, so if they also read the same, silence
    /// becomes reassurance.
    #[test]
    fn the_three_update_states_read_differently() {
        let up_to_date = installed_status(
            Some(&record("9ZZTESTGAME1", &["content-a"])),
            Some(&product("9ZZTESTGAME1", "T", &["content-a"])),
            Some(false),
            None,
        );
        let stale = installed_status(
            Some(&record("9ZZTESTGAME1", &["content-a"])),
            Some(&product("9ZZTESTGAME1", "T", &["content-b"])),
            Some(true),
            None,
        );
        let unrecorded = installed_status(
            None,
            Some(&product("9ZZTESTGAME1", "T", &["content-a"])),
            None,
            None,
        );
        let no_desktop_package = installed_status(
            Some(&record("9ZZTESTGAME1", &["content-a"])),
            Some(&product("9ZZTESTGAME1", "T", &[])),
            None,
            None,
        );

        assert!(up_to_date.contains("up to date"), "{up_to_date}");
        assert!(stale.contains("update available"), "{stale}");
        assert!(unrecorded.contains("cannot be checked"), "{unrecorded}");
        assert!(
            no_desktop_package.contains("no desktop package"),
            "{no_desktop_package}"
        );

        let all = [&up_to_date, &stale, &unrecorded, &no_desktop_package];
        let distinct: BTreeSet<&String> = all.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            4,
            "no two states may read the same: {all:?}"
        );
    }

    /// A title installed outside this launcher has a version on disk and no
    /// record. Showing "Installed" when the tree plainly says 9.9.9.9 throws
    /// away an answer we already have.
    #[test]
    fn the_version_falls_back_to_the_one_on_disk() {
        let from_disk = installed_status(None, None, None, Some("9.9.9.9"));
        assert!(from_disk.starts_with("Version 9.9.9.9"), "{from_disk}");
        assert!(from_disk.contains("cannot be checked"), "{from_disk}");

        // The record wins when there is one: it is what the comparison is against.
        let recorded = installed_status(
            Some(&record("9ZZTESTGAME1", &["content-a"])),
            Some(&product("9ZZTESTGAME1", "T", &["content-a"])),
            Some(false),
            Some("9.9.9.9"),
        );
        assert!(recorded.starts_with("Version 1.26.4501.0"), "{recorded}");
    }

    /// The version is what a person recognises; without a record there is none
    /// to show, and the row must not pretend otherwise.
    #[test]
    fn an_installed_title_shows_the_version_it_recorded() {
        let with = installed_status(
            Some(&record("9ZZTESTGAME1", &["content-a"])),
            Some(&product("9ZZTESTGAME1", "T", &["content-a"])),
            Some(false),
            None,
        );
        assert!(with.starts_with("Version 1.26.4501.0"), "{with}");

        let without = installed_status(None, None, None, None);
        assert!(without.starts_with("Installed"), "{without}");
        assert!(!without.contains("Version"), "{without}");
    }

    // --- search and pagination --------------------------------------------

    fn rows_named(names: &[(&str, &str)]) -> Vec<LibraryRow> {
        names
            .iter()
            .map(|(id, name)| LibraryRow {
                product_id: (*id).into(),
                name: (*name).into(),
                subtitle: String::new(),
                badge: None,
                image: None,
                owned: true,
                has_recipe: false,
                installed: false,
                update: None,
                action: Action::Adopt,
            })
            .collect()
    }

    #[test]
    fn search_matches_the_name_or_the_product_id() {
        let rows = rows_named(&[
            ("9ZZTESTGAME1", "Minecraft for Windows"),
            ("9ZZTESTGAME2", "Forza Horizon 5"),
        ]);
        assert_eq!(search(&rows, "").len(), 2, "an empty box hides nothing");
        assert_eq!(search(&rows, "  ").len(), 2);
        assert_eq!(search(&rows, "mine")[0].product_id, "9ZZTESTGAME1");
        assert_eq!(search(&rows, "MINE")[0].product_id, "9ZZTESTGAME1");
        assert_eq!(
            search(&rows, "game2")[0].product_id,
            "9ZZTESTGAME2",
            "an id pasted from a Store URL or an issue should find its row"
        );
        assert!(search(&rows, "nothing at all").is_empty());
    }

    #[test]
    fn pages_describe_themselves_the_way_the_pager_reads() {
        let rows = rows_named(
            &(0..45)
                .map(|i| {
                    (
                        "9ZZTEST00000",
                        Box::leak(format!("Title {i:02}").into_boxed_str()) as &str,
                    )
                })
                .collect::<Vec<_>>(),
        );
        let all: Vec<&LibraryRow> = rows.iter().collect();

        let first = paginate(&all, 0, 20);
        assert_eq!(first.rows.len(), 20);
        assert_eq!((first.pages, first.total), (3, 45));
        assert_eq!(first.label(), "1-20 of 45");
        assert!(!first.has_previous() && first.has_next());

        let last = paginate(&all, 2, 20);
        assert_eq!(last.rows.len(), 5);
        assert_eq!(last.label(), "41-45 of 45");
        assert!(last.has_previous() && !last.has_next());
    }

    /// Typing in the search box shortens the list under whatever page someone
    /// was on. Showing them an empty page instead of results would read as
    /// "no matches".
    #[test]
    fn a_page_past_the_end_is_clamped_not_shown_empty() {
        let rows = rows_named(&[("9ZZTESTGAME1", "Only One")]);
        let all: Vec<&LibraryRow> = rows.iter().collect();
        let page = paginate(&all, 7, 20);
        assert_eq!(page.page, 0);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.label(), "1-1 of 1");
    }

    #[test]
    fn an_empty_library_still_produces_a_sane_page() {
        let page = paginate(&[], 0, 20);
        assert_eq!(
            (page.pages, page.total, page.first, page.last),
            (1, 0, 0, 0)
        );
        assert!(!page.has_previous() && !page.has_next());
        assert_eq!(page.label(), "Nothing to show");
    }

    #[test]
    fn the_cli_next_to_the_gui_wins_over_the_path() {
        let dir = Path::new("/opt/ferestre/bin");
        assert_eq!(
            cli_binary(Some(dir), &|p| p == Path::new("/opt/ferestre/bin/ferestre")),
            PathBuf::from("/opt/ferestre/bin/ferestre")
        );
        assert_eq!(cli_binary(Some(dir), &|_| false), PathBuf::from("ferestre"));
        assert_eq!(cli_binary(None, &|_| true), PathBuf::from("ferestre"));
    }
}

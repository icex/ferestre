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
use ferestre_core::gamepass;
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
    /// This window started the title and it is still running. Pressing it asks
    /// the title to close, politely -- a game killed outright loses whatever it
    /// had not written yet, and saves are the whole reason people run these
    /// titles here rather than buying them again somewhere else.
    Stop,
    /// This window is downloading it right now.
    ///
    /// Its own state because the alternatives both lie. A directory appears the
    /// moment a download starts and the package header lands well before the
    /// files do, so "is it on disk" answers yes halfway through -- which is how
    /// a title being downloaded came to offer Set up and call itself installed.
    Installing,
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
            Action::Stop => "Stop",
            Action::Installing => "Installing…",
            Action::Blocked(_) => "Play",
        }
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Action::Blocked(_) | Action::Installing)
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
            // Both are handled by the window rather than by a command: `Adopt`
            // opens the editor, `Stop` signals a process this window started.
            Action::Adopt | Action::Stop | Action::Installing | Action::Blocked(_) => None,
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
    /// The version the service is offering, keyed the same way. Empty today,
    /// and that is the honest state: the anonymous catalog reports every
    /// version as "0", and the endpoint that knows -- `GetBasePackage` on
    /// update.xboxlive.com -- needs a token nothing here mints yet. Injected
    /// rather than looked up so that connecting it is one line at the call
    /// site, and so a test can describe a machine with an update waiting.
    pub available: &'a BTreeMap<String, String>,
    /// Whether the title's install directory is on disk. A closure because the
    /// answer is a filesystem probe, and keeping it out of here is what lets a
    /// test describe a half-installed machine in one line.
    pub installed: &'a dyn Fn(&Recipe) -> bool,
    /// Whether a title with no recipe is nevertheless on disk, by product id.
    /// Installed-but-undescribed happens when the automatic recipe could not
    /// find an executable, or when somebody downloaded it by other means.
    pub installed_product: &'a dyn Fn(&str) -> bool,
    /// Whether the package on disk names its own entry point.
    ///
    /// The difference between a title that can simply be started and one that
    /// genuinely needs a person: when the manifest names an executable there is
    /// nothing to ask, so asking is a step that exists only because nobody
    /// looked. Separate from [`Self::installed_product`] because a package can
    /// be on disk and still not say what to run.
    pub product_describes_itself: &'a dyn Fn(&str) -> bool,
    /// Whether this window is downloading it at this moment. Only what this
    /// window started, like [`Self::running`]: a download begun elsewhere is
    /// not something it can honestly report on.
    pub installing: &'a dyn Fn(&str) -> bool,
    /// Whether this window started the title and it has not exited. Only what
    /// this window started: a title launched from a terminal is not tracked,
    /// and claiming otherwise would need guessing from the process table.
    pub running: &'a dyn Fn(&str) -> bool,
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

/// Append one more piece of doubt to whatever is already there.
fn join(existing: Option<String>, next: &str) -> String {
    match existing {
        Some(first) => format!("{first}  ·  {next}"),
        None => next.to_string(),
    }
}

/// The action, and a line of doubt to show beside it when there is one.
fn action_for(inputs: &Inputs, recipe: &Recipe) -> (Action, Option<String>) {
    let blocked = |why: String| (Action::Blocked(why), None);

    // First, because it is the only one that reports what is happening rather
    // than what could happen. A row offering "Play" for a title that is already
    // running is telling the person something they can see is untrue.
    if (inputs.running)(&recipe.title.product_id) {
        return (Action::Stop, None);
    }
    if (inputs.installing)(&recipe.title.product_id) {
        return (Action::Installing, None);
    }

    // Nothing about the *title* blocks a launch -- only facts about this
    // machine do. A title someone else found broken is a warning, not a
    // prohibition: the catalog exists so people try things and report back,
    // protection changes between builds, and a machine that is not the one it
    // failed on is exactly where the next data point comes from.
    let broken = match recipe.status.state {
        TitleState::Broken => Some(
            recipe
                .status
                .blocked_by
                .as_deref()
                .map(|b| format!("known not to run: {b}"))
                .unwrap_or_else(|| format!("known not to run: {}", recipe.status.summary)),
        ),
        _ => None,
    };

    // Absence from the library listing is a caution, not a refusal.
    //
    // The listing is not exhaustive, and that is not a guess: Clair Obscur:
    // Expedition 33 is installed, licensed and launches on this machine, and
    // does not appear in the 124 entitlements collections returns for the
    // account that owns it. Refusing to launch an installed title on that
    // evidence would be the launcher calling its owner a liar and being wrong.
    // A download for something genuinely unowned fails at the licence step
    // anyway, which teaches the same lesson at the right moment.
    let mut caution = broken;
    if inputs.ownership.owns(&recipe.title.product_id) == Some(false) {
        caution = Some(join(caution, "not in this account's library listing"));
    }

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
        caution = Some(join(caution, &line));
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

/// The child of a bundle worth installing: the first one the catalog has
/// answered about that this runtime can actually open.
///
/// `None` for a product that is not a bundle, and for one whose children have
/// not been fetched yet -- a moment rather than a state, since the children of
/// the rows on screen are asked for alongside their names.
fn bundle_child<'a>(inputs: &'a Inputs, product: Option<&Product>) -> Option<&'a Product> {
    product?
        .bundled_ids
        .iter()
        .find_map(|id| inputs.catalog.get(&id.to_ascii_uppercase()))
        .filter(|child| child.is_runnable_here() == Some(true))
}

/// What can be said about a title's download, as a phrase for a row.
///
/// `None` only when the catalog has not been asked yet. Everything else has an
/// answer, and each of them is a different answer: 35 of the Game Pass titles
/// this account can see have no size of their own, and they split into bundles
/// whose size belongs to a child and products the catalog lists no package for
/// at all. A row that says nothing for both is a row that looks broken for one
/// and misleading for the other.
fn download_note(inputs: &Inputs, product: Option<&Product>) -> Option<String> {
    let product = product?;

    if !product.bundled_ids.is_empty() {
        return Some(match bundle_child(inputs, Some(product)) {
            Some(child) => match child.size_label().as_str() {
                "" => format!("bundle — install {}", child.name),
                size => format!("bundle — install {} ({size})", child.name),
            },
            // The children are fetched with the page, so this is a moment
            // rather than a state; saying how many there are beats saying
            // nothing while it resolves.
            None => format!("bundle of {}", plural(product.bundled_ids.len(), "title")),
        });
    }
    match product.size_label() {
        size if !size.is_empty() => Some(size),
        _ if product.has_packages && !product.has_pc_package => Some("no PC version".into()),
        _ if !product.has_packages => Some("the catalog lists no package for it".into()),
        // A PC package with no size on it. Rare, and not worth a sentence.
        _ => None,
    }
}

/// The rows for the Game Pass section: what a subscription makes installable.
///
/// Built from the public catalogue listing, not from an entitlement, and that
/// is the honest shape. What a given tier covers is not derivable from its name
/// -- the tiers have been renamed and re-sliced, and the catalogue differs by
/// market -- so a row here means "PC Game Pass includes this", never "you can
/// definitely install this". The licence request at install time is what
/// decides, and it says why when it refuses.
///
/// Titles already owned outright are dropped: they are in the library, with
/// their real state, and listing them twice under a heading that implies a
/// subscription is needed would be worse than not listing them.
pub fn gamepass(
    inputs: &Inputs,
    product_ids: &[String],
    tiers: &gamepass::Tiers,
    held: &[&str],
) -> Vec<LibraryRow> {
    let mut rows: Vec<LibraryRow> = product_ids
        .iter()
        .filter(|id| inputs.ownership.owns(id) != Some(true))
        .map(|product_id| {
            let key = product_id.to_ascii_uppercase();
            let product = inputs.catalog.get(&key);
            let name = product
                .map(|p| p.name.clone())
                .unwrap_or_else(|| product_id.clone());
            let installed = (inputs.installed_product)(product_id);
            let installing = (inputs.installing)(product_id);
            let runnable = product.and_then(Product::is_runnable_here);

            // Said before the download, not after it. A title outside the
            // account's tier fails at the licence step with "not entitled to
            // this content" -- after several minutes and a partial container --
            // and the catalogue knew all along.
            let covered = tiers.covered(product_id, held);
            let mut parts = vec![match covered {
                Some(false) => "Not in your Game Pass tier".to_string(),
                _ => "Included with PC Game Pass".to_string(),
            }];
            if let Some(note) = download_note(inputs, product) {
                parts.push(note);
            }
            let action = match (installed, runnable) {
                _ if installing => {
                    parts.push("installing".into());
                    Action::Installing
                }
                (true, _) => {
                    parts.push("installed".into());
                    if (inputs.product_describes_itself)(product_id) {
                        Action::Play
                    } else {
                        Action::Adopt
                    }
                }
                (false, Some(false)) => {
                    let container = product
                        .and_then(|p| p.package_format.clone())
                        .unwrap_or_else(|| "that format".into());
                    parts.push(format!("{container} package, not MSIXVC"));
                    Action::Blocked(format!(
                        "this is a {container} package; the runtime here opens MSIXVC packages"
                    ))
                }
                // `None` is "the catalog has not been asked yet", and it must
                // read as install rather than as a refusal: a row greyed out
                // while its own description is still loading is a row nobody
                // comes back to.
                (false, _) => Action::Install,
            };
            // A warning, never a refusal: the button stays live. The tier
            // mapping is inferred rather than stated by the service, so being
            // wrong here must cost a reader a sentence, not a title -- and
            // somebody who turns the filter off is asking to try anyway.
            if covered == Some(false) {
                parts.push("installing it will probably be refused".into());
            }

            LibraryRow {
                product_id: product_id.clone(),
                name,
                subtitle: parts.join("  ·  "),
                badge: None,
                image: product.and_then(|p| p.image.clone()),
                owned: false,
                has_recipe: false,
                installed,
                update: None,
                unsupported: runnable == Some(false),
                outside_tier: covered == Some(false),
                install_as: bundle_child(inputs, product)
                    .map(|c| (c.product_id.clone(), c.name.clone())),
                action,
            }
        })
        .collect();
    rows.sort_by(sort_key);
    rows
}

/// `1 title`, `3 titles`. English only, like the rest of the window.
fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
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
    /// The catalog says this title's PC package is a container the runtime
    /// cannot open, and nobody has written a recipe claiming otherwise. Of 102
    /// titles owned on the development account, 95 are in this state -- so a
    /// list that does not separate them is one where the seven that work are
    /// lost among the ninety-five that do not. Never set on "we do not know":
    /// hiding a title on no evidence is the same mistake as refusing one.
    pub unsupported: bool,
    /// The catalogue lists this title, and the subscriptions this account holds
    /// do not include it. Only ever set from a positive answer -- `covered()`
    /// says `None` for everything it does not actually know, and a row hidden
    /// on a guess is the same mistake as a row refused on one.
    pub outside_tier: bool,
    /// The product to install for this row, when that is not the row itself,
    /// with its name. Only a bundle sets it.
    ///
    /// A bundle carries no packages, so "install" on one means installing the
    /// child that does. Without this the row could only say which child to go
    /// and find, which is an instruction to do the launcher's job by hand.
    pub install_as: Option<(String, String)>,
    pub action: Action,
}

impl LibraryRow {
    /// Whether the catalog has been asked about this row yet. A row still
    /// showing its product id is waiting, not broken.
    pub fn is_named(&self) -> bool {
        self.name != self.product_id
    }

    /// Whether there is a known reason this machine and this account could not
    /// install it. The rule the default filter uses, in one place because both
    /// list sections need the same answer.
    ///
    /// Both halves are set only from positive evidence, so "we have not asked
    /// yet" is never a reason to hide anything.
    pub fn known_uninstallable(&self) -> bool {
        self.unsupported || self.outside_tier
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

    // And whatever is on disk. The third source, and the one that was missing:
    // a title installed through a Game Pass subscription is not in the
    // entitlement listing and has no recipe, so it appeared in neither of the
    // two lists above -- installed, taking up 40 GB, and absent from the
    // library and from Installed both.
    for record in inputs.records.values() {
        let key = record.product_id.to_ascii_uppercase();
        if !rows.contains_key(&key) {
            rows.insert(
                key.clone(),
                undescribed_row(inputs, &record.product_id, &key),
            );
        }
    }

    let mut rows: Vec<LibraryRow> = rows.into_values().collect();
    rows.sort_by(sort_key);
    rows
}

/// Named rows alphabetically, then the ones still showing a product id.
///
/// Sorting them together would scatter unnamed rows through the list purely
/// because a Store id happens to start with a digit.
fn sort_key(a: &LibraryRow, b: &LibraryRow) -> std::cmp::Ordering {
    a.is_named()
        .cmp(&b.is_named())
        .reverse()
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
}

fn described_row(inputs: &Inputs, recipe: &Recipe, key: &str) -> LibraryRow {
    let view = row(inputs, recipe);
    let product = inputs.catalog.get(key);
    let installed = (inputs.installed)(recipe);
    let record = inputs.records.get(key);
    let update = record
        .and_then(|record| record.update_available(inputs.available.get(key).map(String::as_str)));

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
        (Action::Stop, _) => format!("{}  ·  running", installed_version_or(record, product)),
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
        // A described title is one somebody got working, whatever the catalog
        // says the container is.
        unsupported: false,
        outside_tier: false,
        install_as: None,
        action,
    }
}

/// The version to show, from the record if there is one and the disk if not.
fn installed_version_or(record: Option<&Record>, _product: Option<&Product>) -> String {
    record
        .and_then(|r| r.package_version.as_deref())
        .map(|v| format!("Version {v}"))
        .unwrap_or_else(|| "Installed".to_string())
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
        // A record and a catalog entry, and still no answer: the version on
        // offer is not published anonymously. Saying so is the point -- the
        // failure this wording exists to prevent is silence reading as "fine".
        None => format!("{version}  ·  update checking is not wired up yet"),
    }
}

fn undescribed_row(inputs: &Inputs, product_id: &str, key: &str) -> LibraryRow {
    let product = inputs.catalog.get(key);
    let name = product
        .map(|p| p.name.clone())
        .unwrap_or_else(|| product_id.to_string());
    // Whether it is on disk is asked once and used everywhere below, including
    // on the rows that cannot run: something that got installed and then turned
    // out to be unrunnable is exactly the row that needs to offer removal.
    let on_disk = (inputs.installed_product)(product_id);
    let installing = (inputs.installing)(product_id);
    let unrunnable = |subtitle: String, reason: String| LibraryRow {
        product_id: product_id.to_string(),
        name: name.clone(),
        subtitle,
        badge: None,
        image: product.and_then(|p| p.image.clone()),
        owned: true,
        has_recipe: false,
        installed: on_disk,
        update: None,
        unsupported: true,
        outside_tier: false,
        install_as: None,
        action: Action::Blocked(reason),
    };

    // A bundle before anything else, because it is the one state where the
    // answer is somewhere other than this record: a bundle carries no packages,
    // so every other check would read it as "nothing is known".
    if let Some(children) = product.map(|p| &p.bundled_ids).filter(|c| !c.is_empty()) {
        return match bundle_child(inputs, product) {
            // Installable, by installing the child. The row names it, because
            // pressing Install on "Age of Empires II: Definitive Edition" and
            // getting a differently-named download is otherwise the launcher
            // doing something unexplained.
            Some(child) => LibraryRow {
                product_id: product_id.to_string(),
                name: name.clone(),
                subtitle: format!(
                    "Bundle of {}  ·  installs {}{}",
                    plural(children.len(), "title"),
                    child.name,
                    match child.size_label().as_str() {
                        "" => String::new(),
                        size => format!("  ·  {size}"),
                    }
                ),
                badge: None,
                image: product.and_then(|p| p.image.clone()),
                owned: true,
                has_recipe: false,
                installed: on_disk,
                update: None,
                unsupported: false,
                outside_tier: false,
                install_as: Some((child.product_id.clone(), child.name.clone())),
                action: Action::Install,
            },
            None => unrunnable(
                format!(
                    "Bundle of {}  ·  none of them is a title this runtime opens",
                    plural(children.len(), "title")
                ),
                "a bundle, and none of its titles ships a package this runtime opens".into(),
            ),
        };
    }

    // Packages, but none a PC can install. A fact about the title rather than
    // a gap in what was fetched, and the difference matters: "no recipe yet"
    // invites someone to write one for a title that has no PC build to launch.
    if product.is_some_and(|p| p.has_packages && !p.has_pc_package) {
        return unrunnable(
            "No PC version  ·  this title ships for Xbox only".into(),
            "the catalog lists no PC package for this title, only an Xbox one".into(),
        );
    }

    // A PC package this runtime cannot open is worth naming before someone
    // spends a download finding out. Candy Crush Saga is a UWP Appx; the whole
    // decrypt-and-launch path here is built for MSIXVC.
    if let Some(format) = product.filter(|p| p.is_runnable_here() == Some(false)) {
        let container = format.package_format.as_deref().unwrap_or("that format");
        return LibraryRow {
            product_id: product_id.to_string(),
            name,
            subtitle: format!(
                "{container} package, not MSIXVC — Ferestre installs Xbox GDK titles"
            ),
            badge: None,
            image: product.and_then(|p| p.image.clone()),
            owned: true,
            has_recipe: false,
            installed: false,
            update: None,
            unsupported: true,
            outside_tier: false,
            install_as: None,
            action: Action::Blocked(format!(
                "this is a {container} package; the runtime here opens MSIXVC packages"
            )),
        };
    }
    let subtitle = match product {
        Some(p) => {
            // "No recipe yet" is the right closing note only when a download is
            // the next step. When the catalog lists no package at all -- a
            // delisted title, or one fulfilled some other way -- saying that
            // invites someone to write a recipe for something there is nothing
            // to install.
            let tail = if p.has_packages {
                "no recipe yet"
            } else {
                "the catalog lists no package for it"
            };
            let size = p.size_label();
            match (p.publisher.as_deref(), size.is_empty()) {
                (Some(publisher), false) => format!("{publisher}  ·  {size}  ·  {tail}"),
                (Some(publisher), true) => format!("{publisher}  ·  {tail}"),
                (None, false) => format!("{size}  ·  {tail}"),
                (None, true) => {
                    let mut chars = tail.chars();
                    chars
                        .next()
                        .into_iter()
                        .flat_map(char::to_uppercase)
                        .collect::<String>()
                        + chars.as_str()
                }
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
        installed: on_disk,
        update: None,
        unsupported: false,
        outside_tier: false,
        install_as: None,
        // Install, unless it is already here -- and then Play, not "set up",
        // whenever the package names its own entry point. That is the common
        // case, and asking someone to go and find an .exe in a tree of
        // thousands of files when the manifest already says which one is a step
        // that exists only because nobody read it. `run` writes the recipe from
        // the manifest on the way past, so the answer is kept and stays
        // editable for a title that turns out to need something else.
        //
        // Set up stays for the case that genuinely needs a person: on disk, and
        // the package does not say what to start.
        action: match (on_disk, (inputs.product_describes_itself)(product_id)) {
            _ if installing => Action::Installing,
            (true, true) => Action::Play,
            (true, false) => Action::Adopt,
            (false, _) => Action::Install,
        },
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
            available: empty_available(),
            installed,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
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

    /// The production state: nothing knows what version is on offer.
    fn empty_available() -> &'static BTreeMap<String, String> {
        static EMPTY: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(BTreeMap::new)
    }

    /// A machine where the service is offering a newer build.
    fn offering(product_id: &str, version: &str) -> BTreeMap<String, String> {
        [(product_id.to_ascii_uppercase(), version.to_string())].into()
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
            // A coherent record: it has a size and an MSIXVC format, so it has
            // a PC package. Deriving this from `content_ids` instead made the
            // fixture describe a title with a 2.5 GB download and no packages,
            // which the catalog would never return.
            has_packages: true,
            package_format: Some("MSIXVC".into()),
            has_pc_package: true,
            bundled_ids: Vec::new(),
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
    /// And where the window has not started anything.
    const NOTHING_RUNNING: &dyn Fn(&str) -> bool = &|_| false;
    /// And where an undescribed title is not on disk either.
    const NOT_ON_DISK: &dyn Fn(&str) -> bool = &|_| false;
    /// And so nothing on disk describes itself.
    const NOTHING_DESCRIBES_ITSELF: &dyn Fn(&str) -> bool = &|_| false;
    /// And nothing is being downloaded.
    const NOTHING_INSTALLING: &dyn Fn(&str) -> bool = &|_| false;

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

    /// A title someone else found broken is a warning, not a prohibition. The
    /// catalog exists so people try things and report back, and the machine it
    /// did not fail on is where the next data point comes from.
    #[test]
    fn a_broken_title_is_launchable_and_says_what_is_known() {
        let recipes = vec![broken_recipe()];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, Some(&rt), &own, ALL_INSTALLED));
        assert_eq!(rows[0].action, Action::Play, "offered, not refused");
        assert!(rows[0].action.is_enabled());
        assert!(
            rows[0]
                .subtitle
                .contains("known not to run: title-protection"),
            "but the row says what is known: {}",
            rows[0].subtitle
        );
        assert_eq!(
            rows[0].badge.map(|b| b.0),
            Some("Does not run"),
            "and the badge still says so at a glance"
        );
    }

    /// What *does* block is a fact about this machine. With no runtime there is
    /// nothing to run anything with, and the action is to install one.
    #[test]
    fn a_missing_runtime_blocks_where_a_broken_title_does_not() {
        let recipes = vec![broken_recipe()];
        let own = Ownership::Unknown;
        let rows = library(&inputs(&recipes, None, &own, ALL_INSTALLED));
        assert_eq!(
            rows[0].action,
            Action::Blocked("install the patched runtime first".into())
        );
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
        let avail = offering("9ZZTESTGAME1", "1.26.4600.0");
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            available: &avail,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
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
        let avail = BTreeMap::new();
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            available: &avail,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
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
        // Install, not "set up": nothing can describe a title that is not on
        // disk yet, because the executable comes out of its package manifest.
        assert_eq!(owned_not_described.action, Action::Install);
        assert!(owned_not_described.badge.is_none(), "no state to report");
        assert!(
            owned_not_described.subtitle.contains("no recipe"),
            "{}",
            owned_not_described.subtitle
        );
    }

    /// The flag the library filter hides rows by. It exists because 95 of the
    /// 102 titles owned on the development account are UWP packages this
    /// runtime cannot open, so a list that mixes them in is one where the seven
    /// that work cannot be found.
    ///
    /// The half that matters most is the last assertion: "we could not ask the
    /// catalog" must never set it. Hiding a title on no evidence is the same
    /// mistake as refusing to run one on no evidence.
    #[test]
    fn only_a_known_non_msixvc_package_is_marked_unsupported() {
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Known(entries(&["9ZZTESTUWP1", "9ZZTESTGDK2", "9ZZTESTMYST"]));
        let uwp = Product {
            package_format: Some("AppxBundle".into()),
            ..product("9ZZTESTUWP1", "Candy Something", &["contentid-uwp"])
        };
        // 9ZZTESTMYST is owned but deliberately absent from the catalog.
        let cat = catalog(vec![uwp, product("9ZZTESTGDK2", "A GDK Title", &["cid"])]);
        let recs = BTreeMap::new();
        let avail = BTreeMap::new();
        let rows = library(&Inputs {
            recipes: &[],
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            available: &avail,
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
        });
        let by_id: BTreeMap<&str, &LibraryRow> =
            rows.iter().map(|r| (r.product_id.as_str(), r)).collect();

        let uwp = by_id["9ZZTESTUWP1"];
        assert!(uwp.unsupported, "an AppxBundle cannot be opened here");
        assert!(matches!(uwp.action, Action::Blocked(_)));
        assert!(
            uwp.subtitle.contains("AppxBundle"),
            "name the format rather than saying it failed: {}",
            uwp.subtitle
        );
        assert_eq!(
            uwp.name, "Candy Something",
            "still a real name while hidden"
        );

        assert!(
            !by_id["9ZZTESTGDK2"].unsupported,
            "MSIXVC is the runnable one"
        );
        assert_eq!(by_id["9ZZTESTGDK2"].action, Action::Install);

        assert!(
            !by_id["9ZZTESTMYST"].unsupported,
            "the catalog said nothing about it, which is not the same as saying no"
        );
    }

    /// Three states the catalog can now tell apart, and used not to. Every one
    /// of them used to render as "no recipe yet", which is advice to write a
    /// recipe -- useless for a title with no PC build, and actively wrong for a
    /// bundle, where the thing to install is a different product entirely.
    #[test]
    fn a_title_with_no_pc_build_says_so_instead_of_asking_for_a_recipe() {
        let console = Product {
            has_packages: true,
            has_pc_package: false,
            package_format: None,
            download_bytes: None,
            ..product("9ZZTESTXBOX", "A Console Title", &[])
        };
        let own = Ownership::Known(entries(&["9ZZTESTXBOX"]));
        let cat = catalog(vec![console]);
        let rows = library(&Inputs {
            recipes: &[],
            runtime: None,
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &BTreeMap::new(),
            available: &BTreeMap::new(),
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
        });
        assert!(
            rows[0].subtitle.contains("No PC version"),
            "{}",
            rows[0].subtitle
        );
        assert!(
            !rows[0].subtitle.contains("recipe"),
            "no recipe can give a title a PC build: {}",
            rows[0].subtitle
        );
        assert!(rows[0].unsupported);
        assert!(matches!(rows[0].action, Action::Blocked(_)));
    }

    /// A bundle points at the child worth installing, by name and by size.
    /// Minecraft: Java & Bedrock Edition is the real case: the bundle itself
    /// has no package, and one of its three children is a title this launcher
    /// runs.
    #[test]
    fn a_bundle_installs_the_child_that_has_the_package() {
        let bundle = Product {
            has_packages: false,
            has_pc_package: false,
            package_format: None,
            download_bytes: None,
            bundled_ids: vec!["9ZZTESTCHLD".into(), "9ZZTESTOTHR".into()],
            ..product("9ZZTESTBNDL", "A Bundle", &[])
        };
        let child = product("9ZZTESTCHLD", "The Playable One", &["cid"]);
        let own = Ownership::Known(entries(&["9ZZTESTBNDL"]));
        let cat = catalog(vec![bundle, child]);
        let rows = library(&Inputs {
            recipes: &[],
            runtime: None,
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &BTreeMap::new(),
            available: &BTreeMap::new(),
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
        });
        let row = rows
            .iter()
            .find(|r| r.product_id == "9ZZTESTBNDL")
            .expect("the bundle");
        assert!(
            row.subtitle.contains("Bundle of 2 titles"),
            "{}",
            row.subtitle
        );
        assert!(
            row.subtitle.contains("installs The Playable One"),
            "{}",
            row.subtitle
        );
        assert!(
            row.subtitle.contains("2.5 GB"),
            "the child's size is the bundle's answer: {}",
            row.subtitle
        );
        // Pressing Install works, and installs the child. Before this the row
        // could only name the child and leave the reader to go and find it.
        assert_eq!(row.action, Action::Install);
        assert_eq!(
            row.install_as,
            Some(("9ZZTESTCHLD".to_string(), "The Playable One".to_string())),
            "the download is the child's, under the child's name"
        );
    }

    /// And says only what it knows. Before the children are fetched there is no
    /// name and no size to give, and inventing either would be worse than the
    /// count on its own.
    #[test]
    fn a_bundle_whose_children_are_not_cached_yet_claims_nothing() {
        let bundle = Product {
            has_packages: false,
            has_pc_package: false,
            package_format: None,
            download_bytes: None,
            bundled_ids: vec!["9ZZTESTCHLD".into()],
            ..product("9ZZTESTBNDL", "A Bundle", &[])
        };
        let own = Ownership::Known(entries(&["9ZZTESTBNDL"]));
        let cat = catalog(vec![bundle]);
        let rows = library(&Inputs {
            recipes: &[],
            runtime: None,
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &BTreeMap::new(),
            available: &BTreeMap::new(),
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
        });
        assert!(
            rows[0].subtitle.contains("Bundle of 1 title"),
            "{}",
            rows[0].subtitle
        );
        assert!(
            !rows[0].subtitle.contains("install"),
            "{}",
            rows[0].subtitle
        );
    }

    /// The Game Pass list is what a subscription makes installable, and the
    /// three states it has to keep apart.
    #[test]
    fn the_gamepass_list_shows_what_is_installable_and_says_what_is_not() {
        let listing: Vec<String> = ["9ZZTESTGDK1", "9ZZTESTUWP2", "9ZZTESTOWND", "9ZZTESTUNKN"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Owned outright: it belongs in the library with its real state, not
        // here under a heading implying a subscription is needed for it.
        let own = Ownership::Known(entries(&["9ZZTESTOWND"]));
        let cat = catalog(vec![
            product("9ZZTESTGDK1", "A GDK Title", &["cid"]),
            Product {
                package_format: Some("AppxBundle".into()),
                ..product("9ZZTESTUWP2", "A UWP Title", &["cid2"])
            },
            product("9ZZTESTOWND", "Already Owned", &["cid3"]),
            // 9ZZTESTUNKN is deliberately absent: the catalog has not answered.
        ]);
        let rows = gamepass(
            &Inputs {
                recipes: &[],
                runtime: None,
                registry: None,
                ownership: &own,
                catalog: &cat,
                records: &BTreeMap::new(),
                available: &BTreeMap::new(),
                installed: NOTHING_INSTALLED,
                installed_version: NO_VERSION_ON_DISK,
                installed_product: NOT_ON_DISK,
                product_describes_itself: NOTHING_DESCRIBES_ITSELF,
                installing: NOTHING_INSTALLING,
                running: NOTHING_RUNNING,
            },
            &listing,
            &gamepass::Tiers::default(),
            &[],
        );

        let ids: Vec<&str> = rows.iter().map(|r| r.product_id.as_str()).collect();
        assert!(
            !ids.contains(&"9ZZTESTOWND"),
            "an owned title is in the library, not in the subscription list: {ids:?}"
        );
        assert_eq!(rows.len(), 3);

        let by_id = |id: &str| rows.iter().find(|r| r.product_id == id).expect(id);

        let runnable = by_id("9ZZTESTGDK1");
        assert_eq!(runnable.action, Action::Install);
        assert!(
            !runnable.owned,
            "included is not owned, and the row says so"
        );
        assert!(runnable.subtitle.contains("Included with PC Game Pass"));
        assert!(
            runnable.subtitle.contains("2.5 GB"),
            "the size before the download: {}",
            runnable.subtitle
        );

        let uwp = by_id("9ZZTESTUWP2");
        assert!(uwp.unsupported);
        assert!(matches!(uwp.action, Action::Blocked(_)));

        // Still loading is not a refusal. A row greyed out while its own
        // description is being fetched is a row nobody comes back to.
        let unknown = by_id("9ZZTESTUNKN");
        assert_eq!(unknown.action, Action::Install);
        assert!(!unknown.unsupported);
    }

    /// A title installed through a subscription is in neither the entitlement
    /// listing nor the recipes, so before this it was in no list at all --
    /// installed, taking up disk, and invisible in both Library and Installed.
    #[test]
    fn a_title_that_is_only_on_disk_still_gets_a_row() {
        let mut recs = BTreeMap::new();
        recs.insert("9ZZTESTDISK".to_string(), record("9ZZTESTDISK", &[]));
        let cat = catalog(vec![product("9ZZTESTDISK", "Only On Disk", &["cid"])]);
        let on_disk: &dyn Fn(&str) -> bool = &|id| id.eq_ignore_ascii_case("9ZZTESTDISK");
        let describes: &dyn Fn(&str) -> bool = &|_| true;
        let rows = library(&Inputs {
            recipes: &[],
            // Signed out, so nothing is known about what the account owns.
            ownership: &Ownership::Unknown,
            runtime: None,
            registry: None,
            catalog: &cat,
            records: &recs,
            available: &BTreeMap::new(),
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: on_disk,
            product_describes_itself: describes,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
        });
        assert_eq!(rows.len(), 1, "the row exists at all");
        assert_eq!(rows[0].name, "Only On Disk");
        assert!(rows[0].installed, "and it shows up under Installed");
    }

    /// The package manifest names the entry point, so there is nothing to ask.
    /// "Set up" survives only for the case that genuinely needs a person: on
    /// disk, and the package does not say what to start.
    #[test]
    fn an_installed_title_that_describes_itself_offers_play_not_set_up() {
        let mut recs = BTreeMap::new();
        recs.insert("9ZZTESTDISK".to_string(), record("9ZZTESTDISK", &[]));
        let cat = catalog(vec![product("9ZZTESTDISK", "Only On Disk", &["cid"])]);
        let on_disk: &dyn Fn(&str) -> bool = &|_| true;

        let with_manifest: &dyn Fn(&str) -> bool = &|_| true;
        let rows = library(&Inputs {
            recipes: &[],
            ownership: &Ownership::Unknown,
            runtime: None,
            registry: None,
            catalog: &cat,
            records: &recs,
            available: &BTreeMap::new(),
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: on_disk,
            product_describes_itself: with_manifest,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
        });
        assert_eq!(rows[0].action, Action::Play);

        let rows = library(&Inputs {
            recipes: &[],
            ownership: &Ownership::Unknown,
            runtime: None,
            registry: None,
            catalog: &cat,
            records: &recs,
            available: &BTreeMap::new(),
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: on_disk,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
        });
        assert_eq!(
            rows[0].action,
            Action::Adopt,
            "nothing on disk says what to start, so a person has to"
        );
    }

    /// Said before the download instead of after it. Blood Dungeon is in the PC
    /// catalogue and not in the tier this account holds, and the only way to
    /// find that out used to be to start the download and have the licence step
    /// refuse it several minutes in.
    #[test]
    fn a_gamepass_title_outside_the_held_tier_says_so_before_the_download() {
        let listing = vec!["9MSVBF0KZFVW".to_string(), "9MSVVM5NS9L6".to_string()];
        let cat = catalog(vec![
            product("9MSVBF0KZFVW", "Blood Dungeon", &["cid"]),
            product("9MSVVM5NS9L6", "DREDGE", &["cid2"]),
        ]);
        let tiers = ferestre_core::gamepass::parse_tiers(
            r#"{
              "9MSVBF0KZFVW": {"PCSubMetadata": {"Included": true}},
              "9MSVVM5NS9L6": {"PCSubMetadata": {"Included": true},
                               "StandardSubMetadata": {"Included": true}}
            }"#,
        )
        .expect("fixture parses");
        let rows = gamepass(
            &Inputs {
                recipes: &[],
                runtime: None,
                registry: None,
                ownership: &Ownership::Unknown,
                catalog: &cat,
                records: &BTreeMap::new(),
                available: &BTreeMap::new(),
                installed: NOTHING_INSTALLED,
                installed_version: NO_VERSION_ON_DISK,
                installed_product: NOT_ON_DISK,
                product_describes_itself: NOTHING_DESCRIBES_ITSELF,
                installing: NOTHING_INSTALLING,
                running: NOTHING_RUNNING,
            },
            &listing,
            &tiers,
            &["Game Pass Premium"],
        );
        let by_id = |id: &str| rows.iter().find(|r| r.product_id == id).expect(id);

        let outside = by_id("9MSVBF0KZFVW");
        assert!(
            outside.subtitle.contains("Not in your Game Pass tier"),
            "{}",
            outside.subtitle
        );
        // A warning, not a refusal: the tier mapping is inferred, and being
        // wrong must cost a sentence rather than a title.
        assert_eq!(outside.action, Action::Install);

        let inside = by_id("9MSVVM5NS9L6");
        assert!(
            inside.subtitle.contains("Included with PC Game Pass"),
            "{}",
            inside.subtitle
        );
        assert!(!inside.subtitle.contains("refused"), "{}", inside.subtitle);
    }

    /// A download in flight is its own state. Both alternatives lie: a
    /// directory appears the moment a download starts and the package header
    /// lands well before the files do, so "is it on disk" answers yes halfway
    /// through -- which is how a title being downloaded came to call itself
    /// installed and offer Set up.
    #[test]
    fn a_title_being_downloaded_says_so_and_offers_nothing_to_press() {
        let downloading: &dyn Fn(&str) -> bool = &|id| id.eq_ignore_ascii_case("9ZZTESTGDK1");
        // The worst case for the old behaviour: far enough in that the files
        // look like an install.
        let on_disk: &dyn Fn(&str) -> bool = &|_| true;
        let cat = catalog(vec![product("9ZZTESTGDK1", "Being Downloaded", &["cid"])]);
        let own = Ownership::Known(entries(&["9ZZTESTGDK1"]));
        let rows = library(&Inputs {
            recipes: &[],
            runtime: None,
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &BTreeMap::new(),
            available: &BTreeMap::new(),
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: on_disk,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: downloading,
            running: NOTHING_RUNNING,
        });
        assert_eq!(rows[0].action, Action::Installing);
        assert_eq!(rows[0].action.label(), "Installing…");
        assert!(!rows[0].action.is_enabled());
        assert_eq!(
            rows[0].action.command("9ZZTESTGDK1"),
            None,
            "there is nothing to run: it is already running"
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
        let avail = BTreeMap::new();
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            available: &avail,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
        });
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["aardvark", "Zebra", "9ZZTESTNEW3"]);
    }

    /// The record holds 1.26.4501.0; the service offers 1.26.4600.0.
    #[test]
    fn a_newer_version_on_offer_turns_play_into_update() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let cat = catalog(vec![product("9ZZTESTGAME1", "Test Game", &["content-b"])]);
        let recs: BTreeMap<String, Record> = [(
            "9ZZTESTGAME1".to_string(),
            record("9ZZTESTGAME1", &["content-a"]),
        )]
        .into();
        let avail = offering("9ZZTESTGAME1", "1.26.4600.0");
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            available: &avail,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
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
        let avail = BTreeMap::new();
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            available: &avail,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
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
        let not_wired_up = installed_status(
            Some(&record("9ZZTESTGAME1", &["content-a"])),
            Some(&product("9ZZTESTGAME1", "T", &[])),
            None,
            None,
        );

        assert!(up_to_date.contains("up to date"), "{up_to_date}");
        assert!(stale.contains("update available"), "{stale}");
        assert!(unrecorded.contains("cannot be checked"), "{unrecorded}");
        assert!(not_wired_up.contains("not wired up"), "{not_wired_up}");

        let all = [&up_to_date, &stale, &unrecorded, &not_wired_up];
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

    /// A row that says "Play" for a title that is already running is telling
    /// someone something they can see is untrue.
    #[test]
    fn a_running_title_offers_to_stop_rather_than_to_play() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Unknown;
        let recs: BTreeMap<String, Record> = [(
            "9ZZTESTGAME1".to_string(),
            record("9ZZTESTGAME1", &["content-a"]),
        )]
        .into();
        let cat = BTreeMap::new();
        let avail = BTreeMap::new();
        let running: &dyn Fn(&str) -> bool = &|id| id == "9ZZTESTGAME1";
        let rows = library(&Inputs {
            recipes: &recipes,
            runtime: Some(&rt),
            registry: None,
            ownership: &own,
            catalog: &cat,
            records: &recs,
            available: &avail,
            installed: ALL_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running,
        });
        assert_eq!(rows[0].action, Action::Stop);
        assert_eq!(rows[0].action.label(), "Stop");
        assert!(
            rows[0].action.is_enabled(),
            "stopping is something you can do"
        );
        assert_eq!(
            rows[0].action.command("9ZZTESTGAME1"),
            None,
            "the window signals the process it started; there is no command for it"
        );
        assert!(rows[0].subtitle.contains("running"), "{}", rows[0].subtitle);
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
                unsupported: false,
                outside_tier: false,
                install_as: None,
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

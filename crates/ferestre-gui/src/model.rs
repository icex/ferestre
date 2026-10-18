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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ferestre_core::autofix;
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
    /// A download that stopped before it finished left files behind. The same
    /// command as [`Action::Install`], which picks up where it stopped; its own
    /// state so the row does not offer to launch half a game.
    Resume,
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
            Action::Resume => "Resume",
            Action::Update => "Update",
            Action::Adopt => "Set up",
            Action::Stop => "Stop",
            Action::Installing => "Installing..",
            Action::Blocked(_) => "Play",
        }
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Action::Blocked(_) | Action::Installing)
    }

    pub fn is_enabled_during_download(&self, downloading: bool) -> bool {
        self.is_enabled()
            && !(downloading && matches!(self, Action::Install | Action::Resume | Action::Update))
    }

    /// The `ferestre` arguments this action runs, or `None` when it runs nothing.
    /// `Adopt` runs nothing on purpose -- it opens the editor -- so this being
    /// `None` is not the same question as [`Action::is_enabled`].
    pub fn command<'a>(&self, product_id: &'a str) -> Option<[&'a str; 2]> {
        match self {
            Action::Play => Some(["run", product_id]),
            // Updating is a reinstall today. Fetching only what changed is a
            // separate piece of work and this is the honest version of it.
            Action::Install | Action::Resume | Action::Update => Some(["install", product_id]),
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
        TitleState::Menus => ("Menus only", "warning"),
        TitleState::Broken => ("Does not run", "error"),
        TitleState::Untested => ("Untested", "dim-label"),
    }
}

/// Everything a row needs that the pure code cannot find out for itself.
pub struct Inputs<'a> {
    pub recipes: &'a [Recipe],
    /// Every runtime installed here, newest first. A row is judged against the
    /// one its launch would use, not simply the newest: see [`launch_runtime`].
    pub runtimes: &'a [InstalledRuntime],
    /// The runtime chosen for a title in "Choose runtime", by product id.
    pub selected_runtime: &'a dyn Fn(&str) -> Option<PathBuf>,
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
    /// Whether a download of this product stopped before it finished and left
    /// files where the title goes. Such a folder is not an install -- the
    /// package header and the manifest land long before the rest -- and it is
    /// not nothing either: it is resumed, or removed.
    pub incomplete: &'a dyn Fn(&str) -> bool,
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
    /// Where the title is installed, when that is known. Handed to
    /// [`autofix::infer`] so a row is assessed against the same recipe `ferestre
    /// run` will use, engine defaults included.
    pub game_dir: &'a dyn Fn(&Recipe) -> Option<PathBuf>,
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

    // Collections omit subscription grants and some licensed purchases.
    // Access source is labelled separately from runtime compatibility.
    let mut caution = broken;

    let game_dir = (inputs.game_dir)(recipe);
    let inferred = autofix::infer(recipe, game_dir.as_deref());
    let requested = (inputs.selected_runtime)(&recipe.title.product_id);
    let assessment = match launch_runtime(
        &inferred.recipe,
        inputs.runtimes,
        requested.as_deref(),
        inputs.registry,
    ) {
        Ok((_, assessment)) => assessment,
        Err(why) => return blocked(why),
    };
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
    } else if (inputs.incomplete)(&recipe.title.product_id) {
        Action::Resume
    } else {
        Action::Install
    };
    (action, caution)
}

/// The runtime a launch of `recipe` would use, and how it measures up to it.
///
/// The rule `ferestre run` follows, so a row cannot offer Play for a launch
/// that is then refused, or refuse one that would work. A runtime chosen for
/// the title is the one used, and only it: if it is gone, that is the answer.
/// Otherwise the newest runtime that satisfies the recipe, and when none does,
/// the newest, whose assessment then says what is missing. A miss against a
/// published capability list refuses; a miss against a probed one only warns,
/// because a probe cannot see most of what a build provides.
///
/// `Err` is a sentence for the row when there is no runtime to judge by.
pub fn launch_runtime<'a>(
    recipe: &Recipe,
    runtimes: &'a [InstalledRuntime],
    requested: Option<&Path>,
    registry: Option<&Registry>,
) -> Result<(&'a InstalledRuntime, runtime::Assessment), String> {
    if runtimes.is_empty() {
        return Err("install the patched runtime first".into());
    }
    if let Some(path) = requested {
        let chosen = runtimes.iter().find(|rt| rt.path == path).ok_or_else(|| {
            format!(
                "the runtime chosen for this title is no longer installed ({}); choose another \
                 with Choose runtime",
                path.display()
            )
        })?;
        return Ok((chosen, runtime::assess(recipe, chosen, registry)));
    }
    let assessed = runtimes
        .iter()
        .map(|rt| (rt, runtime::assess(recipe, rt, registry)));
    let mut newest = None;
    for (rt, assessment) in assessed {
        if assessment.is_satisfied() {
            return Ok((rt, assessment));
        }
        newest.get_or_insert((rt, assessment));
    }
    Ok(newest.expect("runtimes is not empty"))
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

/// Package and bundle context, without duplicating the shared size phrase.
fn package_note(inputs: &Inputs, product: Option<&Product>) -> Option<String> {
    let product = product?;

    if !product.bundled_ids.is_empty() {
        return Some(match bundle_child(inputs, Some(product)) {
            Some(child) => format!("bundle — install {}", child.name),
            // The children are fetched with the page, so this is a moment
            // rather than a state; saying how many there are beats saying
            // nothing while it resolves.
            None => format!("bundle of {}", plural(product.bundled_ids.len(), "title")),
        });
    }
    match product.size_label() {
        size if !size.is_empty() => None,
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
/// `merchandised` is the "All PC Games" list itself. Ids the tier feed adds to
/// it (see [`gamepass::complete_listing`]) are listed only once the catalog
/// confirms a PC package -- add-ons are dropped above by their kind -- and
/// until then stay behind the "cannot install" switch. `None` treats every id
/// as merchandised.
pub fn gamepass(
    inputs: &Inputs,
    product_ids: &[String],
    merchandised: Option<&BTreeSet<String>>,
    tiers: &gamepass::Tiers,
    held: &[&str],
) -> Vec<LibraryRow> {
    let mut rows: Vec<LibraryRow> = product_ids
        .iter()
        .filter(|id| inputs.ownership.owns(id) != Some(true))
        // The tier feed marks add-ons as included as well; an expansion is not
        // a Game Pass game. Unknown kinds (older cache entries) stay listed.
        .filter(|id| {
            inputs
                .catalog
                .get(&id.to_ascii_uppercase())
                .and_then(|p| p.product_kind.as_deref())
                .map_or(true, |kind| kind.eq_ignore_ascii_case("Game"))
        })
        .map(|product_id| {
            let key = product_id.to_ascii_uppercase();
            let product = inputs.catalog.get(&key);
            let recipe = inputs
                .recipes
                .iter()
                .find(|recipe| recipe.matches(product_id));
            let compatibility = recipe.or_else(|| {
                product?
                    .bundled_ids
                    .iter()
                    .find_map(|id| inputs.recipes.iter().find(|recipe| recipe.matches(id)))
            });
            let name = product
                .map(|p| p.name.clone())
                .or_else(|| recipe.map(|recipe| recipe.title.name.clone()))
                .unwrap_or_else(|| product_id.clone());
            let installed = (inputs.installed_product)(product_id);
            let installing = (inputs.installing)(product_id);
            let incomplete = !installed && !installing && (inputs.incomplete)(product_id);
            let runnable = product.and_then(Product::is_runnable_here);

            // Said before the download, not after it. A title outside the
            // account's tier fails at the licence step with "not entitled to
            // this content" -- after several minutes and a partial container --
            // and the catalogue knew all along.
            let covered = if installing || installed {
                None
            } else {
                tiers.covered(product_id, held)
            };
            let mut parts = vec![match covered {
                Some(false) => "Not in your Game Pass tier".to_string(),
                _ => "Included with PC Game Pass".to_string(),
            }];
            if installed {
                parts.insert(0, installed_version_or(inputs.records.get(&key), product));
            } else if incomplete {
                parts.insert(0, INCOMPLETE.into());
            }
            if let Some(note) = package_note(inputs, product) {
                parts.push(note);
            }
            if let Some(recipe) = compatibility {
                if !recipe.status.summary.is_empty() {
                    parts.push(recipe.status.summary.clone());
                }
            }
            let action = match (installed, runnable) {
                _ if installing => {
                    parts.push("installing".into());
                    Action::Installing
                }
                (true, _) => {
                    if (inputs.product_describes_itself)(product_id) {
                        Action::Play
                    } else {
                        Action::Adopt
                    }
                }
                (false, _) if incomplete => Action::Resume,
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
                badge: Some(state_label(
                    compatibility
                        .map(|recipe| recipe.status.state)
                        .unwrap_or(TitleState::Untested),
                )),
                image: product.and_then(|p| p.image.clone()),
                owned: false,
                has_recipe: recipe.is_some(),
                installed,
                update: None,
                // Not installable from the Store here: no PC package and not a
                // bundle of one (EA app titles), or a tier-feed addition the
                // catalog has not confirmed as a game with a PC package.
                unsupported: runnable == Some(false)
                    || product.is_some_and(|p| !p.has_pc_package && p.bundled_ids.is_empty())
                    || (merchandised.is_some_and(|m| !m.contains(&key))
                        && !product.is_some_and(|p| p.has_pc_package)),
                outside_tier: covered == Some(false),
                unavailable: None,
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

/// The single size phrase for the details line in every section. A catalog
/// download size is never presented as measured disk use.
pub fn game_size_label(
    installed: bool,
    measured: Option<Option<u64>>,
    download: Option<u64>,
) -> String {
    if installed {
        match measured {
            Some(Some(bytes)) => format!("{} on disk", ferestre_core::human_bytes(bytes)),
            Some(None) => "Disk usage unavailable".to_string(),
            None => "Measuring disk usage…".to_string(),
        }
    } else {
        match download.filter(|bytes| *bytes > 0) {
            Some(bytes) => format!("{} download", ferestre_core::human_bytes(bytes)),
            None => "Size unavailable".to_string(),
        }
    }
}

/// Identify how the account can access a title. A public catalog entry alone
/// is not a subscription grant. Purchases take precedence over subscriptions;
/// a full subscription takes precedence over an old trial entry.
pub fn access_source(
    product_id: &str,
    ownership: &Ownership,
    catalog: &BTreeMap<String, Product>,
    gamepass_ids: &[String],
    tiers: &gamepass::Tiers,
    held: &[&str],
) -> &'static str {
    let related = |id: &str| {
        id.eq_ignore_ascii_case(product_id)
            || catalog.get(&id.to_ascii_uppercase()).is_some_and(|p| {
                p.bundled_ids
                    .iter()
                    .any(|child| child.eq_ignore_ascii_case(product_id))
            })
    };
    if let Ownership::Known(entries) = ownership {
        if let Some(entry) = entries.iter().find(|entry| {
            entry.is_title()
                && related(&entry.product_id)
                && !entry
                    .sku_type
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("Trial"))
        }) {
            return match entry.ownership_type.as_deref() {
                Some("OwnedByBeneficiary") | None => "Purchased",
                Some("Shared") | Some("SharedWithBeneficiary") => "Shared licence",
                _ => "Owned",
            };
        }
    }
    if tiers.covered(product_id, held) == Some(true)
        || gamepass_ids
            .iter()
            .filter(|id| related(id))
            .any(|id| tiers.covered(id, held) == Some(true))
    {
        return "Game Pass";
    }
    if let Ownership::Known(entries) = ownership {
        if entries.iter().any(|e| {
            e.is_title()
                && related(&e.product_id)
                && e.sku_type
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("Trial"))
        }) {
            return "Trial";
        }
    }
    if gamepass_ids.iter().any(|id| related(id)) {
        return GAME_PASS_CATALOGUE;
    }
    ACCESS_NOT_CHECKED
}

const GAME_PASS_CATALOGUE: &str = "Game Pass catalogue";
const ACCESS_NOT_CHECKED: &str = "Access not checked";

/// Why the Store account cannot be expected to license a title it reached
/// through Game Pass. Absence from the collections listing is deliberately not
/// one of these: that listing is not exhaustive (see
/// `missing_purchase_entries_do_not_claim_that_access_is_missing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionGap {
    /// A Game Pass title, and the account holds no subscription at all.
    NoSubscription,
    /// The account subscribes, but no tier it holds includes the title.
    OutsideTier,
}

/// Only said once the account's library was read: an unread library holds no
/// subscriptions, which is not the same as holding none.
pub fn subscription_gap(
    product_id: &str,
    ownership: &Ownership,
    catalog: &BTreeMap<String, Product>,
    gamepass_ids: &[String],
    tiers: &gamepass::Tiers,
    held: &[&str],
) -> Option<SubscriptionGap> {
    if !matches!(ownership, Ownership::Known(_))
        || access_source(product_id, ownership, catalog, gamepass_ids, tiers, held)
            != GAME_PASS_CATALOGUE
    {
        return None;
    }
    if held.is_empty() {
        return Some(SubscriptionGap::NoSubscription);
    }
    std::iter::once(product_id)
        .chain(gamepass_ids.iter().map(String::as_str))
        .filter(|id| {
            id.eq_ignore_ascii_case(product_id)
                || catalog.get(&id.to_ascii_uppercase()).is_some_and(|p| {
                    p.bundled_ids
                        .iter()
                        .any(|child| child.eq_ignore_ascii_case(product_id))
                })
        })
        .all(|id| tiers.covered(id, held) == Some(false))
        .then_some(SubscriptionGap::OutsideTier)
}

/// The licence step's own refusal, as `xodus-cli` reports it.
pub fn is_licence_refusal(reason: &str) -> bool {
    reason.starts_with("not entitled to this content")
}

/// Why `ferestre run` failed, from the tail of what it wrote to stderr. The
/// launcher's own refusals are `!! message`. A title stopped before it starts
/// -- at the licence step, for instance -- prints its reason last, before the
/// launcher's `-- the title exited` summary. Wine, Proton, driver and package
/// listing chatter is not a reason.
pub fn launch_failure(lines: &[String]) -> Option<String> {
    if let Some(refusal) = lines.iter().rev().find_map(|l| l.strip_prefix("!! ")) {
        return Some(refusal.trim().to_string());
    }
    lines
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !launch_noise(line))
        .map(str::to_string)
}

fn launch_noise(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "::", "-- ", "=== ", "WARNING:", "ntsync:", "Proton:", "wine:", "fsync:", "esync:",
    ];
    const WINE_DEBUG: &[&str] = &[":trace:", ":fixme:", ":err:", ":warn:"];
    // The client lists the package files it maps: `Logo.png: 1097007104 46926`.
    let listing = line.rsplit_once(": ").is_some_and(|(_, sizes)| {
        let mut sizes = sizes.split(' ');
        sizes.next().is_some_and(|s| s.parse::<u64>().is_ok())
            && sizes.next().is_some_and(|s| s.parse::<u64>().is_ok())
            && sizes.next().is_none()
    });
    line.is_empty()
        || listing
        || PREFIXES.iter().any(|p| line.starts_with(p))
        || WINE_DEBUG.iter().any(|d| line.contains(d))
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
    /// Installed, and the selected Store account cannot license it: its launch
    /// was refused this session, or it is a Game Pass title outside what the
    /// account subscribes to. Set by the view, which knows the account, to
    /// the sentence that says so.
    pub unavailable: Option<String>,
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

/// Every title worth a row: everything owned, plus everything described once
/// an account has supplied a library, plus what is installed on disk.
///
/// The union rather than the intersection, deliberately. A recipe for something
/// this account does not own still belongs on screen -- it is how someone finds
/// out the launcher supports a game before buying it -- and an owned title with
/// no recipe is the most useful row in the list, because it is the one someone
/// can do something about.
pub fn library(inputs: &Inputs) -> Vec<LibraryRow> {
    let mut rows: BTreeMap<String, LibraryRow> = BTreeMap::new();

    // A packaged recipe is a compatibility description, not proof that this
    // machine owns or installed the game. Showing all of them before sign-in
    // made a fresh AppImage look like it had populated somebody's library.
    // Keep a recipe-backed row for an on-disk record, and otherwise wait for
    // the account listing that turns a catalogue of support into a library.
    for recipe in inputs.recipes {
        let key = recipe.title.product_id.to_ascii_uppercase();
        if !matches!(inputs.ownership, Ownership::Unknown)
            || (inputs.installed)(recipe)
            || inputs.records.contains_key(&key)
        {
            rows.insert(key.clone(), described_row(inputs, recipe, &key));
        }
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
        .then_with(|| a.product_id.cmp(&b.product_id))
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
        (Action::Resume, _) => join(Some(INCOMPLETE.into()), &view.subtitle),
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
        unavailable: None,
        install_as: None,
        action,
    }
}

/// What a row says about a download that stopped before it finished.
const INCOMPLETE: &str = "Download incomplete: resume it, or remove what arrived";

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
        // A record and a catalog entry, and still no answer: checking could not
        // reach the authenticated service. Silence would read as "fine".
        None => format!("{version}  ·  update service unavailable"),
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
        unavailable: None,
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
                    "Bundle of {}  ·  installs {}",
                    plural(children.len(), "title"),
                    child.name,
                ),
                badge: None,
                image: product.and_then(|p| p.image.clone()),
                owned: true,
                has_recipe: false,
                installed: on_disk,
                update: None,
                unsupported: false,
                outside_tier: false,
                unavailable: None,
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
            unavailable: None,
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
            match p.publisher.as_deref() {
                Some(publisher) => format!("{publisher}  ·  {tail}"),
                None => {
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
    let incomplete = !on_disk && !installing && (inputs.incomplete)(product_id);
    let subtitle = if on_disk {
        installed_status(inputs.records.get(key), product, None, None)
    } else if incomplete {
        join(Some(INCOMPLETE.into()), &subtitle)
    } else {
        subtitle
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
        unavailable: None,
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
            (false, _) if incomplete => Action::Resume,
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
    let normalize = |text: &str| {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let needle = normalize(query);
    let terms: Vec<&str> = needle.split_whitespace().collect();
    rows.iter()
        .filter(|row| {
            let name = normalize(&row.name);
            let id = row.product_id.to_lowercase();
            terms
                .iter()
                .all(|term| name.contains(term) || id.contains(term))
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

/// What the client says when its token store holds no sign-in at all. Ferestre's
/// own client patch (0003, `xsts_for`) prints it, with exit status 1, for both
/// the device and the user token.
const SIGNED_OUT_MARKER: &str = "run `xodus-cli login` first";

/// Whether a failed `xodus-cli account --json` means nobody is signed in.
///
/// Anything else that fails -- no network, a service that is down, a token the
/// service refused -- is not evidence either way, and the saved account stays.
/// Only the empty token store is: it is the one failure that says something
/// about who is signed in rather than about whether the service answered.
pub fn client_reports_signed_out(stderr: &str) -> bool {
    stderr.contains(SIGNED_OUT_MARKER)
}

/// Why a launch is refused before it starts when the Store and Player
/// accounts differ, or `None` to let it go ahead.
///
/// Sharing an account needs a runtime that provides it, and the window can
/// only tell which runtime a launch will use when it has the title's recipe.
/// Without one -- a title on disk that describes itself, whose recipe `run`
/// writes on the way past -- the check is left to `ferestre run`, which makes
/// it with the recipe in hand and says precisely what is missing. Refusing
/// here instead sent people to a runtime menu that could never help.
pub fn account_sharing_refusal(has_recipe: bool, runtime_shares: bool) -> Option<&'static str> {
    (has_recipe && !runtime_shares).then_some(
        "Select an account-sharing runtime for this game using its runtime button, then try again.",
    )
}

/// What is in a folder named as an install destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Missing,
    NotADirectory,
    Empty,
    /// This product's own install: its record names the folder, or the
    /// package in it says it is this product.
    This,
    /// Only what a stopped download leaves before the package says what it is.
    Download,
    /// Anything else: other games, or somebody's files.
    Other,
}

/// Expand a leading `~` the way a shell would. `~user` is not supported.
fn expand_home(text: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let rest = match text.strip_prefix('~') {
        None => return Ok(PathBuf::from(text)),
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.trim_start_matches('/'),
        Some(_) => {
            return Err("Write the folder as a full path, like ~/Games or /mnt/games.".into())
        }
    };
    let home = home.ok_or("HOME is not set, so ~ cannot be expanded. Use a full path.")?;
    Ok(if rest.is_empty() {
        home.to_path_buf()
    } else {
        home.join(rest)
    })
}

/// Where an install goes, from what was typed in "Install to".
///
/// The field names a library folder, the way other launchers do: the title is
/// installed into its own folder inside it, `folder_name`, so a library holding
/// other games is never the title's folder -- and never what Remove deletes.
/// Naming a title's own existing folder still works, for a repair or an
/// update. `~` is expanded, and anything that is not a full path is refused
/// rather than resolved against whatever directory the window started in.
pub fn install_destination(
    typed: &str,
    home: Option<&Path>,
    folder_name: &Path,
    inspect: &dyn Fn(&Path) -> Folder,
) -> Result<PathBuf, String> {
    let text = typed.trim();
    if text.is_empty() {
        return Err("Choose the folder to install into.".into());
    }
    let chosen = expand_home(text, home)?;
    if !chosen.is_absolute() {
        return Err("Write the folder as a full path, like ~/Games or /mnt/games.".into());
    }
    if chosen
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Write the folder without \"..\" in it.".into());
    }
    // Components, so `/mnt/games/` and `/mnt/./games` are the same folder.
    let chosen: PathBuf = chosen.components().collect();
    match inspect(&chosen) {
        Folder::This => return Ok(chosen),
        Folder::NotADirectory => {
            return Err(format!("{} is a file, not a folder.", chosen.display()))
        }
        _ => {}
    }
    let destination = chosen.join(folder_name);
    match inspect(&destination) {
        Folder::Missing | Folder::Empty | Folder::This | Folder::Download => Ok(destination),
        Folder::NotADirectory => Err(format!(
            "{} is a file, not a folder.",
            destination.display()
        )),
        Folder::Other => Err(format!(
            "{} already holds other files. Choose another folder.",
            destination.display()
        )),
    }
}

/// How long to leave a background check that failed before trying it again
/// on its own. A Refresh still tries at once.
pub fn retry_delay(failures: u32) -> std::time::Duration {
    let minutes = match failures {
        0 => 0,
        1 => 1,
        2 => 5,
        3 => 15,
        _ => 60,
    };
    std::time::Duration::from_secs(minutes * 60)
}

/// The ids in `wanted` not yet in `asked`, which records them as asked. So a
/// lookup that came back with nothing is not repeated on every redraw.
pub fn unasked(wanted: Vec<String>, asked: &mut BTreeSet<String>) -> Vec<String> {
    wanted
        .into_iter()
        .filter(|id| asked.insert(id.to_ascii_uppercase()))
        .collect()
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
            runtimes: runtime.map(std::slice::from_ref).unwrap_or(&[]),
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
            artwork: Default::default(),
            download_bytes: Some(2_490_064_896),
            last_update: None,
            content_ids: content_ids.iter().map(|s| s.to_string()).collect(),
            // A coherent record: it has a size and an MSIXVC format, so it has
            // a PC package. Deriving this from `content_ids` instead made the
            // fixture describe a title with a 2.5 GB download and no packages,
            // which the catalog would never return.
            has_packages: true,
            package_format: Some("MSIXVC".into()),
            wu_category_id: None,
            has_pc_package: true,
            bundled_ids: Vec::new(),
            product_kind: None,
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
    /// And where no installed engine shapes the recipe.
    const NO_GAME_DIR: &dyn Fn(&Recipe) -> Option<PathBuf> = &|_| None;
    /// And where an undescribed title is not on disk either.
    const NOT_ON_DISK: &dyn Fn(&str) -> bool = &|_| false;
    /// And so nothing on disk describes itself.
    const NOTHING_DESCRIBES_ITSELF: &dyn Fn(&str) -> bool = &|_| false;
    /// And nothing is being downloaded.
    const NOTHING_INSTALLING: &dyn Fn(&str) -> bool = &|_| false;
    /// And no download stopped halfway.
    const NOTHING_INCOMPLETE: &dyn Fn(&str) -> bool = &|_| false;
    /// And nobody chose a runtime for a title.
    const NO_SELECTION: &dyn Fn(&str) -> Option<PathBuf> = &|_| None;

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
        let own = Ownership::Known(entries(&["9ZZTESTGAME1"]));
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
        let own = Ownership::Known(entries(&["9ZZTESTGAME1"]));
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
            runtimes: std::slice::from_ref(&rt),
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
    fn missing_purchase_entries_do_not_claim_that_access_is_missing() {
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
            !row.subtitle.contains("library listing"),
            "absence is not a negative entitlement verdict: {}",
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
    fn access_sources_distinguish_purchases_game_pass_trials_and_unknown_access() {
        let id = "9ZZTESTGAME1";
        let ids = vec![id.to_string()];
        let tiers =
            gamepass::parse_tiers(r#"{"9ZZTESTGAME1":{"PCSubMetadata":{"Included":true}}}"#)
                .unwrap();
        let empty = Ownership::Known(Vec::new());
        let cat = BTreeMap::new();
        assert_eq!(
            access_source(id, &empty, &cat, &ids, &tiers, &["PC Game Pass"]),
            "Game Pass"
        );
        assert_eq!(
            access_source(id, &empty, &cat, &ids, &tiers, &[]),
            "Game Pass catalogue"
        );
        assert_eq!(
            access_source(id, &empty, &cat, &[], &tiers, &[]),
            "Access not checked"
        );
        let owned = Ownership::Known(entries(&[id]));
        assert_eq!(
            access_source(id, &owned, &cat, &ids, &tiers, &["PC Game Pass"]),
            "Purchased"
        );
        let mut trial = entries(&[id]);
        trial[0].sku_type = Some("Trial".into());
        let trial = Ownership::Known(trial);
        assert_eq!(access_source(id, &trial, &cat, &ids, &tiers, &[]), "Trial");
        assert_eq!(
            access_source(id, &trial, &cat, &ids, &tiers, &["PC Game Pass"]),
            "Game Pass"
        );
        let mut parent = product("9ZZTESTBUNDL", "Fixture bundle", &[]);
        parent.bundled_ids = vec![id.into()];
        let cat = catalog(vec![parent]);
        let owned = Ownership::Known(entries(&["9ZZTESTBUNDL"]));
        assert_eq!(
            access_source(id, &owned, &cat, &[], &tiers, &[]),
            "Purchased"
        );
    }

    #[test]
    fn installed_titles_say_when_the_store_account_has_no_access() {
        let id = "9ZZTESTGAME1";
        let ids = vec![id.to_string()];
        let tiers =
            gamepass::parse_tiers(r#"{"9ZZTESTGAME1":{"PCSubMetadata":{"Included":true}}}"#)
                .unwrap();
        let cat = BTreeMap::new();
        let empty = Ownership::Known(Vec::new());
        let gap = |ownership, ids: &[String], tiers, held: &[&str]| {
            subscription_gap(id, ownership, &cat, ids, tiers, held)
        };
        assert_eq!(
            gap(&empty, &ids, &tiers, &[]),
            Some(SubscriptionGap::NoSubscription)
        );
        let console =
            gamepass::parse_tiers(r#"{"9ZZTESTGAME1":{"ConsoleSubMetadata":{"Included":true}}}"#)
                .unwrap();
        assert_eq!(
            gap(&empty, &ids, &console, &["PC Game Pass"]),
            Some(SubscriptionGap::OutsideTier)
        );
        // Covered, purchased, unread, or merely absent from the listing.
        assert_eq!(gap(&empty, &ids, &tiers, &["PC Game Pass"]), None);
        let owned = Ownership::Known(entries(&[id]));
        assert_eq!(gap(&owned, &ids, &tiers, &[]), None);
        assert_eq!(gap(&Ownership::Unknown, &ids, &tiers, &[]), None);
        assert_eq!(gap(&empty, &[], &tiers, &[]), None);
        // A subscriber whose tier data has not arrived may still be covered.
        let unknown = gamepass::Tiers::default();
        assert_eq!(gap(&empty, &ids, &unknown, &["PC Game Pass"]), None);
        assert!(is_licence_refusal(
            "not entitled to this content: Users do not possess any satisfying entitlements"
        ));
        assert!(!is_licence_refusal("Player account is signed out"));
    }

    #[test]
    fn launch_failures_report_the_reason_not_the_chatter() {
        let lines = |text: &str| text.lines().map(str::to_string).collect::<Vec<_>>();
        let refused = lines(
            ":: using the selected Store and Player accounts\n\
             Logo.png: 1097007104 46926\n\
             not entitled to this content: Users do not possess any satisfying entitlements for the content id in question.\n\
             -- the title exited 1; the log is at /games/fixture-launch.log",
        );
        assert_eq!(
            launch_failure(&refused).as_deref(),
            Some("not entitled to this content: Users do not possess any satisfying entitlements for the content id in question.")
        );
        let launcher = lines(
            ":: using the selected Store and Player accounts\n\
             !! Player account is signed out or unavailable. Open Accounts and sign in again.\n   caused by",
        );
        assert_eq!(
            launch_failure(&launcher).as_deref(),
            Some("Player account is signed out or unavailable. Open Accounts and sign in again.")
        );
        let chatter = lines(
            "Proton: Error while copying to \"C:/x.dll\": No such file or directory\n\
             WARNING: radv is not a conformant Vulkan implementation, testing use only.\n\
             00e4:err:openxr:get_vulkan_extensions Could not create key, status 0x2.\n\
             -- the title exited 3; the log is at /games/fixture-launch.log",
        );
        assert_eq!(launch_failure(&chatter), None);
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
            runtimes: std::slice::from_ref(&rt),
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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

    #[test]
    fn an_unsigned_machine_shows_only_titles_that_are_actually_installed() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let record = record("9ZZTESTDISK2", &["content-a"]);
        let records = [(record.product_id.clone(), record)].into();
        let catalog = BTreeMap::new();
        let available = BTreeMap::new();
        let unknown = Ownership::Unknown;
        let rows = library(&Inputs {
            recipes: &recipes,
            runtimes: &[],
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
            registry: None,
            ownership: &unknown,
            catalog: &catalog,
            records: &records,
            available: &available,
            installed: NOTHING_INSTALLED,
            installed_version: NO_VERSION_ON_DISK,
            installed_product: NOT_ON_DISK,
            product_describes_itself: NOTHING_DESCRIBES_ITSELF,
            installing: NOTHING_INSTALLING,
            running: NOTHING_RUNNING,
            game_dir: NO_GAME_DIR,
        });
        assert_eq!(
            rows.iter()
                .map(|row| row.product_id.as_str())
                .collect::<Vec<_>>(),
            ["9ZZTESTDISK2"],
            "packaged recipes describe support; they are not this machine's library"
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
            runtimes: std::slice::from_ref(&rt),
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
            runtimes: &[],
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
            product_kind: None,
            ..product("9ZZTESTBNDL", "A Bundle", &[])
        };
        let child = product("9ZZTESTCHLD", "The Playable One", &["cid"]);
        let own = Ownership::Known(entries(&["9ZZTESTBNDL"]));
        let cat = catalog(vec![bundle, child]);
        let rows = library(&Inputs {
            recipes: &[],
            runtimes: &[],
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
            !row.subtitle.contains("2.5 GB"),
            "the shared details line supplies the child's size once: {}",
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
            product_kind: None,
            ..product("9ZZTESTBNDL", "A Bundle", &[])
        };
        let own = Ownership::Known(entries(&["9ZZTESTBNDL"]));
        let cat = catalog(vec![bundle]);
        let rows = library(&Inputs {
            recipes: &[],
            runtimes: &[],
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
                runtimes: &[],
                selected_runtime: NO_SELECTION,
                incomplete: NOTHING_INCOMPLETE,
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
                game_dir: NO_GAME_DIR,
            },
            &listing,
            None,
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
            !runnable.subtitle.contains("2.5 GB"),
            "download size is supplied once by the shared details line: {}",
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
            runtimes: &[],
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
        });
        assert_eq!(rows.len(), 1, "the row exists at all");
        assert_eq!(rows[0].name, "Only On Disk");
        assert!(rows[0].installed, "and it shows up under Installed");
        assert!(
            rows[0].subtitle.starts_with("Version 1.26.4501.0"),
            "installed titles without recipes still show their version"
        );
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
            runtimes: &[],
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
        });
        assert_eq!(rows[0].action, Action::Play);

        let rows = library(&Inputs {
            recipes: &[],
            ownership: &Ownership::Unknown,
            runtimes: &[],
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
    fn gamepass_search_results_report_compatibility_without_claiming_unknown_games_work() {
        let recipes = vec![
            recipe("9ZZTESTGAME1", "playable", &[]),
            recipe("9ZZTESTGAME2", "broken", &[]),
        ];
        let rows = gamepass(
            &inputs(&recipes, None, &Ownership::Unknown, NOTHING_INSTALLED),
            &[
                "9ZZTESTGAME1".into(),
                "9ZZTESTGAME2".into(),
                "9ZZTESTGAME3".into(),
            ],
            None,
            &gamepass::Tiers::default(),
            &[],
        );
        let by_id = |id: &str| rows.iter().find(|row| row.product_id == id).unwrap();
        assert_eq!(by_id("9ZZTESTGAME1").badge.unwrap().0, "Playable");
        assert_eq!(by_id("9ZZTESTGAME2").badge.unwrap().0, "Does not run");
        assert_eq!(by_id("9ZZTESTGAME3").badge.unwrap().0, "Untested");
        assert!(by_id("9ZZTESTGAME1").subtitle.contains("a summary"));
        assert_eq!(search(&rows, "game1")[0].badge.unwrap().0, "Playable");
    }

    /// The count above the Game Pass list is the rows it shows, so the rows
    /// must be games this launcher can install: an add-on, an EA app title
    /// and an unconfirmed tier-feed addition are not.
    #[test]
    fn the_gamepass_list_holds_installable_games_only() {
        let mut addon = product("9ZZTESTADDON", "Expansion", &["cid-a"]);
        addon.product_kind = Some("Durable".into());
        let mut ea = product("9ZZTESTEAAPP", "EA App Title", &[]);
        ea.has_packages = false;
        ea.has_pc_package = false;
        ea.package_format = None;
        let mut supplement = product("9ZZTESTSUPPL", "Tier Feed Game", &["cid-s"]);
        supplement.product_kind = Some("Game".into());
        let cat = catalog(vec![
            product("9ZZTESTGAME1", "Listed Game", &["cid-1"]),
            addon,
            ea,
            supplement,
        ]);
        let ids: Vec<String> = [
            "9ZZTESTGAME1",
            "9ZZTESTADDON",
            "9ZZTESTEAAPP",
            "9ZZTESTSUPPL",
            "9ZZTESTUNSEEN",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let merchandised: BTreeSet<String> = ["9ZZTESTGAME1", "9ZZTESTADDON", "9ZZTESTEAAPP"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut fixture = inputs(&[], None, &Ownership::Unknown, NOTHING_INSTALLED);
        fixture.catalog = &cat;
        let rows = gamepass(
            &fixture,
            &ids,
            Some(&merchandised),
            &gamepass::Tiers::default(),
            &[],
        );
        let listed: Vec<&str> = rows
            .iter()
            .filter(|row| !row.known_uninstallable())
            .map(|row| row.product_id.as_str())
            .collect();
        assert_eq!(listed, vec!["9ZZTESTGAME1", "9ZZTESTSUPPL"]);
        assert!(
            !rows.iter().any(|row| row.product_id == "9ZZTESTADDON"),
            "an add-on is not a Game Pass game at all"
        );
        assert!(
            rows.iter()
                .any(|row| row.product_id == "9ZZTESTUNSEEN" && row.unsupported),
            "an unconfirmed addition waits behind the switch, still asked about"
        );
    }

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
                runtimes: &[],
                selected_runtime: NO_SELECTION,
                incomplete: NOTHING_INCOMPLETE,
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
                game_dir: NO_GAME_DIR,
            },
            &listing,
            None,
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
            runtimes: &[],
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
        });
        assert_eq!(rows[0].action, Action::Installing);
        assert_eq!(rows[0].action.label(), "Installing..");
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
            runtimes: std::slice::from_ref(&rt),
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
            runtimes: std::slice::from_ref(&rt),
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
            runtimes: std::slice::from_ref(&rt),
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
        let service_unavailable = installed_status(
            Some(&record("9ZZTESTGAME1", &["content-a"])),
            Some(&product("9ZZTESTGAME1", "T", &[])),
            None,
            None,
        );

        assert!(up_to_date.contains("up to date"), "{up_to_date}");
        assert!(stale.contains("update available"), "{stale}");
        assert!(unrecorded.contains("cannot be checked"), "{unrecorded}");
        assert!(
            service_unavailable.contains("service unavailable"),
            "{service_unavailable}"
        );

        let all = [&up_to_date, &stale, &unrecorded, &service_unavailable];
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
            runtimes: std::slice::from_ref(&rt),
            selected_runtime: NO_SELECTION,
            incomplete: NOTHING_INCOMPLETE,
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
            game_dir: NO_GAME_DIR,
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
                unavailable: None,
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
    fn one_download_disables_install_and_update_but_allows_play() {
        for action in [Action::Install, Action::Update] {
            assert!(action.is_enabled_during_download(false));
            assert!(!action.is_enabled_during_download(true));
        }
        assert!(Action::Play.is_enabled_during_download(true));
        assert!(Action::Stop.is_enabled_during_download(true));
        assert!(!Action::Installing.is_enabled_during_download(true));
        assert_eq!(Action::Installing.label(), "Installing..");
    }

    #[test]
    fn search_handles_words_and_punctuation() {
        let rows = rows_named(&[("9NR1R1XWLCNB", "Forza Horizon 6: Standard Edition")]);
        for query in ["FORZA   6", "horizon forza", "6 standard", "forza-horizon"] {
            assert_eq!(search(&rows, query).len(), 1, "{query}");
        }
        assert!(search(&rows, "forza 5").is_empty());
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

    /// A client with nothing in its token store says so on stderr. A client
    /// that could not reach the service, or panicked on a refused token, says
    /// something else, and that must not sign the window out.
    #[test]
    fn only_an_empty_token_store_reads_as_signed_out() {
        assert!(client_reports_signed_out(
            "no usable device token; run `xodus-cli login` first\n"
        ));
        assert!(client_reports_signed_out(
            "no usable user token; run `xodus-cli login` first"
        ));
        assert!(!client_reports_signed_out(""));
        assert!(!client_reports_signed_out(
            "thread 'main' panicked: Failed to authenticate Xbox user: MSA user token exchange failed: error sending request"
        ));
        assert!(!client_reports_signed_out("profile settings: HTTP 503"));
    }

    // --- the runtime a row is judged against --------------------------------

    fn runtime_at(path: &str, version: &str, caps: &[&str]) -> InstalledRuntime {
        InstalledRuntime {
            path: PathBuf::from(path),
            version: Some(version.into()),
            provides: caps.iter().map(|c| c.to_string()).collect::<BTreeSet<_>>(),
            source: CapabilitySource::Manifest,
        }
    }

    fn row_with(
        recipes: &[Recipe],
        runtimes: &[InstalledRuntime],
        selected: &dyn Fn(&str) -> Option<PathBuf>,
    ) -> LibraryRow {
        let own = Ownership::Unknown;
        library(&Inputs {
            runtimes,
            selected_runtime: selected,
            ..inputs(recipes, None, &own, ALL_INSTALLED)
        })
        .remove(0)
    }

    /// The newest runtime lost a capability the recipe needs, and an older one
    /// still has it. `ferestre run` picks the older one, so the row plays; it
    /// used to be blocked because only the newest was ever assessed.
    #[test]
    fn a_row_is_judged_against_the_runtime_its_launch_will_use() {
        let recipes = vec![recipe(
            "9ZZTESTGAME1",
            "playable",
            &["loader.memfd-main-image"],
        )];
        let runtimes = vec![
            runtime_at("/runtimes/new", "11", &[]),
            runtime_at("/runtimes/old", "10", &["loader.memfd-main-image"]),
        ];
        assert_eq!(
            row_with(&recipes, &runtimes, NO_SELECTION).action,
            Action::Play
        );

        // Chosen for the title: that one, and only that one, decides.
        let new: &dyn Fn(&str) -> Option<PathBuf> = &|_| Some(PathBuf::from("/runtimes/new"));
        assert!(
            matches!(
                row_with(&recipes, &runtimes, new).action,
                Action::Blocked(_)
            ),
            "the chosen runtime lacks what the recipe requires"
        );
        let old: &dyn Fn(&str) -> Option<PathBuf> = &|_| Some(PathBuf::from("/runtimes/old"));
        assert_eq!(row_with(&recipes, &runtimes, old).action, Action::Play);

        // Chosen, then removed: the launch would be refused, so the row says so.
        let gone: &dyn Fn(&str) -> Option<PathBuf> = &|_| Some(PathBuf::from("/runtimes/gone"));
        let Action::Blocked(why) = row_with(&recipes, &runtimes, gone).action else {
            panic!("a chosen runtime that is gone must block");
        };
        assert!(why.contains("no longer installed"), "{why}");
    }

    #[test]
    fn with_no_runtime_satisfying_it_the_newest_explains_what_is_missing() {
        let recipes = [recipe(
            "9ZZTESTGAME1",
            "playable",
            &["loader.memfd-main-image"],
        )];
        let runtimes = vec![
            runtime_at("/runtimes/new", "11", &[]),
            runtime_at("/runtimes/old", "10", &[]),
        ];
        let (chosen, assessment) =
            launch_runtime(&recipes[0], &runtimes, None, None).expect("a runtime to judge by");
        assert_eq!(chosen.path, PathBuf::from("/runtimes/new"));
        assert!(!assessment.is_satisfied());
        assert!(launch_runtime(&recipes[0], &[], None, None).is_err());
    }

    // --- downloads that stopped ---------------------------------------------

    /// A stopped download leaves a folder with the package header and the
    /// manifest in it. That is not an installed title: Play would launch half
    /// a game. The row offers to resume instead.
    #[test]
    fn a_stopped_download_offers_to_resume_not_to_play() {
        let recipes = vec![recipe("9ZZTESTGAME1", "playable", &[])];
        let rt = runtime_with(&[], CapabilitySource::Manifest);
        let own = Ownership::Known(entries(&["9ZZTESTGAME1", "9ZZNORECIPE1"]));
        let incomplete: &dyn Fn(&str) -> bool = &|_| true;
        let rows = library(&Inputs {
            incomplete,
            ..inputs(&recipes, Some(&rt), &own, NOTHING_INSTALLED)
        });
        for row in &rows {
            assert_eq!(row.action, Action::Resume, "{}", row.product_id);
            assert_eq!(row.action.label(), "Resume");
            assert_eq!(row.action.command("X"), Some(["install", "X"]));
            assert!(!row.installed, "{} is not installed", row.product_id);
            assert!(
                row.subtitle.contains("Download incomplete"),
                "{}",
                row.subtitle
            );
            assert!(
                !row.action.is_enabled_during_download(true),
                "one download at a time"
            );
        }
    }

    // --- launching with separate Store and Player accounts -------------------

    /// A title with no recipe cannot be checked here, because which runtime a
    /// launch uses depends on the recipe. `ferestre run` writes one and checks
    /// precisely; refusing here meant it never got the chance.
    #[test]
    fn a_title_without_a_recipe_is_left_for_the_launcher_to_check() {
        assert_eq!(account_sharing_refusal(false, false), None);
        assert!(account_sharing_refusal(true, false).is_some());
        assert_eq!(account_sharing_refusal(true, true), None);
    }

    // --- where an install goes ----------------------------------------------

    fn fake_disk<'a>(disk: &'a [(&'a str, Folder)]) -> impl Fn(&Path) -> Folder + 'a {
        move |path| {
            disk.iter()
                .find(|(p, _)| Path::new(p) == path)
                .map_or(Folder::Missing, |(_, f)| *f)
        }
    }

    #[test]
    fn install_to_names_a_library_and_the_title_gets_a_folder_in_it() {
        let disk = [("/mnt/games", Folder::Other)];
        let inspect = fake_disk(&disk);
        let home = Some(Path::new("/home/u"));
        let fh6 = Path::new("forza-horizon-6");
        assert_eq!(
            install_destination("/mnt/games", home, fh6, &inspect),
            Ok(PathBuf::from("/mnt/games/forza-horizon-6")),
            "a library holding other games is never the title's own folder"
        );
        assert_eq!(
            install_destination("  /mnt/games/  ", home, fh6, &inspect),
            Ok(PathBuf::from("/mnt/games/forza-horizon-6"))
        );
        assert_eq!(
            install_destination("~/Games", home, Path::new("bedrock/game"), &inspect),
            Ok(PathBuf::from("/home/u/Games/bedrock/game")),
            "~ is expanded, and a recipe's nested layout is kept"
        );
        assert_eq!(
            install_destination("~", home, fh6, &inspect),
            Ok(PathBuf::from("/home/u/forza-horizon-6"))
        );
    }

    #[test]
    fn install_to_refuses_what_it_cannot_use_safely() {
        let disk = [
            ("/mnt/games/forza-horizon-6", Folder::Other),
            ("/mnt/file", Folder::NotADirectory),
        ];
        let inspect = fake_disk(&disk);
        let home = Some(Path::new("/home/u"));
        let fh6 = Path::new("forza-horizon-6");
        for typed in [
            "Games",
            "./Games",
            "",
            "   ",
            "~other/Games",
            "/mnt/../etc",
            "/mnt/file",
        ] {
            assert!(
                install_destination(typed, home, fh6, &inspect).is_err(),
                "{typed:?} must be refused"
            );
        }
        assert!(install_destination("~/Games", None, fh6, &inspect).is_err());
        let Err(why) = install_destination("/mnt/games", home, fh6, &inspect) else {
            panic!("someone else's files in the title's folder must be refused");
        };
        assert!(why.contains("already holds other files"), "{why}");
    }

    #[test]
    fn a_titles_own_folder_still_works_for_a_repair() {
        let disk = [
            ("/mnt/x/fh6", Folder::This),
            ("/mnt/y/forza-horizon-6", Folder::Download),
            ("/mnt/z/forza-horizon-6", Folder::Empty),
        ];
        let inspect = fake_disk(&disk);
        let fh6 = Path::new("forza-horizon-6");
        assert_eq!(
            install_destination("/mnt/x/fh6", None, fh6, &inspect),
            Ok(PathBuf::from("/mnt/x/fh6")),
            "the title's existing folder is used as it is"
        );
        assert_eq!(
            install_destination("/mnt/y", None, fh6, &inspect),
            Ok(PathBuf::from("/mnt/y/forza-horizon-6")),
            "a stopped download in the title's folder is resumed"
        );
        assert_eq!(
            install_destination("/mnt/z", None, fh6, &inspect),
            Ok(PathBuf::from("/mnt/z/forza-horizon-6"))
        );
    }

    // --- background retries and lookups --------------------------------------

    #[test]
    fn a_failing_background_check_waits_longer_each_time() {
        let delays: Vec<u64> = (0..6).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(delays, [0, 60, 300, 900, 3600, 3600]);
    }

    #[test]
    fn each_lookup_is_asked_once() {
        let mut asked = BTreeSet::new();
        assert_eq!(
            unasked(vec!["9abc".into(), "9DEF".into()], &mut asked),
            ["9abc", "9DEF"]
        );
        assert!(unasked(vec!["9ABC".into(), "9def".into()], &mut asked).is_empty());
        assert_eq!(unasked(vec!["9GHI".into()], &mut asked), ["9GHI"]);
    }
}

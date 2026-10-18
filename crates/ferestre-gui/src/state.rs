//! What the window has loaded, and how it loads more.
//!
//! Split from the widgets because the two have different lifetimes: a rebuild
//! throws away every row and builds new ones, and none of what was fetched
//! should go with them. Anything that cost a network round trip -- the account,
//! the library, the catalog, the icons -- lives here and survives a redraw.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ferestre_core::account::Account;
use ferestre_core::accounts::{self, Profiles, Selection};
use ferestre_core::catalog::{self, Cache, Product};
use ferestre_core::gamepass;
use ferestre_core::install::{self, Record};
use ferestre_core::library;
use ferestre_core::paths::Paths;
use ferestre_core::recipe::Recipe;
use ferestre_core::runtime::{self, InstalledRuntime, Registry};

use crate::model::Ownership;

/// Icons are drawn at this size and fetched at it: the CDN resizes, so asking
/// for what will be shown turns a 1 MB download into 17 KB.
pub const ICON_PX: u32 = 64;
/// The gamerpic host serves only certain sizes; `account::supported_avatar_size`
/// is what guarantees this is one of them.
pub const AVATAR_PX: u32 = 64;
/// Rows per page. Enough that scrolling is normal, few enough that a page of
/// icons is a handful of requests rather than a hundred.
pub const PER_PAGE: usize = 20;

/// Which section of the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Library,
    GamePass,
    Installed,
    Updates,
    Runtime,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Library,
        Section::GamePass,
        Section::Installed,
        Section::Updates,
        Section::Runtime,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Library => "Library",
            Section::GamePass => "Game Pass",
            Section::Installed => "Installed",
            Section::Updates => "Updates",
            Section::Runtime => "Runtime",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Section::Library => "view-grid-symbolic",
            Section::GamePass => "emblem-shared-symbolic",
            Section::Installed => "drive-harddisk-symbolic",
            Section::Updates => "software-update-available-symbolic",
            Section::Runtime => "applications-engineering-symbolic",
        }
    }
}

pub struct Model {
    pub roles: Option<Selection>,
    /// Why `account-roles.json` could not be read, while it cannot. The rest
    /// of the window still works; what belongs to an account waits until the
    /// file is reset, because without it the window cannot tell whose it is.
    pub roles_error: Option<String>,
    pub profiles: Option<Profiles>,
    /// Titles whose licence the selected Store account was refused at launch
    /// this session, as uppercase product ids. Stronger than anything the
    /// collections listing implies, and cleared when the Store account changes.
    pub refused: BTreeSet<String>,
    pub paths: Option<Paths>,
    pub recipes: Vec<Recipe>,
    /// The automatic default, used for compatibility rows.
    pub runtime: Option<InstalledRuntime>,
    /// Every runtime the person can choose from on this machine.
    pub runtimes: Vec<InstalledRuntime>,
    pub registry: Option<Registry>,
    pub records: BTreeMap<String, Record>,
    pub disk_usage: BTreeMap<String, Option<u64>>,
    pub sizes_pending: BTreeSet<String>,
    pub size_generation: u64,
    /// Store product ids read from package metadata below `games_dir`, with
    /// the folder each is in. This is separate from records because a title
    /// can have been installed before Ferestre existed, or live in a
    /// human-named directory.
    pub installed_products: BTreeMap<String, PathBuf>,

    /// Fetched, not loaded: these survive a rebuild.
    pub ownership: Ownership,
    pub catalog: BTreeMap<String, Product>,
    pub icons: BTreeMap<String, PathBuf>,
    /// The version the service is offering per title. Nothing fills this yet --
    /// the anonymous catalog reports every version as "0" -- so every installed
    /// title reads "cannot be checked", which is true.
    pub available: BTreeMap<String, String>,
    pub account: Option<Account>,
    pub avatar: Option<PathBuf>,
    /// The PC Game Pass catalogue, as product ids, and the subscriptions this
    /// account actually holds. Separate on purpose: the catalogue is public and
    /// the entitlement is not, and a launcher that guesses the second from the
    /// first tells people they cannot install titles they can.
    pub gamepass: Vec<String>,
    /// The merchandised "All PC Games" ids within `gamepass`; the rest came
    /// from the tier feed and must be confirmed by the catalog to be listed.
    pub gamepass_merchandised: BTreeSet<String>,
    pub subscriptions: Vec<&'static str>,
    pub entitlements_checked: bool,
    /// Which subscription tiers include which products. Joined against
    /// `subscriptions` to warn before a download that would be refused.
    pub tiers: gamepass::Tiers,

    /// Where the view is, which is state the widgets must not own -- a rebuild
    /// destroys them and the person's place in the list should survive it.
    pub section: Section,
    pub query: String,
    pub page: usize,
    /// Whether to list titles whose package this runtime cannot open. Off by
    /// default because they are the overwhelming majority -- 95 of 102 on the
    /// development account -- and a list where the seven that work are buried
    /// among them is not a library, it is a haystack.
    pub show_unsupported: bool,

    pub problem: Option<String>,
}

impl Model {
    /// Entitlements and the tier summary must change together.
    pub fn set_ownership(&mut self, entries: Vec<library::Entry>) {
        self.subscriptions = gamepass::active(&entries);
        self.ownership = Ownership::Known(entries);
        self.entitlements_checked = true;
    }

    /// Nobody is signed in any more. The listing goes with the account:
    /// leaving it on screen, or on disk for the next window, would show a
    /// library belonging to nobody, with buttons that all fail at the licence
    /// step.
    pub fn forget_account(&mut self) {
        self.account = None;
        self.avatar = None;
        self.ownership = Ownership::Unknown;
        self.subscriptions.clear();
        self.refused.clear();
        self.entitlements_checked = false;
        if let Some(state) = self.private_state_dir() {
            forget_saved_account(&state);
        }
        if let Some(cache) = self.account_cache_dir() {
            forget_saved_avatar(&cache);
        }
    }

    pub fn load() -> Self {
        let mut problem = None;
        let paths = match Paths::from_env() {
            Ok(p) => Some(p),
            Err(e) => {
                problem = Some(format!("Cannot work out where things live: {e}"));
                None
            }
        };

        // An unreadable role file used to take the paths with it, which left
        // nothing working -- not even the Accounts dialog that could fix it.
        // Only what belongs to an account waits for it now.
        let mut roles_error = None;
        let roles = match paths
            .as_ref()
            .map(|p| accounts::load(p.config_dir()))
            .transpose()
        {
            Ok(roles) => roles.flatten(),
            Err(error) => {
                roles_error = Some(format!("{error:#}"));
                problem = Some(format!(
                    "Account roles could not be read ({error:#}). Reset them to choose the \
                     Store and Player accounts again."
                ));
                None
            }
        };
        let account_scoped = roles_error.is_none();
        let private_state = paths
            .as_ref()
            .map(|p| accounts::private_state(p.state_dir(), roles.as_ref()));
        let account_cache = paths
            .as_ref()
            .map(|p| accounts::private_state(p.cache_dir(), roles.as_ref()));
        let (recipes, skipped) = paths
            .as_ref()
            .map(|p| load_recipes(&p.title_dirs()))
            .unwrap_or_default();
        let registry = paths
            .as_ref()
            .and_then(Paths::titles_dir)
            .and_then(|dir| Registry::load(&dir.join("capabilities.toml")).ok());
        let runtimes = paths.as_ref().map(runtime::all).unwrap_or_default();
        let runtime = runtimes.first().cloned();
        let records: BTreeMap<String, Record> = paths
            .as_ref()
            .map(|p| {
                install::all(p.state_dir())
                    .into_iter()
                    // A record for a directory somebody deleted would keep the
                    // title in Installed and offer it an update.
                    .filter(|r| !r.is_stale())
                    .map(|r| (r.product_id.to_ascii_uppercase(), r))
                    .collect()
            })
            .unwrap_or_default();
        let records = adopt_existing(paths.as_ref(), &recipes, records);
        let installed_products = paths
            .as_ref()
            .map(|p| installed_products(p.games_dir()))
            .unwrap_or_default();

        // The last listing, and whatever the catalog already knows about it.
        // Both come off disk, so the window opens showing a library instead of
        // an empty page with a button on it; the button then means "check
        // again" rather than "start".
        let ownership = paths
            .as_ref()
            .filter(|_| account_scoped)
            .and_then(|_| library::load_cache(private_state.as_deref()?))
            .map(Ownership::Known)
            .unwrap_or_default();
        let mut catalog: BTreeMap<String, Product> = match (&paths, &ownership) {
            (Some(paths), _) => {
                let cache = paths.catalog_cache();
                let mut ids: Vec<String> = match &ownership {
                    Ownership::Known(entries) => entries
                        .iter()
                        .filter(|e| e.is_title())
                        .map(|e| e.product_id.clone())
                        .collect(),
                    Ownership::Unknown => Vec::new(),
                };
                ids.extend(recipes.iter().map(|r| r.title.product_id.clone()));
                ids.extend(records.keys().cloned());
                // Cache only: opening a window must not wait on a network.
                cache
                    .split(&ids)
                    .0
                    .into_iter()
                    .map(|p| (p.product_id.to_ascii_uppercase(), p))
                    .collect()
            }
            _ => BTreeMap::new(),
        };
        // Both off disk, like the library: a window that opens with an empty
        // Game Pass section until someone presses something has not shown them
        // their Game Pass.
        let market = paths
            .as_ref()
            .map(|p| Paths::market(p).0)
            .unwrap_or_else(|| catalog::DEFAULT_MARKET.to_string());
        let cached = paths
            .as_ref()
            .and_then(|p| gamepass::load_cache(p.state_dir(), &market));
        let (gamepass, gamepass_merchandised, tiers) = match cached {
            Some(listing) => (
                gamepass::complete_listing(&listing.product_ids, &listing.tiers),
                listing
                    .product_ids
                    .iter()
                    .map(|id| id.to_ascii_uppercase())
                    .collect(),
                listing.tiers,
            ),
            None => (Vec::new(), BTreeSet::new(), gamepass::Tiers::default()),
        };
        // Cached Game Pass rows must draw before background revalidation too.
        if let Some(paths) = &paths {
            catalog.extend(
                paths
                    .catalog_cache()
                    .split(&gamepass)
                    .0
                    .into_iter()
                    .map(|p| (p.product_id.to_ascii_uppercase(), p)),
            );
        }
        let subscriptions = match &ownership {
            Ownership::Known(entries) => gamepass::active(entries),
            Ownership::Unknown => Vec::new(),
        };

        let icons = paths
            .as_ref()
            .map(|p| {
                existing_icons(
                    &catalog::Cache::new(p.cache_dir().join("catalog")),
                    &catalog,
                )
            })
            .unwrap_or_default();
        if problem.is_none() {
            problem = match skipped.as_slice() {
                [] if recipes.is_empty() => Some(
                    "No title recipes found. Point FERESTRE_TITLES at the titles/ directory."
                        .into(),
                ),
                [] => None,
                [only] => Some(format!("A recipe file was skipped: {only}")),
                [first, rest @ ..] => Some(format!(
                    "{} recipe files were skipped. The first: {first}",
                    rest.len() + 1
                )),
            };
        }

        let account = paths
            .as_ref()
            .filter(|_| account_scoped)
            .and_then(|_| ferestre_core::account::load_cache(private_state.as_deref()?));
        let avatar = saved_avatar(account.as_ref(), account_cache.as_deref());
        let entitlements_checked = ownership.is_known();
        let available = paths
            .as_ref()
            .map(|_| {
                ferestre_core::updates::available(
                    private_state.as_deref().unwrap(),
                    records.values(),
                )
            })
            .unwrap_or_default();
        Model {
            roles,
            roles_error,
            profiles: None,
            refused: BTreeSet::new(),
            paths,
            recipes,
            runtime,
            runtimes,
            registry,
            records,
            disk_usage: BTreeMap::new(),
            sizes_pending: BTreeSet::new(),
            size_generation: 0,
            installed_products,
            ownership,
            catalog,
            icons,
            available,
            account,
            avatar,
            gamepass,
            gamepass_merchandised,
            subscriptions,
            entitlements_checked,
            tiers,
            section: Section::Library,
            query: String::new(),
            page: 0,
            show_unsupported: false,
            problem,
        }
    }

    /// Reload from disk, keeping what was fetched. Account-private state
    /// (account, avatar, ownership, subscriptions, refused launches) is kept
    /// only while the selected Store profile is unchanged.
    ///
    /// Re-reading a recipe file is not a reason to make someone sign in again,
    /// or to re-download a hundred icons.
    ///
    /// Scans already walking a game folder stay pending: starting another walk
    /// of the same hundred gigabytes on every Refresh would only race the first.
    /// The new generation tells their results to re-check the install first.
    pub fn reload(&mut self) {
        let mut fresh = Model::load();
        let same_store =
            self.roles.as_ref().map(|s| &s.store) == fresh.roles.as_ref().map(|s| &s.store);
        if same_store {
            fresh.ownership = std::mem::take(&mut self.ownership);
            fresh.account = self.account.take();
            fresh.avatar = self.avatar.take();
            fresh.subscriptions = std::mem::take(&mut self.subscriptions);
            fresh.entitlements_checked = self.entitlements_checked;
            fresh.refused = std::mem::take(&mut self.refused);
        }
        fresh.profiles = self.profiles.take();
        *self = Model {
            size_generation: self.size_generation.wrapping_add(1),
            sizes_pending: std::mem::take(&mut self.sizes_pending),
            catalog: std::mem::take(&mut self.catalog),
            icons: std::mem::take(&mut self.icons),
            gamepass: std::mem::take(&mut self.gamepass),
            gamepass_merchandised: std::mem::take(&mut self.gamepass_merchandised),
            tiers: std::mem::take(&mut self.tiers),
            section: self.section,
            query: std::mem::take(&mut self.query),
            page: self.page,
            show_unsupported: self.show_unsupported,
            ..fresh
        };
    }

    /// Whether the title is installed, not merely started: a folder appears
    /// the moment a download begins, and a stopped one is not a title to play.
    pub fn is_installed(&self, recipe: &Recipe) -> bool {
        self.game_dir(recipe)
            .is_some_and(|dir| install_complete(&dir))
    }

    /// Whether a download of this product stopped and left files where the
    /// title goes, so the row offers to resume it rather than to play it.
    pub fn incomplete(&self, product_id: &str) -> bool {
        let dir = match self.recipe_for(product_id) {
            Some(recipe) => self.game_dir(recipe),
            None => self
                .paths
                .as_ref()
                .map(|p| p.games_dir().join(product_id.to_ascii_lowercase())),
        };
        dir.is_some_and(|dir| !install_complete(&dir) && has_entries(&dir))
    }

    /// Where a title is, or would be, installed.
    ///
    /// Where it was installed wins over where its recipe would put it, unless
    /// the recipe's directory variable is set: a title installed somewhere
    /// else through "Install to" is there, and looking for it in the default
    /// place made it look uninstalled -- no Play, no Remove, and Install
    /// downloading it all again.
    pub fn game_dir(&self, recipe: &Recipe) -> Option<PathBuf> {
        let paths = self.paths.as_ref()?;
        let from_env = recipe
            .install
            .dir_env
            .as_deref()
            .is_some_and(|name| paths.var(name).is_some());
        if !from_env {
            if let Some(record) = self
                .records
                .get(&recipe.title.product_id.to_ascii_uppercase())
                .filter(|record| record.dir.is_dir())
            {
                return Some(record.dir.clone());
            }
        }
        paths.install_dir(recipe).ok()
    }

    /// The version in the package manifest on disk, if the title is there.
    ///
    /// A title installed outside this launcher has a version and no record;
    /// reading it costs one small file and is better than showing "Installed".
    pub fn installed_version(&self, recipe: &Recipe) -> Option<String> {
        install::package_version(&self.game_dir(recipe)?)
    }

    /// Wine's own way of shutting a title down: the server for its prefix, and
    /// the prefix. Signalling the process group this window created is not
    /// enough on its own -- wineserver detaches, so the game is reparented and
    /// survives, and the row would say stopped while it kept running.
    pub fn wineserver(&self, recipe: &Recipe) -> Option<(PathBuf, PathBuf)> {
        let paths = self.paths.as_ref()?;
        let runtime = self.launch_runtime(recipe).ok()?;
        let server = runtime.path.join("files/bin/wineserver");
        if !server.is_file() {
            return None;
        }
        let mut prefix = paths.prefix_dir(recipe).ok()?;
        if let Some(roles) = &self.roles {
            let (_, player) = roles.selected(self.profiles.as_ref()?).ok()?;
            prefix = accounts::player_prefix(&prefix, player).ok()?;
        }
        Some((server, prefix.join("pfx")))
    }

    /// The runtime a launch of this recipe would use, judged the way its row is.
    pub fn launch_runtime(&self, recipe: &Recipe) -> Result<&InstalledRuntime, String> {
        let paths = self.paths.as_ref().ok_or("nowhere to look for a runtime")?;
        let inferred = ferestre_core::autofix::infer(recipe, self.game_dir(recipe).as_deref());
        let selected = runtime::selected(paths.config_dir(), &recipe.title.product_id);
        crate::model::launch_runtime(
            &inferred.recipe,
            &self.runtimes,
            selected.as_deref(),
            self.registry.as_ref(),
        )
        .map(|(runtime, _)| runtime)
    }

    /// The runtime a title's launch would use, for its row's tooltip: whether
    /// it was chosen for the title, or picked as the newest that fits.
    pub fn runtime_note(&self, product_id: &str) -> Option<String> {
        let paths = self.paths.as_ref()?;
        let recipe = self.recipe_for(product_id)?;
        let chosen = runtime::selected(paths.config_dir(), product_id).is_some();
        let runtime = self.launch_runtime(recipe).ok()?;
        Some(if chosen {
            format!("Runtime: {} (chosen for this title)", runtime.label())
        } else {
            format!("Runtime: {}", runtime.label())
        })
    }

    /// The library folder "Install to" offers for this product, the folder the
    /// title gets inside it, and where that puts it by default.
    ///
    /// Below the games directory the title's folder is its path relative to
    /// it -- a recipe may put a package one level down, as Bedrock's does --
    /// and elsewhere it is the last component of where it was installed.
    pub fn install_target(&self, product_id: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
        let paths = self.paths.as_ref()?;
        let default = self.install_destination(product_id)?;
        let (library, folder) = match default.strip_prefix(paths.games_dir()) {
            Ok(relative) if !relative.as_os_str().is_empty() => {
                (paths.games_dir().to_path_buf(), relative.to_path_buf())
            }
            _ => (
                default.parent()?.to_path_buf(),
                PathBuf::from(default.file_name()?),
            ),
        };
        Some((library, folder, default))
    }

    /// What is in `dir`, as a destination for installing `product_id`.
    pub fn inspect_folder(&self, product_id: &str, dir: &Path) -> crate::model::Folder {
        use crate::model::Folder;
        let Ok(meta) = std::fs::metadata(dir) else {
            return Folder::Missing;
        };
        if !meta.is_dir() {
            return Folder::NotADirectory;
        }
        let key = product_id.to_ascii_uppercase();
        let recorded = self
            .records
            .get(&key)
            .is_some_and(|record| same_folder(&record.dir, dir));
        if recorded || store_id(dir).is_some_and(|id| id == key) {
            return Folder::This;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Folder::Other;
        };
        let names: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
        if names.is_empty() {
            return Folder::Empty;
        }
        if store_id(dir).is_none()
            && names
                .iter()
                .any(|n| n.to_string_lossy().starts_with(".xodus-streaming"))
        {
            return Folder::Download;
        }
        Folder::Other
    }

    /// The folder "Remove from disk" deletes, found the way `ferestre
    /// uninstall` finds it: the record, then the recipe's directory, then
    /// games_dir/<product id>. A stopped download has no record, and those are
    /// exactly the folders that most need removing.
    pub fn removal_dir(&self, product_id: &str) -> Option<PathBuf> {
        let paths = self.paths.as_ref()?;
        if let Some(record) = self.records.get(&product_id.to_ascii_uppercase()) {
            return Some(record.dir.clone());
        }
        // Records the window dropped as stale still count: uninstall reads them.
        if let Some(record) = install::load(paths.state_dir(), product_id) {
            return Some(record.dir);
        }
        self.recipe_for(product_id)
            .and_then(|recipe| paths.install_dir(recipe).ok())
            .or_else(|| {
                let guess = paths.games_dir().join(product_id.to_ascii_lowercase());
                guess.is_dir().then_some(guess)
            })
            .filter(|dir| dir.is_dir())
    }

    /// The title's Wine prefixes on disk: the one every launch used before
    /// accounts had roles, and one per Player that has launched it since.
    pub fn prefixes(&self, product_id: &str) -> Vec<PathBuf> {
        let Some(base) = self
            .paths
            .as_ref()
            .zip(self.recipe_for(product_id))
            .and_then(|(paths, recipe)| paths.prefix_dir(recipe).ok())
        else {
            return Vec::new();
        };
        player_prefixes(&base)
    }

    /// Where installing this product puts it by default: where it is already
    /// installed, so an update or a repair goes over it; otherwise the
    /// recipe's directory when there is one, and games_dir/<product id> when
    /// there is not -- the answers `ferestre install` gives without a folder.
    pub fn install_destination(&self, product_id: &str) -> Option<PathBuf> {
        let paths = self.paths.as_ref()?;
        match self.recipe_for(product_id) {
            Some(recipe) => self.game_dir(recipe),
            None => Some(
                self.records
                    .get(&product_id.to_ascii_uppercase())
                    .map(|record| record.dir.clone())
                    .unwrap_or_else(|| paths.games_dir().join(product_id.to_ascii_lowercase())),
            ),
        }
    }

    /// Whether a title with no recipe is on disk, where `ferestre install`
    /// would have put it: games_dir/<product id>.
    /// Whether a title with no recipe is nevertheless installed.
    ///
    /// A directory being there is not enough: one appears the moment a download
    /// starts, so this answered yes for a title that was 2% downloaded, and the
    /// row called itself installed and offered to set it up.
    pub fn product_is_installed(&self, product_id: &str) -> bool {
        self.installed_products
            .contains_key(&product_id.to_ascii_uppercase())
            || self
                .paths
                .as_ref()
                .map(|p| p.games_dir().join(product_id.to_ascii_lowercase()))
                .is_some_and(|dir| install_complete(&dir))
    }

    /// Where a title with no recipe is on disk, when anything says: its
    /// record, the folder whose package names it, or games_dir/<product id>.
    pub fn product_dir(&self, product_id: &str) -> Option<PathBuf> {
        let key = product_id.to_ascii_uppercase();
        self.records
            .get(&key)
            .map(|record| record.dir.clone())
            .or_else(|| self.installed_products.get(&key).cloned())
            .or_else(|| {
                let guess = self
                    .paths
                    .as_ref()?
                    .games_dir()
                    .join(product_id.to_ascii_lowercase());
                guess.is_dir().then_some(guess)
            })
    }

    /// The entry point the installed package declares, if it is on disk.
    pub fn detected_executable(&self, recipe: &Recipe) -> Option<String> {
        install::executable(&self.game_dir(recipe)?)
    }

    /// What the installed package says it launches, if anything did install it
    /// and the manifest names an entry point.
    ///
    /// By product id rather than by recipe, because the titles this matters for
    /// are exactly the ones with no recipe yet.
    pub fn product_executable(&self, product_id: &str) -> Option<String> {
        let record = self.records.get(&product_id.to_ascii_uppercase())?;
        install::executable(&record.dir)
    }

    pub fn catalog_cache(&self) -> Option<Cache> {
        self.paths.as_ref().map(Paths::catalog_cache)
    }

    pub fn cache_dir(&self) -> Option<&Path> {
        self.paths.as_ref().map(Paths::cache_dir)
    }

    pub fn private_state_dir(&self) -> Option<PathBuf> {
        Some(accounts::private_state(
            self.paths.as_ref()?.state_dir(),
            self.roles.as_ref(),
        ))
    }

    pub fn account_cache_dir(&self) -> Option<PathBuf> {
        Some(accounts::private_state(
            self.paths.as_ref()?.cache_dir(),
            self.roles.as_ref(),
        ))
    }

    pub fn xodus_cli(&self) -> Option<PathBuf> {
        self.paths.as_ref().and_then(Paths::xodus_cli)
    }

    /// Where an edited recipe goes. Never the packaged directory.
    pub fn user_titles_dir(&self) -> Option<PathBuf> {
        self.paths.as_ref().map(Paths::user_titles_dir)
    }

    pub fn recipe_for(&self, product_id: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|r| r.matches(product_id))
    }

    /// The market and language the catalog is asked in.
    pub fn market(&self) -> (String, String) {
        self.paths.as_ref().map(Paths::market).unwrap_or_else(|| {
            (
                catalog::DEFAULT_MARKET.to_string(),
                catalog::DEFAULT_LANGUAGE.to_string(),
            )
        })
    }
}

/// Remove the saved account and the library listed for it, so a window opened
/// later does not show either. Best effort: a file that is already gone is the
/// point.
pub fn forget_saved_account(state_dir: &Path) {
    let _ = std::fs::remove_file(library::cache_path(state_dir));
    let _ = std::fs::remove_file(ferestre_core::account::cache_path(state_dir));
}

/// Remove the saved picture of the account that signed out, so the next one
/// to sign in -- or nobody -- is not shown with it.
pub fn forget_saved_avatar(account_cache_dir: &Path) {
    let _ = std::fs::remove_file(ferestre_core::account::avatar_path(
        account_cache_dir,
        AVATAR_PX,
    ));
}

/// The saved picture, for a saved account only: a picture with nobody signed
/// in is the last person's.
fn saved_avatar(account: Option<&Account>, account_cache_dir: Option<&Path>) -> Option<PathBuf> {
    account?;
    Some(ferestre_core::account::avatar_path(
        account_cache_dir?,
        AVATAR_PX,
    ))
    .filter(|p| p.is_file())
}

/// Move an unreadable `account-roles.json` aside, so the window can start
/// without roles and the Accounts dialog can write a new one. Kept rather than
/// deleted: it may be a newer build's file, wanted back after an upgrade.
pub fn set_aside_roles(config_dir: &Path) -> std::io::Result<PathBuf> {
    let file = config_dir.join("account-roles.json");
    let aside = (0..)
        .map(|n| match n {
            0 => config_dir.join("account-roles.json.unreadable"),
            n => config_dir.join(format!("account-roles.json.unreadable.{n}")),
        })
        .find(|path| !path.exists())
        .expect("an unused name");
    std::fs::rename(file, &aside)?;
    Ok(aside)
}

/// A file for a launch's diagnostic output, created afresh for each launch and
/// readable only by its owner: it can name accounts and paths.
pub fn launch_output(state_dir: &Path, key: &str) -> std::io::Result<(PathBuf, std::fs::File)> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = state_dir.join("launches");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    let name: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let path = dir.join(format!("{name}.stderr"));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    Ok((path, file))
}

/// The last `count` lines of a file, each cut to a sensible length, without
/// reading more than its last 64 KiB.
pub fn tail_lines(path: &Path, count: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    const WINDOW: u64 = 64 * 1024;
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let length = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = length.saturating_sub(WINDOW);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    // A window that starts mid-file starts mid-line.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let lines: Vec<String> = lines
        .into_iter()
        .map(|line| {
            String::from_utf8_lossy(&line[..line.len().min(512)])
                .trim_end()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].to_vec()
}

/// The temporary container a download writes into, renamed when it ends.
const DOWNLOADING: &str = ".xodus-streaming-tmp.msixvc";

/// Whether `dir` holds a finished install. The package header and manifest
/// land early in a download, so their being there is not enough while the
/// temporary container is still beside them.
pub fn install_complete(dir: &Path) -> bool {
    install::looks_installed(dir) && !dir.join(DOWNLOADING).exists()
}

fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

fn same_folder(a: &Path, b: &Path) -> bool {
    let normal = |p: &Path| p.components().collect::<PathBuf>();
    normal(a) == normal(b)
        || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// The `<StoreId>` in a package's MicrosoftGame.config, uppercase.
fn store_id(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(install::game_config(dir)?).ok()?;
    let start = text.find("<StoreId>")? + "<StoreId>".len();
    let value = text[start..].split_once("</StoreId>")?.0.trim();
    (!value.is_empty()).then(|| value.to_ascii_uppercase())
}

/// Finished installs directly below `games_dir`, by the product id their
/// package names, with the folder each is in.
fn installed_products(games_dir: &Path) -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(games_dir) else {
        return found;
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        if dir.is_dir() && install_complete(&dir) {
            if let Some(id) = store_id(&dir) {
                found.entry(id).or_insert(dir);
            }
        }
    }
    found
}

/// A title's base Wine prefix and every Player's copy of it that exists:
/// `<parent>/players/<namespace>/<name>`, as `accounts::player_prefix` makes.
fn player_prefixes(base: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    if base.is_dir() {
        found.push(base.to_path_buf());
    }
    let (Some(parent), Some(name)) = (base.parent(), base.file_name()) else {
        return found;
    };
    let mut players: Vec<PathBuf> = std::fs::read_dir(parent.join("players"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join(name))
        .filter(|path| path.is_dir() && !path.is_symlink())
        .collect();
    players.sort();
    found.extend(players);
    found
}

/// Recipes from each directory, later ones winning, as
/// `Recipe::load_layered` reads them -- except that a file that cannot be
/// read is skipped and named, rather than costing every other recipe. One
/// typo used to empty the whole library.
fn load_recipes(dirs: &[PathBuf]) -> (Vec<Recipe>, Vec<String>) {
    let mut by_product: BTreeMap<String, Recipe> = BTreeMap::new();
    let mut skipped = Vec::new();
    for dir in dirs.iter().filter(|dir| dir.is_dir()) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                skipped.push(format!("{}: {error}", dir.display()));
                continue;
            }
        };
        let mut found: Vec<Recipe> = Vec::new();
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            if path.extension().and_then(|e| e.to_str()) != Some("toml")
                || path.file_stem().and_then(|s| s.to_str()) == Some("capabilities")
            {
                continue;
            }
            match Recipe::load(&path) {
                Ok(recipe) => found.push(recipe),
                Err(error) => skipped.push(format!("{error:#}")),
            }
        }
        found.sort_by(|a, b| a.title.product_id.cmp(&b.title.product_id));
        for recipe in found {
            by_product.insert(recipe.title.product_id.to_ascii_uppercase(), recipe);
        }
    }
    (by_product.into_values().collect(), skipped)
}

/// Icons already on disk from a previous run. Nothing is fetched here: a window
/// that waits on a hundred HTTP requests before it appears is not a window.
fn existing_icons(cache: &Cache, catalog: &BTreeMap<String, Product>) -> BTreeMap<String, PathBuf> {
    catalog
        .keys()
        .filter_map(|key| {
            cache
                .image_path(key, ICON_PX)
                .filter(|path| path.is_file())
                .map(|path| (key.clone(), path))
        })
        .collect()
}

/// Record titles that are installed but were never recorded.
///
/// Silent and automatic, because it is exact rather than assumed: the content
/// id comes out of the package header the client left on disk, so there is
/// nothing to ask and nothing to get wrong. A title whose header cannot be read
/// is left alone -- no record is better than one that answers "up to date" to a
/// question it cannot answer.
fn adopt_existing(
    paths: Option<&Paths>,
    recipes: &[Recipe],
    mut records: BTreeMap<String, Record>,
) -> BTreeMap<String, Record> {
    let Some(paths) = paths else { return records };
    for recipe in recipes {
        let key = recipe.title.product_id.to_ascii_uppercase();
        if records.contains_key(&key) {
            continue;
        }
        let Ok(dir) = paths.install_dir(recipe) else {
            continue;
        };
        if !dir.is_dir() {
            continue;
        }
        if let Some(record) = install::adopt(&recipe.title.product_id, &dir) {
            // Best effort: an unwritable state directory costs the adoption
            // next time, not this listing.
            let _ = install::save(paths.state_dir(), &record);
            records.insert(key, record);
        }
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Folder;
    use ferestre_core::paths::Env;

    /// A scratch directory, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ferestre-gui-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn config(store_id: &str) -> String {
        format!("<Game><StoreId>{store_id}</StoreId></Game>")
    }

    /// A finished install of `store_id` in `dir`.
    fn installed(dir: &Path, store_id: &str) {
        write(&dir.join("MicrosoftGame.config"), &config(store_id));
        write(&dir.join("appxmanifest.xml"), "<Package/>");
        write(&dir.join(".xodus-streaming.msixvc"), "");
    }

    fn recipe(product_id: &str, dir: Option<&str>) -> Recipe {
        let install = dir.map_or(String::new(), |d| format!("[install]\ndir = \"{d}\"\n"));
        Recipe::parse(&format!(
            "schema = 1\n[title]\nproduct-id = \"{product_id}\"\nname = \"Test\"\n\
             slug = \"test\"\n[launch]\nexecutable = \"Game.exe\"\n{install}\
             [status]\nstate = \"untested\"\nsummary = \"s\"\n"
        ))
        .unwrap()
    }

    fn model_at(root: &Path) -> Model {
        let root = root.display().to_string();
        let paths = Paths::resolve(&Env::from_map([
            ("HOME", format!("{root}/home")),
            ("XODUS_GAMES_DIR", format!("{root}/games")),
            ("XDG_STATE_HOME", format!("{root}/state")),
            ("XDG_CONFIG_HOME", format!("{root}/config")),
            ("XDG_CACHE_HOME", format!("{root}/cache")),
        ]))
        .unwrap();
        Model {
            roles: None,
            roles_error: None,
            profiles: None,
            refused: Default::default(),
            paths: Some(paths),
            recipes: Vec::new(),
            runtime: None,
            runtimes: Vec::new(),
            registry: None,
            records: BTreeMap::new(),
            disk_usage: BTreeMap::new(),
            sizes_pending: Default::default(),
            size_generation: 0,
            installed_products: Default::default(),
            ownership: Default::default(),
            catalog: BTreeMap::new(),
            icons: BTreeMap::new(),
            available: BTreeMap::new(),
            account: None,
            avatar: None,
            gamepass: Vec::new(),
            gamepass_merchandised: Default::default(),
            subscriptions: Vec::new(),
            entitlements_checked: false,
            tiers: Default::default(),
            section: Section::Library,
            query: String::new(),
            page: 0,
            show_unsupported: false,
            problem: None,
        }
    }

    fn record(product_id: &str, dir: &Path) -> Record {
        Record {
            product_id: product_id.into(),
            dir: dir.to_path_buf(),
            content_ids: vec![],
            installed_at: None,
            package_version: None,
        }
    }

    /// The package header and manifest arrive early in a download. With the
    /// temporary container still beside them, the title is not installed.
    #[test]
    fn a_download_still_in_progress_is_not_an_install() {
        let scratch = Scratch::new("partial");
        let dir = scratch.0.join("games/test");
        installed(&dir, "9ZZTESTGAME1");
        assert!(install_complete(&dir));
        write(&dir.join(DOWNLOADING), "");
        assert!(!install_complete(&dir));
        assert!(
            installed_products(&scratch.0.join("games")).is_empty(),
            "a partial download is not an installed product"
        );

        let mut model = model_at(&scratch.0);
        model.recipes = vec![recipe("9ZZTESTGAME1", None)];
        assert!(!model.is_installed(&model.recipes[0]));
        assert!(model.incomplete("9ZZTESTGAME1"));
        std::fs::remove_file(dir.join(DOWNLOADING)).unwrap();
        assert!(model.is_installed(&model.recipes[0]));
        assert!(!model.incomplete("9ZZTESTGAME1"));
        // A folder nothing arrived in is neither.
        std::fs::create_dir_all(scratch.0.join("games/9zzemptyone1")).unwrap();
        assert!(!model.incomplete("9ZZEMPTYONE1"));
    }

    /// Installed with "Install to" somewhere else, a title whose recipe names
    /// a folder is found where it was installed, not looked for where the
    /// recipe would have put it.
    #[test]
    fn a_title_is_found_where_it_was_installed() {
        let scratch = Scratch::new("record");
        let elsewhere = scratch.0.join("mnt/forza-horizon-6");
        installed(&elsewhere, "9NR1R1XWLCNB");
        let mut model = model_at(&scratch.0);
        model.recipes = vec![recipe("9NR1R1XWLCNB", Some("forza-horizon-6"))];
        assert!(!model.is_installed(&model.recipes[0]));
        model
            .records
            .insert("9NR1R1XWLCNB".into(), record("9NR1R1XWLCNB", &elsewhere));
        assert_eq!(model.game_dir(&model.recipes[0]), Some(elsewhere.clone()));
        assert!(model.is_installed(&model.recipes[0]));
    }

    /// Remove finds the folder the way `ferestre uninstall` does, so a
    /// download that stopped before anything was recorded can be removed.
    #[test]
    fn remove_finds_what_uninstall_would_remove() {
        let scratch = Scratch::new("removal");
        let mut model = model_at(&scratch.0);
        assert_eq!(model.removal_dir("9ZZNOTHING01"), None);

        let guess = scratch.0.join("games/9zzpartial01");
        write(&guess.join(DOWNLOADING), "x");
        assert_eq!(model.removal_dir("9ZZPARTIAL01"), Some(guess));

        model.recipes = vec![recipe("9ZZRECIPE001", Some("described"))];
        let described = scratch.0.join("games/described");
        std::fs::create_dir_all(&described).unwrap();
        assert_eq!(model.removal_dir("9ZZRECIPE001"), Some(described));

        let recorded = scratch.0.join("mnt/somewhere");
        installed(&recorded, "9ZZRECIPE001");
        model
            .records
            .insert("9ZZRECIPE001".into(), record("9ZZRECIPE001", &recorded));
        assert_eq!(model.removal_dir("9ZZRECIPE001"), Some(recorded));
    }

    /// With account roles, each Player plays in a prefix of its own. All of
    /// them are saved games, and all of them are listed.
    #[test]
    fn every_players_prefix_is_listed() {
        let scratch = Scratch::new("prefixes");
        let mut model = model_at(&scratch.0);
        model.recipes = vec![recipe("9ZZRECIPE001", None)];
        assert!(model.prefixes("9ZZRECIPE001").is_empty());
        let games = scratch.0.join("games");
        for dir in [
            "test-proton/pfx",
            "players/bbbb/test-proton/pfx",
            "players/aaaa/test-proton/pfx",
            "players/cccc/other-proton/pfx",
        ] {
            std::fs::create_dir_all(games.join(dir)).unwrap();
        }
        assert_eq!(
            model.prefixes("9ZZRECIPE001"),
            vec![
                games.join("test-proton"),
                games.join("players/aaaa/test-proton"),
                games.join("players/bbbb/test-proton"),
            ]
        );
    }

    #[test]
    fn install_to_knows_a_titles_folder_from_someone_elses() {
        let scratch = Scratch::new("inspect");
        let mut model = model_at(&scratch.0);
        let root = &scratch.0;
        assert_eq!(
            model.inspect_folder("9ZZA", &root.join("none")),
            Folder::Missing
        );
        write(&root.join("file"), "x");
        assert_eq!(
            model.inspect_folder("9ZZA", &root.join("file")),
            Folder::NotADirectory
        );
        std::fs::create_dir_all(root.join("empty")).unwrap();
        assert_eq!(
            model.inspect_folder("9ZZA", &root.join("empty")),
            Folder::Empty
        );
        installed(&root.join("mine"), "9ZZA");
        assert_eq!(
            model.inspect_folder("9zza", &root.join("mine")),
            Folder::This
        );
        installed(&root.join("theirs"), "9ZZB");
        assert_eq!(
            model.inspect_folder("9ZZA", &root.join("theirs")),
            Folder::Other
        );
        write(&root.join("partial").join(DOWNLOADING), "x");
        assert_eq!(
            model.inspect_folder("9ZZA", &root.join("partial")),
            Folder::Download
        );
        write(&root.join("library/Some Game/game.exe"), "x");
        assert_eq!(
            model.inspect_folder("9ZZA", &root.join("library")),
            Folder::Other
        );
        model
            .records
            .insert("9ZZA".into(), record("9ZZA", &root.join("library")));
        assert_eq!(
            model.inspect_folder("9ZZA", &root.join("library/")),
            Folder::This,
            "the folder its record names is its own, whatever is in it"
        );
    }

    #[test]
    fn install_to_offers_the_library_and_keeps_the_titles_folder() {
        let scratch = Scratch::new("target");
        let mut model = model_at(&scratch.0);
        let games = scratch.0.join("games");
        model.recipes = vec![recipe("9NBLGGH2JHXJ", Some("bedrock/game"))];
        assert_eq!(
            model.install_target("9NBLGGH2JHXJ"),
            Some((
                games.clone(),
                PathBuf::from("bedrock/game"),
                games.join("bedrock/game")
            ))
        );
        assert_eq!(
            model.install_target("9ZZNORECIPE1"),
            Some((
                games.clone(),
                PathBuf::from("9zznorecipe1"),
                games.join("9zznorecipe1")
            ))
        );

        // Installed elsewhere: an update goes over it, wherever it is.
        let elsewhere = scratch.0.join("mnt/big/game");
        installed(&elsewhere, "9NBLGGH2JHXJ");
        model
            .records
            .insert("9NBLGGH2JHXJ".into(), record("9NBLGGH2JHXJ", &elsewhere));
        assert_eq!(
            model.install_target("9NBLGGH2JHXJ"),
            Some((scratch.0.join("mnt/big"), PathBuf::from("game"), elsewhere))
        );
    }

    /// One unreadable recipe used to empty the whole library.
    #[test]
    fn a_bad_recipe_file_is_skipped_and_named() {
        let scratch = Scratch::new("recipes");
        let packaged = scratch.0.join("titles");
        let user = scratch.0.join("user");
        let good = |id: &str, name: &str| {
            format!(
                "schema = 1\n[title]\nproduct-id = \"{id}\"\nname = \"{name}\"\n\
                 slug = \"s{id}\"\n[launch]\nexecutable = \"Game.exe\"\n\
                 [status]\nstate = \"untested\"\nsummary = \"s\"\n"
            )
        };
        write(&packaged.join("a.toml"), &good("9ZZA", "Packaged A"));
        write(&packaged.join("b.toml"), &good("9ZZB", "Packaged B"));
        write(&packaged.join("capabilities.toml"), "not = a recipe");
        write(&user.join("a.toml"), &good("9ZZA", "Edited A"));
        write(&user.join("broken.toml"), "schema = 1\n[title\n");
        let (recipes, skipped) = load_recipes(&[packaged, user, scratch.0.join("missing")]);
        let names: Vec<_> = recipes.iter().map(|r| r.title.name.as_str()).collect();
        assert_eq!(names, ["Edited A", "Packaged B"]);
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("broken.toml"), "{skipped:?}");
    }

    #[test]
    fn an_unreadable_roles_file_is_set_aside_not_deleted() {
        let scratch = Scratch::new("roles");
        let file = scratch.0.join("account-roles.json");
        write(&file, "{\"store\":1}");
        let aside = set_aside_roles(&scratch.0).unwrap();
        assert!(!file.exists());
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{\"store\":1}");
        write(&file, "again");
        let second = set_aside_roles(&scratch.0).unwrap();
        assert_ne!(aside, second, "an earlier copy is kept");
        assert!(set_aside_roles(&scratch.0).is_err(), "nothing left to move");
    }

    /// The picture is cached by size alone, so with nobody signed in a saved
    /// one is the previous account's.
    #[test]
    fn the_saved_picture_goes_with_the_account() {
        let scratch = Scratch::new("avatar");
        let picture = ferestre_core::account::avatar_path(&scratch.0, AVATAR_PX);
        write(&picture, "png");
        let someone =
            ferestre_core::account::parse(r#"{"gamertag":"Someone","xuid":"25"}"#).unwrap();
        assert_eq!(saved_avatar(None, Some(&scratch.0)), None);
        assert_eq!(
            saved_avatar(Some(&someone), Some(&scratch.0)),
            Some(picture.clone())
        );
        forget_saved_avatar(&scratch.0);
        assert!(!picture.exists());
        assert_eq!(saved_avatar(Some(&someone), Some(&scratch.0)), None);
    }

    /// The launcher's diagnostics go to a file of the window's, not a pipe:
    /// the title outlives the window.
    #[test]
    fn a_launchs_output_is_kept_in_a_private_file_and_its_tail_read_back() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("launch");
        let (path, mut file) = launch_output(&scratch.0, "9ZZA/../x").unwrap();
        assert_eq!(path.parent().unwrap(), scratch.0.join("launches"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for n in 0..100 {
            writeln!(file, "line {n}").unwrap();
        }
        file.write_all(b"\xff not utf-8\n!! the reason\n").unwrap();
        let tail = tail_lines(&path, 3);
        assert_eq!(tail, ["line 99", "\u{fffd} not utf-8", "!! the reason"]);
        // A new launch starts the file afresh.
        let (_, _) = launch_output(&scratch.0, "9ZZA/../x").unwrap();
        assert!(tail_lines(&path, 3).is_empty());
    }

    #[test]
    fn forgetting_the_saved_account_removes_its_library_too() {
        let dir = std::env::temp_dir().join(format!("ferestre-gui-forget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let account = ferestre_core::account::cache_path(&dir);
        let listing = library::cache_path(&dir);
        std::fs::write(&account, "{}").unwrap();
        std::fs::write(&listing, "[]").unwrap();

        forget_saved_account(&dir);
        assert!(!account.exists());
        assert!(!listing.exists());
        // Already gone is fine: that is what it is for.
        forget_saved_account(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

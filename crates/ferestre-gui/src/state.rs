//! What the window has loaded, and how it loads more.
//!
//! Split from the widgets because the two have different lifetimes: a rebuild
//! throws away every row and builds new ones, and none of what was fetched
//! should go with them. Anything that cost a network round trip -- the account,
//! the library, the catalog, the icons -- lives here and survives a redraw.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ferestre_core::account::Account;
use ferestre_core::catalog::{self, Cache, Product};
use ferestre_core::gamepass;
use ferestre_core::install::{self, Record};
use ferestre_core::library;
use ferestre_core::paths::Paths;
use ferestre_core::recipe::Recipe;
use ferestre_core::runtime::{InstalledRuntime, Registry};

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
    pub paths: Option<Paths>,
    pub recipes: Vec<Recipe>,
    pub runtime: Option<InstalledRuntime>,
    pub registry: Option<Registry>,
    pub records: BTreeMap<String, Record>,

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
    pub subscriptions: Vec<&'static str>,

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
    pub fn load() -> Self {
        let mut problem = None;
        let paths = match Paths::from_env() {
            Ok(p) => Some(p),
            Err(e) => {
                problem = Some(format!("Cannot work out where things live: {e}"));
                None
            }
        };

        let recipes = paths
            .as_ref()
            .map(|p| Recipe::load_layered(&p.title_dirs()).unwrap_or_default())
            .unwrap_or_default();
        let registry = paths
            .as_ref()
            .and_then(Paths::titles_dir)
            .and_then(|dir| Registry::load(&dir.join("capabilities.toml")).ok());
        let runtime = paths
            .as_ref()
            .and_then(|p| InstalledRuntime::discover(p).ok());
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

        // The last listing, and whatever the catalog already knows about it.
        // Both come off disk, so the window opens showing a library instead of
        // an empty page with a button on it; the button then means "check
        // again" rather than "start".
        let ownership = paths
            .as_ref()
            .and_then(|p| library::load_cache(p.state_dir()))
            .map(Ownership::Known)
            .unwrap_or_default();
        let catalog = match (&paths, &ownership) {
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
        let gamepass = paths
            .as_ref()
            .and_then(|p| gamepass::load_cache(p.state_dir(), &market))
            .map(|listing| listing.product_ids)
            .unwrap_or_default();
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
        let avatar = paths
            .as_ref()
            .map(|p| ferestre_core::account::avatar_path(p.cache_dir(), AVATAR_PX))
            .filter(|p| p.is_file());

        if problem.is_none() && recipes.is_empty() {
            problem = Some(
                "No title recipes found. Point FERESTRE_TITLES at the titles/ directory.".into(),
            );
        }

        Model {
            paths,
            recipes,
            runtime,
            registry,
            records,
            ownership,
            catalog,
            icons,
            available: BTreeMap::new(),
            account: None,
            avatar,
            gamepass,
            subscriptions,
            section: Section::Library,
            query: String::new(),
            page: 0,
            show_unsupported: false,
            problem,
        }
    }

    /// Reload from disk, keeping everything that was fetched.
    ///
    /// Re-reading a recipe file is not a reason to make someone sign in again,
    /// or to re-download a hundred icons.
    pub fn reload(&mut self) {
        let fresh = Model::load();
        *self = Model {
            ownership: std::mem::take(&mut self.ownership),
            catalog: std::mem::take(&mut self.catalog),
            icons: std::mem::take(&mut self.icons),
            available: std::mem::take(&mut self.available),
            account: self.account.take(),
            avatar: self.avatar.take(),
            gamepass: std::mem::take(&mut self.gamepass),
            subscriptions: std::mem::take(&mut self.subscriptions),
            section: self.section,
            query: std::mem::take(&mut self.query),
            page: self.page,
            show_unsupported: self.show_unsupported,
            ..fresh
        };
    }

    pub fn is_installed(&self, recipe: &Recipe) -> bool {
        self.paths
            .as_ref()
            .and_then(|p| p.install_dir(recipe).ok())
            .is_some_and(|dir| dir.is_dir())
    }

    /// The version in the package manifest on disk, if the title is there.
    ///
    /// A title installed outside this launcher has a version and no record;
    /// reading it costs one small file and is better than showing "Installed".
    pub fn installed_version(&self, recipe: &Recipe) -> Option<String> {
        let dir = self.paths.as_ref()?.install_dir(recipe).ok()?;
        install::package_version(&dir)
    }

    /// Wine's own way of shutting a title down: the server for its prefix, and
    /// the prefix. Signalling the process group this window created is not
    /// enough on its own -- wineserver detaches, so the game is reparented and
    /// survives, and the row would say stopped while it kept running.
    pub fn wineserver(&self, recipe: &Recipe) -> Option<(PathBuf, PathBuf)> {
        let paths = self.paths.as_ref()?;
        let server = paths.runtime_dir()?.join("files/bin/wineserver");
        if !server.is_file() {
            return None;
        }
        // Proton puts the prefix one level inside STEAM_COMPAT_DATA_PATH.
        Some((server, paths.prefix_dir(recipe).ok()?.join("pfx")))
    }

    /// Where installing this product would put it: the recipe's directory when
    /// there is one, and games_dir/<product id> when there is not -- the same
    /// two answers `ferestre install` gives, so the dialog cannot promise a
    /// path the command would not use.
    pub fn install_destination(&self, product_id: &str) -> Option<PathBuf> {
        let paths = self.paths.as_ref()?;
        match self.recipe_for(product_id) {
            Some(recipe) => paths.install_dir(recipe).ok(),
            None => Some(paths.games_dir().join(product_id.to_ascii_lowercase())),
        }
    }

    /// Whether a title with no recipe is on disk, where `ferestre install`
    /// would have put it: games_dir/<product id>.
    pub fn product_is_installed(&self, product_id: &str) -> bool {
        self.paths
            .as_ref()
            .map(|p| p.games_dir().join(product_id.to_ascii_lowercase()))
            .is_some_and(|dir| dir.is_dir())
    }

    /// The entry point the installed package declares, if it is on disk.
    pub fn detected_executable(&self, recipe: &Recipe) -> Option<String> {
        let dir = self.paths.as_ref()?.install_dir(recipe).ok()?;
        install::executable(&dir)
    }

    pub fn catalog_cache(&self) -> Option<Cache> {
        self.paths.as_ref().map(Paths::catalog_cache)
    }

    pub fn cache_dir(&self) -> Option<&Path> {
        self.paths.as_ref().map(Paths::cache_dir)
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

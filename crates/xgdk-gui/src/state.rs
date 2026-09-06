//! What the window has loaded, and how it loads more.
//!
//! Split from the widgets because the two have different lifetimes: a rebuild
//! throws away every row and builds new ones, and none of what was fetched
//! should go with them. Anything that cost a network round trip -- the account,
//! the library, the catalog, the icons -- lives here and survives a redraw.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use xgdk_core::account::Account;
use xgdk_core::catalog::{self, Cache, Product};
use xgdk_core::install::{self, Record};
use xgdk_core::library;
use xgdk_core::paths::Paths;
use xgdk_core::recipe::Recipe;
use xgdk_core::runtime::{InstalledRuntime, Registry};

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
    Installed,
    Updates,
    Runtime,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Library,
        Section::Installed,
        Section::Updates,
        Section::Runtime,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Library => "Library",
            Section::Installed => "Installed",
            Section::Updates => "Updates",
            Section::Runtime => "Runtime",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Section::Library => "view-grid-symbolic",
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
    pub account: Option<Account>,
    pub avatar: Option<PathBuf>,

    /// Where the view is, which is state the widgets must not own -- a rebuild
    /// destroys them and the person's place in the list should survive it.
    pub section: Section,
    pub query: String,
    pub page: usize,

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
        let records = paths
            .as_ref()
            .map(|p| {
                install::all(p.state_dir())
                    .into_iter()
                    .map(|r| (r.product_id.to_ascii_uppercase(), r))
                    .collect()
            })
            .unwrap_or_default();

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
                let cache = catalog::Cache::new(paths.cache_dir().join("catalog"));
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
            .map(|p| xgdk_core::account::avatar_path(p.cache_dir(), AVATAR_PX))
            .filter(|p| p.is_file());

        if problem.is_none() && recipes.is_empty() {
            problem =
                Some("No title recipes found. Point XGDK_TITLES at the titles/ directory.".into());
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
            account: None,
            avatar,
            section: Section::Library,
            query: String::new(),
            page: 0,
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
            account: self.account.take(),
            avatar: self.avatar.take(),
            section: self.section,
            query: std::mem::take(&mut self.query),
            page: self.page,
            ..fresh
        };
    }

    pub fn is_installed(&self, recipe: &Recipe) -> bool {
        self.paths
            .as_ref()
            .and_then(|p| p.install_dir(recipe).ok())
            .is_some_and(|dir| dir.is_dir())
    }

    pub fn catalog_cache(&self) -> Option<Cache> {
        self.paths
            .as_ref()
            .map(|p| Cache::new(p.cache_dir().join("catalog")))
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

    /// The market and language the catalog is asked in. Overridable, because a
    /// title reads better in its own and the ids are the same either way.
    pub fn market(&self) -> (String, String) {
        let var = |name: &str, fallback: &str| {
            self.paths
                .as_ref()
                .and_then(|p| p.var(name))
                .unwrap_or(fallback)
                .to_string()
        };
        (
            var("XGDK_MARKET", catalog::DEFAULT_MARKET),
            var("XGDK_LANGUAGE", catalog::DEFAULT_LANGUAGE),
        )
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

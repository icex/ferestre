//! What the account owns, joined to the recipes.
//!
//! The listing comes from `xodus-cli library --json`, which queries the Store's
//! collections service and prints `{"count":N,"items":[…]}` on stdout. That
//! command runs as a child process and nothing else: this crate must not link
//! the Xodus crates (GPL-3.0-only, against this crate's MIT), and shelling out
//! is what keeps the boundary honest.
//!
//! So no function here runs anything. `library_command` builds the command and
//! hands it back for the caller to spawn; `parse` takes the JSON as a string.
//! Every decision this module makes -- what counts as a title, which owned
//! product matches which recipe -- is therefore testable with no account, no
//! network and no sign-in.
//!
//! An entitlement list is not a game list. An account accumulates add-ons,
//! subscriptions, expired trials and console-only titles, so the filtering is
//! the substance of this module, and it keeps the reason a row was dropped
//! rather than discarding it: "why is my game missing" is only answerable if
//! something can name the filter that removed it.

use crate::recipe::Recipe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Whether an entitlement is still good.
///
/// Kept as an enum with a catch-all rather than a bool so an unfamiliar value
/// survives into the output instead of being silently read as "not active".
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
// Round-trips as the service's own string. Without `into`, the derive writes a
// catch-all as `{"Other":"Foo"}`, which is not what `from = "String"` reads back
// -- so a cached listing containing one unfamiliar value would fail to load.
#[serde(from = "String", into = "String")]
pub enum EntitlementStatus {
    Active,
    Expired,
    Revoked,
    /// Anything else the service returns, verbatim.
    Other(String),
}

impl From<String> for EntitlementStatus {
    fn from(value: String) -> Self {
        match value.as_str() {
            "Active" => Self::Active,
            "Expired" => Self::Expired,
            "Revoked" => Self::Revoked,
            _ => Self::Other(value),
        }
    }
}

impl From<EntitlementStatus> for String {
    fn from(value: EntitlementStatus) -> Self {
        value.as_str().to_string()
    }
}

impl Default for EntitlementStatus {
    fn default() -> Self {
        Self::Other(String::new())
    }
}

impl EntitlementStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Active => "Active",
            Self::Expired => "Expired",
            Self::Revoked => "Revoked",
            Self::Other(s) => s,
        }
    }
}

/// What kind of thing the entitlement is for. `Game` and `Application` are the
/// two a launcher can install; `Durable` is an add-on and `Pass` a subscription,
/// and an entitlement to a subscription is not an entitlement to the titles in
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(from = "String", into = "String")]
pub enum ProductType {
    Game,
    Application,
    Durable,
    Pass,
    Other(String),
}

impl From<String> for ProductType {
    fn from(value: String) -> Self {
        match value.as_str() {
            "Game" => Self::Game,
            "Application" => Self::Application,
            "Durable" => Self::Durable,
            "Pass" => Self::Pass,
            _ => Self::Other(value),
        }
    }
}

impl From<ProductType> for String {
    fn from(value: ProductType) -> Self {
        value.as_str().to_string()
    }
}

impl Default for ProductType {
    fn default() -> Self {
        Self::Other(String::new())
    }
}

impl ProductType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Game => "Game",
            Self::Application => "Application",
            Self::Durable => "Durable",
            Self::Pass => "Pass",
            Self::Other(s) => s,
        }
    }
}

/// One entitlement out of the collections listing.
///
/// Unknown fields are ignored, unlike a recipe, where they are an error. A
/// recipe is our own file and a misspelled key there is a bug; this payload is
/// Microsoft's and gains fields without warning, so refusing it on sight would
/// break a library listing for no gain. Only what a launcher acts on is modelled.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// The 12-character Store id, and the key everything joins on: it is what a
    /// Store URL carries and what `titles/*.toml` is named after.
    pub product_id: String,
    /// Missing rather than unrecognised values default to the catch-all, so a
    /// row the service describes oddly is skipped with a reason instead of
    /// aborting the whole listing.
    #[serde(default)]
    pub product_type: ProductType,
    #[serde(default)]
    pub status: EntitlementStatus,
    /// `Full`, `Trial`, and so on. Carried through for display; not filtered on,
    /// because a trial of a title is still a title you can install.
    #[serde(default)]
    pub sku_type: Option<String>,
    /// Left as the service's own string. No date type here would be worth a
    /// dependency, and nothing in the launcher does arithmetic on it.
    #[serde(default)]
    pub acquired_date: Option<String>,
    #[serde(default)]
    pub ownership_type: Option<String>,
}

impl Entry {
    /// Why this row is not something a launcher would offer, or `None` if it is.
    ///
    /// The reason is returned rather than folded into a bool so a caller can
    /// show its filtering instead of asking to be trusted.
    pub fn skip_reason(&self) -> Option<&'static str> {
        if self.status != EntitlementStatus::Active {
            return Some("entitlement is not active");
        }
        if !matches!(self.product_type, ProductType::Game | ProductType::Application) {
            return Some("not a game or application");
        }
        None
    }

    pub fn is_title(&self) -> bool {
        self.skip_reason().is_none()
    }
}

/// Parse the output of `xodus-cli library --json`.
///
/// Accepts either the whole document or a bare array of items, because the
/// obvious thing to do with that output is pipe it through `jq .items` and the
/// difference is not worth an error.
pub fn parse(json: &str) -> anyhow::Result<Vec<Entry>> {
    let document: serde_json::Value =
        serde_json::from_str(json).map_err(|e| anyhow::anyhow!("not JSON: {e}"))?;

    let items = match document {
        serde_json::Value::Array(items) => serde_json::Value::Array(items),
        serde_json::Value::Object(mut map) => map.remove("items").ok_or_else(|| {
            // A failed call prints the service's error body on stdout, so this
            // is the likely case and the message should not claim a parse bug.
            anyhow::anyhow!("no \"items\" in the collections output (a failed query prints the service's error here)")
        })?,
        _ => anyhow::bail!("expected a collections document or an array of items"),
    };

    serde_json::from_value(items).map_err(|e| anyhow::anyhow!("unexpected item shape: {e}"))
}

/// The rows a launcher could plausibly install: active entitlements for a game
/// or an application.
pub fn titles(entries: Vec<Entry>) -> Vec<Entry> {
    entries.into_iter().filter(Entry::is_title).collect()
}

/// The rows that were filtered out, each with the reason, so the filtering is
/// auditable rather than a black box.
pub fn skipped(entries: &[Entry]) -> Vec<(&Entry, &'static str)> {
    entries
        .iter()
        .filter_map(|e| e.skip_reason().map(|why| (e, why)))
        .collect()
}

/// How an owned title and a recipe line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Owned, and there is a recipe: the only combination that can be launched.
    OwnedWithRecipe,
    /// Owned, but nobody has written a recipe for it yet. This is the list that
    /// says what to work on next.
    OwnedNoRecipe,
    /// A recipe exists for something this account does not own.
    RecipeNotOwned,
}

impl Standing {
    pub fn as_str(self) -> &'static str {
        match self {
            Standing::OwnedWithRecipe => "owned-with-recipe",
            Standing::OwnedNoRecipe => "owned-no-recipe",
            Standing::RecipeNotOwned => "recipe-not-owned",
        }
    }
}

/// One line of a library listing: an owned entitlement, a recipe, or both.
#[derive(Debug, Clone)]
pub struct Row<'a> {
    /// Spelled as whichever source it came from spelled it. The join itself is
    /// case-insensitive; this is for display.
    pub product_id: &'a str,
    pub entry: Option<&'a Entry>,
    pub recipe: Option<&'a Recipe>,
}

impl<'a> Row<'a> {
    pub fn standing(&self) -> Standing {
        match (self.entry.is_some(), self.recipe.is_some()) {
            (true, true) => Standing::OwnedWithRecipe,
            (true, false) => Standing::OwnedNoRecipe,
            (false, _) => Standing::RecipeNotOwned,
        }
    }

    /// A name to show. Only a recipe knows one -- collections returns ids, not
    /// titles, and the catalog lookup that would supply the rest is a separate
    /// anonymous call this module does not make -- so an owned title with no
    /// recipe shows as its product id.
    pub fn name(&self) -> &'a str {
        match self.recipe {
            Some(r) => &r.title.name,
            None => self.product_id,
        }
    }

    pub fn is_launchable(&self) -> bool {
        self.entry.is_some() && self.recipe.is_some()
    }
}

/// Pair each owned entitlement with its recipe, and keep the recipes that
/// matched nothing.
///
/// Sorted by product id so output is stable between runs, the same reason
/// `Recipe::load_dir` sorts. Matching is case-insensitive: a Store id is
/// uppercase, but `Recipe::matches` already accepts either case and a recipe
/// file written by hand should not fail to join over that.
pub fn join<'a>(entries: &'a [Entry], recipes: &'a [Recipe]) -> Vec<Row<'a>> {
    let mut rows: BTreeMap<String, Row<'a>> = BTreeMap::new();

    for entry in entries {
        let key = entry.product_id.to_ascii_uppercase();
        match rows.get_mut(&key) {
            // A product can appear more than once -- a trial and the full sku,
            // or several skus of the same game. One game is one row, and an
            // active entitlement beats a lapsed one so a stale row cannot hide
            // a live one.
            Some(existing) => {
                let stale = existing
                    .entry
                    .is_some_and(|e| e.status != EntitlementStatus::Active);
                if stale && entry.status == EntitlementStatus::Active {
                    existing.entry = Some(entry);
                    existing.product_id = &entry.product_id;
                }
            }
            None => {
                rows.insert(
                    key,
                    Row {
                        product_id: &entry.product_id,
                        entry: Some(entry),
                        recipe: None,
                    },
                );
            }
        }
    }

    for recipe in recipes {
        let key = recipe.title.product_id.to_ascii_uppercase();
        rows.entry(key)
            .and_modify(|row| row.recipe = Some(recipe))
            .or_insert(Row {
                product_id: &recipe.title.product_id,
                entry: None,
                recipe: Some(recipe),
            });
    }

    rows.into_values().collect()
}

/// Where the last listing is kept.
///
/// A launcher that shows an empty library every time it opens, until you
/// remember to press a button, is not showing you your library. The listing is
/// small, changes about as often as you buy a game, and is cheap to refresh --
/// so it is written down and shown immediately, and the button means "check
/// again" rather than "start".
pub fn cache_path(state_dir: &Path) -> PathBuf {
    state_dir.join("library.json")
}

/// Written in the same shape the service returns, so [`parse`] reads it back
/// and there is only one deserialiser to keep correct.
pub fn save_cache(state_dir: &Path, entries: &[Entry]) -> anyhow::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let document = serde_json::json!({ "count": entries.len(), "items": entries });
    std::fs::write(cache_path(state_dir), serde_json::to_vec_pretty(&document)?)?;
    Ok(())
}

/// The last listing, if there is one. A missing or unreadable cache is not an
/// error -- it only means the window opens asking rather than showing.
pub fn load_cache(state_dir: &Path) -> Option<Vec<Entry>> {
    let text = std::fs::read_to_string(cache_path(state_dir)).ok()?;
    parse(&text).ok()
}

/// How to ask the client for the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Ask for everything, including add-ons, passes and inactive rows.
    ///
    /// On by default, and deliberately: the client's own `--all` switch decides
    /// what to hide, and we would rather that decision be made in exactly one
    /// place -- `skip_reason` -- where the launcher can also report it. Ask for
    /// the filtered set and a row that vanished has no explanation.
    pub all: bool,
    /// Collections pages; this is the page size the client asks for. `None`
    /// leaves the client's default alone.
    pub page_size: Option<u32>,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            all: true,
            page_size: None,
        }
    }
}

/// Build the command that produces the JSON `parse` expects, and nothing else.
///
/// It is not run here. Running it is the caller's business, and keeping the
/// spawn out of this crate is what makes the rest of this module testable.
/// The JSON goes to stdout; progress and HTTP diagnostics go to stderr.
pub fn library_command(xodus_cli: impl AsRef<OsStr>, query: &Query) -> Command {
    let mut command = Command::new(xodus_cli);
    command.arg("library").arg("--json");
    if query.all {
        command.arg("--all");
    }
    if let Some(size) = query.page_size {
        command.arg("--page-size").arg(size.to_string());
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a real collections page -- extra fields included, because
    /// ignoring them is behaviour worth pinning -- with invented product ids.
    /// No identifier here belongs to an account.
    const PAYLOAD: &str = r#"{
      "count": 6,
      "items": [
        {
          "acquiredDate": "2026-01-04T18:11:32.4785484+00:00",
          "acquisitionType": "Single",
          "endDate": "9999-12-31T23:59:59.9999999+00:00",
          "id": "00000000-0000-0000-0000-000000000001",
          "localTicketReference": "1",
          "modifiedDate": "2026-01-04T18:11:32.4785484+00:00",
          "orderId": "00000000-0000-0000-0000-000000000101",
          "ownershipType": "Purchased",
          "productFamily": "Games",
          "productId": "9ZZTESTGAME1",
          "productKind": "Game",
          "productType": "Game",
          "purchasedCountry": "ZZ",
          "quantity": 1,
          "skuId": "0010",
          "skuType": "Full",
          "startDate": "2026-01-04T18:11:32.4785484+00:00",
          "status": "Active"
        },
        {
          "acquiredDate": "2026-02-10T09:00:00.0000000+00:00",
          "ownershipType": "Purchased",
          "productId": "9ZZTESTAPP02",
          "productType": "Application",
          "skuType": "Full",
          "status": "Active"
        },
        {
          "acquiredDate": "2026-02-11T09:00:00.0000000+00:00",
          "productId": "9ZZTESTDLC03",
          "productType": "Durable",
          "skuType": "Full",
          "status": "Active"
        },
        {
          "acquiredDate": "2026-02-12T09:00:00.0000000+00:00",
          "productId": "9ZZTESTPASS4",
          "productType": "Pass",
          "skuType": "Full",
          "status": "Active"
        },
        {
          "acquiredDate": "2026-03-01T09:00:00.0000000+00:00",
          "productId": "9ZZTESTTRL05",
          "productType": "Game",
          "skuType": "Trial",
          "status": "Expired"
        },
        {
          "acquiredDate": "2026-03-02T09:00:00.0000000+00:00",
          "productId": "9ZZTESTRVK06",
          "productType": "Game",
          "skuType": "Full",
          "status": "Revoked"
        }
      ]
    }"#;

    fn recipe(product_id: &str, name: &str, slug: &str) -> Recipe {
        Recipe::parse(&format!(
            r#"
schema = 1
[title]
product-id = "{product_id}"
name = "{name}"
slug = "{slug}"
[launch]
executable = 'Game.exe'
[status]
state = "playable"
summary = "Runs."
"#
        ))
        .expect("test recipe should parse")
    }

    #[test]
    fn parses_a_realistic_payload() {
        let entries = parse(PAYLOAD).expect("should parse");
        assert_eq!(entries.len(), 6);

        let first = &entries[0];
        assert_eq!(first.product_id, "9ZZTESTGAME1");
        assert_eq!(first.product_type, ProductType::Game);
        assert_eq!(first.status, EntitlementStatus::Active);
        assert_eq!(first.sku_type.as_deref(), Some("Full"));
        assert_eq!(first.ownership_type.as_deref(), Some("Purchased"));
        assert_eq!(
            first.acquired_date.as_deref(),
            Some("2026-01-04T18:11:32.4785484+00:00")
        );
    }

    #[test]
    fn a_bare_items_array_parses_too() {
        // `xodus-cli library --json | jq .items` is the obvious thing to do.
        let entries = parse(r#"[{"productId":"9ZZTESTGAME1","productType":"Game","status":"Active"}]"#)
            .expect("should parse");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn only_active_games_and_applications_survive_the_filter() {
        let entries = parse(PAYLOAD).unwrap();
        let kept = titles(entries.clone());
        let kept: Vec<&str> = kept.iter().map(|e| e.product_id.as_str()).collect();
        // Durable is an add-on, Pass is a subscription and neither installs;
        // Expired and Revoked are not entitlements any more.
        assert_eq!(kept, vec!["9ZZTESTGAME1", "9ZZTESTAPP02"]);

        let dropped = skipped(&entries);
        assert_eq!(dropped.len(), 4);
        let reason = |id: &str| {
            dropped
                .iter()
                .find(|(e, _)| e.product_id == id)
                .map(|(_, why)| *why)
        };
        assert_eq!(reason("9ZZTESTDLC03"), Some("not a game or application"));
        assert_eq!(reason("9ZZTESTPASS4"), Some("not a game or application"));
        assert_eq!(reason("9ZZTESTTRL05"), Some("entitlement is not active"));
        assert_eq!(reason("9ZZTESTRVK06"), Some("entitlement is not active"));
    }

    #[test]
    fn a_trial_of_an_owned_game_is_still_a_title() {
        // skuType is carried for display, not filtered on: a trial installs and
        // runs like anything else.
        let entries = parse(
            r#"[{"productId":"9ZZTESTGAME1","productType":"Game","skuType":"Trial","status":"Active"}]"#,
        )
        .unwrap();
        assert!(entries[0].is_title());
    }

    #[test]
    fn an_unfamiliar_product_type_is_skipped_with_its_own_name_intact() {
        // A new type should be dropped from the launchable list but still be
        // reportable as what it was, not as an empty string.
        let entries =
            parse(r#"[{"productId":"9ZZTESTNEW07","productType":"Doodad","status":"Active"}]"#)
                .unwrap();
        assert_eq!(entries[0].product_type.as_str(), "Doodad");
        assert_eq!(
            entries[0].skip_reason(),
            Some("not a game or application")
        );
    }

    #[test]
    fn a_row_missing_its_type_or_status_is_skipped_not_fatal() {
        // One odd row must not cost the whole listing.
        let entries = parse(r#"[{"productId":"9ZZTESTODD08"}]"#).expect("should parse");
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].is_title());
    }

    /// The cache is written and read by the same code that reads the service,
    /// so a value neither of them recognises has to survive the round trip.
    #[test]
    fn a_cached_listing_round_trips_including_values_we_do_not_recognise() {
        let dir = std::env::temp_dir().join(format!("xgdk-library-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_cache(&dir).is_none(), "no cache is not an error");

        let mut entries = parse(PAYLOAD).expect("parses");
        entries.push(Entry {
            product_id: "9ZZTESTODD07".into(),
            product_type: ProductType::Other("SomethingNew".into()),
            status: EntitlementStatus::Other("Pending".into()),
            sku_type: None,
            acquired_date: None,
            ownership_type: None,
        });

        save_cache(&dir, &entries).expect("writes");
        let read_back = load_cache(&dir).expect("reads");
        assert_eq!(read_back, entries);
        assert_eq!(read_back[6].product_type.as_str(), "SomethingNew");
        assert_eq!(read_back[6].status.as_str(), "Pending");

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }

    #[test]
    fn the_join_separates_owned_with_recipe_owned_without_and_recipe_unowned() {
        let entries = titles(parse(PAYLOAD).unwrap());
        let recipes = vec![
            recipe("9ZZTESTGAME1", "Test Game", "test-game"),
            recipe("9ZZTESTNONE9", "Unowned Game", "unowned"),
        ];

        let rows = join(&entries, &recipes);
        let seen: Vec<(&str, Standing)> = rows
            .iter()
            .map(|r| (r.product_id, r.standing()))
            .collect();
        // Sorted by product id, so this order is the contract.
        assert_eq!(
            seen,
            vec![
                ("9ZZTESTAPP02", Standing::OwnedNoRecipe),
                ("9ZZTESTGAME1", Standing::OwnedWithRecipe),
                ("9ZZTESTNONE9", Standing::RecipeNotOwned),
            ]
        );

        let launchable: Vec<&str> = rows
            .iter()
            .filter(|r| r.is_launchable())
            .map(|r| r.name())
            .collect();
        assert_eq!(launchable, vec!["Test Game"]);

        // Without a recipe there is no name to show, only the id.
        let app = rows.iter().find(|r| r.product_id == "9ZZTESTAPP02").unwrap();
        assert_eq!(app.name(), "9ZZTESTAPP02");
    }

    #[test]
    fn the_join_ignores_case_in_a_product_id() {
        // Recipe::matches is already case-insensitive; a hand-written recipe
        // must not silently fail to join over the same thing.
        let entries = parse(r#"[{"productId":"9ZZTESTGAME1","productType":"Game","status":"Active"}]"#)
            .unwrap();
        let recipes = vec![recipe("9zztestgame1", "Test Game", "test-game")];
        let rows = join(&entries, &recipes);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].standing(), Standing::OwnedWithRecipe);
    }

    #[test]
    fn several_skus_of_one_product_collapse_to_one_row_preferring_the_active_one() {
        // Shown twice in a UI is a bug, and so is a lapsed trial hiding the
        // purchase that replaced it.
        let entries = parse(
            r#"[
              {"productId":"9ZZTESTGAME1","productType":"Game","skuType":"Trial","status":"Expired"},
              {"productId":"9ZZTESTGAME1","productType":"Game","skuType":"Full","status":"Active"}
            ]"#,
        )
        .unwrap();
        let rows = join(&entries, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].entry.unwrap().sku_type.as_deref(),
            Some("Full"),
            "the active entitlement should win"
        );
    }

    #[test]
    fn a_recipe_for_a_title_nobody_owns_still_appears() {
        let recipes = vec![recipe("9ZZTESTNONE9", "Unowned", "unowned")];
        let rows = join(&[], &recipes);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].standing(), Standing::RecipeNotOwned);
        assert!(!rows[0].is_launchable());
        assert_eq!(rows[0].name(), "Unowned");
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        // Every one of these is something the client can actually put on stdout
        // on a bad day, and none of them should take the launcher down.
        assert!(parse("").is_err(), "empty output");
        assert!(parse("{\"count\": 1, \"items\": [").is_err(), "truncated");
        assert!(parse("HTTP 401 Unauthorized").is_err(), "not JSON at all");
        assert!(parse("42").is_err(), "not a document or an array");
        assert!(
            parse(r#"{"code":"InvalidMsaTicketFormat","message":"…"}"#).is_err(),
            "a service error body has no items"
        );
        assert!(
            parse(r#"{"count":1,"items":{"productId":"9ZZTESTGAME1"}}"#).is_err(),
            "items must be an array"
        );
        assert!(
            parse(r#"{"count":1,"items":[{"productType":"Game","status":"Active"}]}"#).is_err(),
            "an item with no product id is unusable, and dropping it quietly is how a game goes missing"
        );
    }

    #[test]
    fn the_error_for_a_service_error_body_does_not_blame_the_parser() {
        let err = parse(r#"{"code":"InvalidMsaTicketFormat"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("items"), "should name what was missing: {err}");
    }

    #[test]
    fn the_command_is_built_and_not_run() {
        let command = library_command("/opt/xodus/xodus-cli", &Query::default());
        assert_eq!(command.get_program(), "/opt/xodus/xodus-cli");
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // --all by default: the launcher filters, so it has to be given
        // everything to filter and to report on.
        assert_eq!(args, vec!["library", "--json", "--all"]);

        let narrow = library_command(
            "xodus-cli",
            &Query {
                all: false,
                page_size: Some(50),
            },
        );
        let args: Vec<String> = narrow
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["library", "--json", "--page-size", "50"]);
    }
}

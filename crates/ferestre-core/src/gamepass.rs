//! What a Game Pass subscription makes installable, and whether this account
//! has one.
//!
//! Two separate questions, deliberately kept apart, because conflating them is
//! how a launcher tells someone they cannot install a title they can:
//!
//!   - **What is in the catalogue** is public. `catalog.gamepass.com` answers
//!     anonymously, so the list of titles PC Game Pass includes is knowable
//!     without an account at all -- which is how someone decides whether the
//!     subscription is worth having.
//!   - **What this account is entitled to** is not knowable from a tier name.
//!     Microsoft has renamed and re-sliced the tiers repeatedly, an entitlement
//!     list carries revoked subscriptions beside active ones, and the same tier
//!     covers different catalogues in different markets. So this reports what
//!     the account holds and stops there, and the licence request at install
//!     time is what actually decides -- it returns a satisfaction failure with
//!     a reason, which is a better answer than any table here could give.
//!
//! Nothing in this module gives anyone a title they have not paid for. It reads
//! a public catalogue listing and an entitlement list the account already has.

use crate::library::Entry;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The "All PC Games" collection.
///
/// A `sigl` is one of the merchandised lists the Game Pass apps are built out
/// of, and this is the one that means "everything PC Game Pass includes". The
/// id is a constant of the service rather than something derivable, so
/// [`parse`] checks the collection's own title before trusting the ids under
/// it: a sigl that silently became something else would otherwise fill the
/// window with a few hundred wrong titles and look like it worked.
pub const ALL_PC_GAMES: &str = "fdd9e2a7-0fee-49f6-ad69-4354098401ff";

const HOST: &str = "https://catalog.gamepass.com";

/// The subscription products an entitlement list can carry.
///
/// Recognising one matters for a reason that has nothing to do with Game Pass:
/// without this table a subscription looks like a title with no packages, and
/// the library shows "the catalog lists no package for it" for a row that is
/// not a game at all.
pub const SUBSCRIPTIONS: [(&str, &str); 5] = [
    ("CFQ7TTC0K5DJ", "Game Pass Essential"),
    ("CFQ7TTC0P85B", "Game Pass Premium"),
    ("CFQ7TTC0KHS0", "Game Pass Ultimate"),
    ("CFQ7TTC0KGQ8", "PC Game Pass"),
    ("CFQ7TTC0K6L8", "Xbox Game Pass for Console"),
];

/// The name of the subscription this product id is, if it is one.
pub fn subscription_name(product_id: &str) -> Option<&'static str> {
    let id = product_id.to_ascii_uppercase();
    SUBSCRIPTIONS
        .iter()
        .find(|(pid, _)| *pid == id)
        .map(|(_, name)| *name)
}

/// The subscriptions this account holds that are still active.
///
/// Revoked rows sit in the entitlement list beside live ones -- a real account
/// carried seven revoked subscriptions and two active ones -- so the status is
/// the whole question and "there is a Game Pass row in the list" is not an
/// answer.
pub fn active(entries: &[Entry]) -> Vec<&'static str> {
    let mut held: Vec<&'static str> = entries
        .iter()
        .filter(|entry| entry.status == crate::library::EntitlementStatus::Active)
        .filter_map(|entry| subscription_name(&entry.product_id))
        .collect();
    held.sort_unstable();
    held.dedup();
    held
}

/// The product ids in a sigl response.
///
/// The first element is the collection describing itself and carries no `id`;
/// the rest are `{"id": "9NBLGGH2JHXJ"}`. Both shapes are in one array, which
/// is why this is a hand-written pass rather than a `Vec<Something>`.
pub fn parse(json: &str) -> Result<Vec<String>> {
    let document: serde_json::Value =
        serde_json::from_str(json).map_err(|e| anyhow!("not JSON: {e}"))?;
    let items = document
        .as_array()
        .ok_or_else(|| anyhow!("expected an array of collection entries"))?;

    // The header entry names the collection. Checking it is what stops a
    // changed or mistyped sigl id from quietly filling the window with the
    // wrong few hundred titles -- an empty list is obvious, a plausible list of
    // console games is not.
    let title = items
        .first()
        .and_then(|first| first.get("title"))
        .and_then(|title| title.as_str());
    if let Some(title) = title {
        if !title.to_ascii_lowercase().contains("pc") {
            return Err(anyhow!(
                "this collection is {title:?}, which is not a PC one -- check the sigl id"
            ));
        }
    }

    let ids: Vec<String> = items
        .iter()
        .filter_map(|item| item.get("id")?.as_str())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return Err(anyhow!("the collection listed no products"));
    }
    Ok(ids)
}

/// How to ask for the collection.
pub fn url(sigl: &str, market: &str, language: &str) -> String {
    format!("{HOST}/sigls/v2?id={sigl}&language={language}&market={market}")
}

/// The catalogue, from the service.
pub fn fetch(market: &str, language: &str) -> Result<Vec<String>> {
    let body = crate::http::get_text(&url(ALL_PC_GAMES, market, language))?;
    parse(&body)
}

/// The listing as it was last written down.
///
/// Kept because it is a few hundred ids that change about monthly, and because
/// the alternative is a window that shows nothing until a network round trip
/// finishes. `fetched` is recorded so a caller can decide it is old; nothing
/// here expires it, since a month-old list of Game Pass titles is still mostly
/// right and an empty one is not.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Listing {
    pub schema: u32,
    /// Seconds since the epoch.
    pub fetched: u64,
    pub market: String,
    pub product_ids: Vec<String>,
}

const LISTING_SCHEMA: u32 = 1;

pub fn cache_path(state_dir: &Path) -> PathBuf {
    state_dir.join("gamepass.json")
}

pub fn save_cache(state_dir: &Path, market: &str, product_ids: &[String]) -> Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let listing = Listing {
        schema: LISTING_SCHEMA,
        fetched: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
        market: market.to_string(),
        product_ids: product_ids.to_vec(),
    };
    std::fs::write(cache_path(state_dir), serde_json::to_vec_pretty(&listing)?)?;
    Ok(())
}

/// The cached listing, when there is one for this market.
///
/// A listing fetched for another market is discarded rather than shown: Game
/// Pass catalogues differ by region, and a German list labelled as this one is
/// a worse answer than no list.
pub fn load_cache(state_dir: &Path, market: &str) -> Option<Listing> {
    let text = std::fs::read_to_string(cache_path(state_dir)).ok()?;
    let listing: Listing = serde_json::from_str(&text).ok()?;
    (listing.schema == LISTING_SCHEMA && listing.market.eq_ignore_ascii_case(market))
        .then_some(listing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGL: &str = r#"[
      {"siglId": "fdd9e2a7-0fee-49f6-ad69-4354098401ff",
       "title": "All PC Games",
       "description": "Explore every game included with PC Game Pass"},
      {"id": "9NPDN9R45JX4"},
      {"id": "9P8LR42PTRGJ"},
      {"id": ""}
    ]"#;

    #[test]
    fn a_collection_becomes_product_ids() {
        let ids = parse(SIGL).expect("parses");
        assert_eq!(
            ids,
            vec!["9NPDN9R45JX4", "9P8LR42PTRGJ"],
            "the header describes the collection and is not a product; \
             an empty id is not one either"
        );
    }

    /// The check that a mistyped or repurposed sigl cannot pass silently. A
    /// wrong id does not fail -- it answers, with a few hundred titles for some
    /// other platform, which looks exactly like working.
    #[test]
    fn a_collection_that_is_not_a_pc_one_is_refused() {
        let console = r#"[
          {"siglId": "whatever", "title": "All Console Games"},
          {"id": "9NPDN9R45JX4"}
        ]"#;
        let error = parse(console).expect_err("should refuse");
        assert!(
            error.to_string().contains("not a PC one"),
            "the message has to say what went wrong: {error}"
        );
    }

    /// A collection with no header still parses. The check above is a guard
    /// against a wrong answer, not a demand that the service keep its shape.
    #[test]
    fn a_collection_with_no_header_is_still_read() {
        let bare = r#"[{"id": "9NPDN9R45JX4"}]"#;
        assert_eq!(parse(bare).expect("parses"), vec!["9NPDN9R45JX4"]);
    }

    #[test]
    fn an_empty_collection_is_an_error_rather_than_an_empty_library() {
        assert!(parse("[]").is_err());
        assert!(parse(r#"[{"siglId": "x", "title": "All PC Games"}]"#).is_err());
    }

    #[test]
    fn a_subscription_product_is_recognised_by_either_case() {
        assert_eq!(subscription_name("CFQ7TTC0KGQ8"), Some("PC Game Pass"));
        assert_eq!(
            subscription_name("cfq7ttc0khs0"),
            Some("Game Pass Ultimate")
        );
        assert_eq!(subscription_name("9NBLGGH2JHXJ"), None);
    }

    /// Revoked subscriptions sit in the entitlement list beside live ones, and
    /// a real account had seven of them. Reading "there is a Game Pass row" as
    /// "this account has Game Pass" would be wrong for every one of those.
    #[test]
    fn only_active_subscriptions_count() {
        let entries = crate::library::parse(
            r#"[
              {"productId": "CFQ7TTC0KHS0", "productType": "Pass", "status": "Revoked"},
              {"productId": "CFQ7TTC0P85B", "productType": "Pass", "status": "Active"},
              {"productId": "CFQ7TTC0P85B", "productType": "Pass", "status": "Active"},
              {"productId": "9NBLGGH2JHXJ", "productType": "Game", "status": "Active"}
            ]"#,
        )
        .expect("fixture parses");
        assert_eq!(
            active(&entries),
            vec!["Game Pass Premium"],
            "active only, and each named once"
        );
    }

    #[test]
    fn the_url_carries_the_market_and_language() {
        let url = url(ALL_PC_GAMES, "GB", "en-gb");
        assert!(url.contains(ALL_PC_GAMES));
        assert!(url.contains("market=GB") && url.contains("language=en-gb"));
    }

    /// A listing from another market is not this market's listing. Game Pass
    /// catalogues differ by region, and showing the wrong one is worse than
    /// showing none: every row would be installable-looking and wrong.
    #[test]
    fn a_cached_listing_is_only_used_for_the_market_it_was_fetched_for() {
        let dir = std::env::temp_dir().join(format!("ferestre-gp-{}", std::process::id()));
        save_cache(&dir, "US", &["9NBLGGH2JHXJ".into()]).expect("writes");
        assert!(load_cache(&dir, "US").is_some());
        assert!(load_cache(&dir, "GB").is_none(), "not this market's list");
        assert!(
            load_cache(&dir, "us").is_some(),
            "market case is not meaning"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

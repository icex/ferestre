//! Games Ferestre did not install: a Windows program somewhere on disk, added
//! by hand or imported from a Steam non-Steam shortcut.
//!
//! Such a game is a user recipe with `[title] kind = "local"` and an id that
//! starts with [`ID_PREFIX`]. The prefix has a dash, which a Store product id
//! (12 ASCII letters and digits) never has, so the two can never be confused.

/// What every local game's id starts with.
pub const ID_PREFIX: &str = "LOCAL-";

/// Whether `id` names a local game: `LOCAL-` (any case), then letters, digits
/// and dashes, at most 64 bytes in all.
pub fn is_local_id(id: &str) -> bool {
    id.len() <= 64
        && id.len() > ID_PREFIX.len()
        && id.is_char_boundary(ID_PREFIX.len())
        && id[..ID_PREFIX.len()].eq_ignore_ascii_case(ID_PREFIX)
        && id[ID_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The id of a game imported from a Steam shortcut with this app id.
pub fn steam_id(app_id: u32) -> String {
    format!("{ID_PREFIX}STEAM-{app_id}")
}

/// A new id for a game added by hand: `LOCAL-` and its name as a slug,
/// uppercased, then `-2`, `-3`... while `taken` says the id is in use.
pub fn new_id(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let mut slug = crate::recipe::slugify(name, "game");
    slug.truncate(40);
    let slug = slug.trim_end_matches('-').to_ascii_uppercase();
    let base = format!("{ID_PREFIX}{slug}");
    if !taken(&base) {
        return base;
    }
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("some suffix is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ids_are_recognised_and_bounded() {
        for id in ["LOCAL-STEAM-1", "local-cyberpunk-2077"] {
            assert!(is_local_id(id), "{id}");
        }
        let long = format!("LOCAL-{}", "A".repeat(70));
        for id in ["LOCAL-", "LOCAL-a/b", long.as_str(), "9NBLGGH2JHXJ"] {
            assert!(!is_local_id(id), "{id}");
        }
    }

    #[test]
    fn a_steam_shortcut_id_is_its_app_id() {
        assert_eq!(steam_id(2_876_543_210), "LOCAL-STEAM-2876543210");
    }

    #[test]
    fn a_new_id_is_the_name_and_never_one_taken() {
        assert_eq!(
            new_id("Cyberpunk 2077", &|id| id == "LOCAL-CYBERPUNK-2077"),
            "LOCAL-CYBERPUNK-2077-2"
        );
        assert_eq!(new_id("ゲーム", &|_| false), "LOCAL-GAME");
        let long = new_id(&"word ".repeat(20), &|_| false);
        assert!(is_local_id(&long) && !long.ends_with('-'), "{long}");
    }
}

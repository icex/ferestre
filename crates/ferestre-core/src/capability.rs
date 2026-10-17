//! What a recipe asks of the runtime, and whether a given runtime provides it.
//!
//! Recipes deliberately do not pin a Proton version. A title says it needs
//! `loader.memfd-main-image` and `appmodel.package-identity`; a runtime build
//! publishes what it provides. That way a title keeps working across runtime
//! updates instead of being frozen to the build it was tested against, and a
//! runtime that drops something fails loudly and specifically rather than
//! showing up as "the game stopped launching".

use std::collections::BTreeSet;

/// A capability identifier, e.g. `loader.memfd-main-image`.
pub type Capability = String;

// The names the launcher itself reasons about, as opposed to the ones it only
// passes from a recipe to a runtime. Kept together so that a rename in
// `titles/capabilities.toml` breaks one test here rather than silently turning
// a probe or an inferred default into a name no runtime will ever publish.

/// Proton keeps the inherited fd that holds the decrypted main image.
pub const MEMFD_MAIN_IMAGE: &str = "loader.memfd-main-image";
/// Wine reports the running package's identity to the title.
pub const PACKAGE_IDENTITY: &str = "appmodel.package-identity";
pub const XGAMERUNTIME_GAMESAVE: &str = "xgameruntime.gamesave";
pub const XGAMERUNTIME_NETWORKING: &str = "xgameruntime.networking";
pub const XGAMERUNTIME_PACKAGE: &str = "xgameruntime.package";
pub const XGAMERUNTIME_TASKQUEUE: &str = "xgameruntime.taskqueue";
pub const XGAMERUNTIME_USER: &str = "xgameruntime.user";
/// vkd3d-proton can keep an allocator retired mid-recording alive.
pub const RECORDING_ALLOCATOR_LIFETIME: &str = "d3d12.recording-allocator-lifetime";

/// Every name above, for the test that holds them to the registry.
pub const KNOWN: [&str; 8] = [
    MEMFD_MAIN_IMAGE,
    PACKAGE_IDENTITY,
    XGAMERUNTIME_GAMESAVE,
    XGAMERUNTIME_NETWORKING,
    XGAMERUNTIME_PACKAGE,
    XGAMERUNTIME_TASKQUEUE,
    XGAMERUNTIME_USER,
    RECORDING_ALLOCATOR_LIFETIME,
];

/// The outcome of checking a recipe's needs against a runtime's offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Required capabilities the runtime does not provide. Non-empty means the
    /// title cannot run on it.
    pub missing_required: BTreeSet<Capability>,
    /// Optional capabilities the runtime does not provide. The title runs, but
    /// something it would use is unavailable.
    pub missing_wanted: BTreeSet<Capability>,
}

impl Match {
    /// Whether the runtime can run the title at all.
    pub fn is_satisfied(&self) -> bool {
        self.missing_required.is_empty()
    }
}

/// Compare what a title requires and wants against what a runtime provides.
pub fn check<'a>(
    requires: impl IntoIterator<Item = &'a str>,
    wants: impl IntoIterator<Item = &'a str>,
    provided: &BTreeSet<Capability>,
) -> Match {
    let missing = |it: &mut dyn Iterator<Item = &'a str>| -> BTreeSet<Capability> {
        it.filter(|c| !provided.contains(*c))
            .map(str::to_string)
            .collect()
    };
    Match {
        missing_required: missing(&mut requires.into_iter()),
        missing_wanted: missing(&mut wants.into_iter()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provided(items: &[&str]) -> BTreeSet<Capability> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn satisfied_when_every_requirement_is_provided() {
        let m = check(["a", "b"], [], &provided(&["a", "b", "c"]));
        assert!(m.is_satisfied());
        assert!(m.missing_required.is_empty());
    }

    #[test]
    fn a_missing_requirement_is_not_satisfied_and_is_named() {
        let m = check(["a", "b"], [], &provided(&["a"]));
        assert!(!m.is_satisfied());
        assert_eq!(m.missing_required, provided(&["b"]));
    }

    #[test]
    fn a_missing_want_still_runs() {
        // The distinction is the whole point: a title should not be blocked
        // because something optional is unavailable.
        let m = check(["a"], ["b"], &provided(&["a"]));
        assert!(m.is_satisfied());
        assert_eq!(m.missing_wanted, provided(&["b"]));
    }

    #[test]
    fn every_name_the_launcher_or_a_recipe_uses_is_registered() {
        // titles/validate.py checks recipes in CI; this also covers the names
        // compiled into the launcher, which no script can see.
        let titles = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles");
        if !titles.exists() {
            return; // packaged crate without the repository around it
        }
        let registry = crate::runtime::Registry::load(&titles.join("capabilities.toml"))
            .expect("the registry parses");
        for name in KNOWN {
            assert!(
                registry.get(name).is_some(),
                "{name} is not in capabilities.toml"
            );
        }
        for recipe in crate::recipe::Recipe::load_dir(&titles).expect("recipes parse") {
            for name in recipe.runtime.requires.iter().chain(&recipe.runtime.wants) {
                assert!(
                    registry.get(name).is_some(),
                    "{} names {name}, which is not in capabilities.toml",
                    recipe.title.product_id
                );
            }
        }
    }

    #[test]
    fn a_runtime_providing_nothing_reports_every_requirement() {
        let m = check(["a", "b"], ["c"], &provided(&[]));
        assert_eq!(m.missing_required, provided(&["a", "b"]));
        assert_eq!(m.missing_wanted, provided(&["c"]));
    }
}

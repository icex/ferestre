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
    fn a_runtime_providing_nothing_reports_every_requirement() {
        let m = check(["a", "b"], ["c"], &provided(&[]));
        assert_eq!(m.missing_required, provided(&["a", "b"]));
        assert_eq!(m.missing_wanted, provided(&["c"]));
    }
}

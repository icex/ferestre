//! Which WinRT classes a prefix knows how to activate.
//!
//! A patched runtime can implement a runtime class that Wine does not ship, but
//! implementing it is only half the job: `RoGetActivationFactory` finds a class
//! by looking it up in the prefix registry, so a class nothing registered is a
//! class no title can activate, however complete the implementation behind it.
//!
//! Registration normally happens when the prefix is built. Proton copies new
//! prefixes from a default one baked at *runtime build* time, so a class added
//! to the runtime after that prefix was baked is missing from every prefix made
//! from it -- and a prefix that already exists is never revisited at all.
//! Measured on this machine: the shipped default prefix carries 148 activatable
//! classes and not one of them is `RegionPolicyEvaluator`, a class the runtime
//! has implemented and advertised for months.
//!
//! So the launcher checks, and adds what is missing, before a title asks.

use std::path::{Path, PathBuf};

/// A runtime class the runtime implements, and the dll that hosts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    /// The activatable class id, e.g. `Windows.Foundation.Diagnostics.LoggingChannel`.
    pub name: String,
    /// The hosting dll's file name, e.g. `windows.system.profile.systemid.dll`.
    pub dll: String,
}

impl Class {
    pub fn new(name: &str, dll: &str) -> Self {
        Class {
            name: name.to_string(),
            dll: dll.to_string(),
        }
    }

    /// The registry key a prefix records it under.
    pub fn key(&self) -> String {
        format!(
            "HKLM\\Software\\Microsoft\\WindowsRuntime\\ActivatableClassId\\{}",
            self.name
        )
    }

    /// Where the dll lives from inside the prefix.
    pub fn dll_path(&self) -> String {
        format!("C:\\windows\\system32\\{}", self.dll)
    }
}

/// `system.reg` inside a prefix.
pub fn registry_path(prefix: &Path) -> PathBuf {
    prefix.join("pfx/system.reg")
}

/// The classes this prefix does not know about yet.
///
/// Matching is on the section header rather than on any occurrence of the name,
/// because a class id also appears inside other values -- a substring search
/// reports a class as present when something merely mentions it, and then the
/// launcher skips the one registration that was actually needed.
///
/// A registry that cannot be read is treated as knowing nothing, which errs
/// towards writing a key that is already there: `reg add` overwrites and the
/// result is the same either way.
pub fn missing<'a>(system_reg: &str, classes: &'a [Class]) -> Vec<&'a Class> {
    classes
        .iter()
        .filter(|class| !registers(system_reg, &class.name))
        .collect()
}

fn registers(system_reg: &str, class: &str) -> bool {
    // Wine writes `[Software\\Microsoft\\...\\ActivatableClassId\\<class>] <time>`,
    // with every backslash doubled and the class id at the end of the path.
    let needle = format!("\\\\ActivatableClassId\\\\{class}]");
    system_reg
        .lines()
        .any(|line| line.starts_with('[') && line.contains(&needle))
}

/// Read the prefix and say what is missing. An unreadable prefix means "all of
/// them", for the reason given on [`missing`].
pub fn missing_in_prefix<'a>(prefix: &Path, classes: &'a [Class]) -> Vec<&'a Class> {
    let text = std::fs::read_to_string(registry_path(prefix)).unwrap_or_default();
    missing(&text, classes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real fragment, doubled backslashes and all.
    const REG: &str = r#"WINE REGISTRY Version 2

[Software\\Microsoft\\WindowsRuntime\\ActivatableClassId\\Windows.Internal.System.Profile.RegionPolicyEvaluator] 1788641892
#time=1dd3d7943babc00
"DllPath"="C:\\windows\\system32\\windows.system.profile.systemid.dll"

[Software\\Microsoft\\WindowsRuntime\\ActivatableClassId\\Windows.Foundation.Diagnostics.LoggingChannelOptions] 1788641892
"DllPath"="C:\\windows\\system32\\windows.system.profile.systemid.dll"
"#;

    fn classes() -> Vec<Class> {
        vec![
            Class::new(
                "Windows.Internal.System.Profile.RegionPolicyEvaluator",
                "windows.system.profile.systemid.dll",
            ),
            Class::new(
                "Windows.Foundation.Diagnostics.LoggingChannelOptions",
                "windows.system.profile.systemid.dll",
            ),
            Class::new(
                "Windows.Foundation.Diagnostics.LoggingChannel",
                "windows.system.profile.systemid.dll",
            ),
        ]
    }

    #[test]
    fn only_what_the_prefix_does_not_already_have_is_reported() {
        let classes = classes();
        let missing = missing(REG, &classes);
        assert_eq!(missing.len(), 1);
        assert_eq!(
            missing[0].name,
            "Windows.Foundation.Diagnostics.LoggingChannel"
        );
    }

    /// The trap this exists to avoid. `LoggingChannel` is a prefix of
    /// `LoggingChannelOptions`, so a substring search says it is registered
    /// when only the longer one is -- and the launcher then skips the single
    /// registration that was needed. Anchoring on the closing bracket is what
    /// keeps the two apart.
    #[test]
    fn a_class_whose_name_prefixes_another_is_not_mistaken_for_it() {
        let only_options = Class::new(
            "Windows.Foundation.Diagnostics.LoggingChannel",
            "windows.system.profile.systemid.dll",
        );
        assert!(
            !registers(REG, &only_options.name),
            "LoggingChannelOptions being present does not register LoggingChannel"
        );
        assert!(
            REG.contains("Windows.Foundation.Diagnostics.LoggingChannel"),
            "and a naive substring search would have said otherwise"
        );
    }

    /// A mention is not a registration: the class id turns up inside values and
    /// in other keys, and only a section header under ActivatableClassId means
    /// a title can activate it.
    #[test]
    fn a_mention_somewhere_else_is_not_a_registration() {
        let elsewhere = r#"
[Software\\Classes\\Something] 1788641892
"Comment"="see Windows.Foundation.Diagnostics.LoggingChannel for details"
"#;
        assert!(!registers(
            elsewhere,
            "Windows.Foundation.Diagnostics.LoggingChannel"
        ));
    }

    #[test]
    fn an_unreadable_prefix_reports_everything_missing() {
        let classes = classes();
        let missing = missing_in_prefix(Path::new("/nonexistent/prefix"), &classes);
        assert_eq!(
            missing.len(),
            3,
            "writing a key twice is harmless; skipping one is not"
        );
    }

    #[test]
    fn a_class_names_its_key_and_its_dll_the_way_the_registry_does() {
        let class = Class::new("Windows.Foundation.Diagnostics.LoggingChannel", "host.dll");
        assert_eq!(
            class.key(),
            "HKLM\\Software\\Microsoft\\WindowsRuntime\\ActivatableClassId\\Windows.Foundation.Diagnostics.LoggingChannel"
        );
        assert_eq!(class.dll_path(), "C:\\windows\\system32\\host.dll");
    }
}

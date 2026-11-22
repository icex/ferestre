//! The launch settings a person can choose for a title without knowing their
//! names.
//!
//! A recipe's `[launch.env]` is free-form, which is right for the file and
//! wrong for a person: nobody can guess that native Wayland is
//! `FERESTRE_NATIVE_WAYLAND=1` or that Halo needs `XGR_STRICT_MANUAL_QUEUES`.
//! This is the list the recipe editor turns into switches and menus. Anything
//! not here, or set to a value its control cannot show, stays an ordinary
//! variable, so nothing a recipe says is lost by opening it in the editor.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Display,
    Controllers,
    Compatibility,
    AntiCheat,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Display,
        Section::Controllers,
        Section::Compatibility,
        Section::AntiCheat,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Display => "Display and performance",
            Section::Controllers => "Controllers",
            Section::Compatibility => "Compatibility",
            Section::AntiCheat => "Anti-cheat",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// On or off. Each side is the value written, or `None` for leaving the
    /// variable out, which is what the launcher does when nobody chose.
    Switch {
        on: Option<&'static str>,
        off: Option<&'static str>,
    },
    /// One of a few values; the first is the default and must be `None`.
    Choice(&'static [(Option<&'static str>, &'static str)]),
}

#[derive(Debug, Clone, Copy)]
pub struct LaunchOption {
    pub key: &'static str,
    pub section: Section,
    pub title: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// A runtime capability the setting does nothing without.
    pub capability: Option<&'static str>,
}

impl LaunchOption {
    /// Whether a recipe value can be shown by this option's control.
    pub fn represents(&self, value: &str) -> bool {
        match self.kind {
            Kind::Switch { on, off } => on == Some(value) || off == Some(value),
            Kind::Choice(choices) => choices.iter().any(|(v, _)| *v == Some(value)),
        }
    }
}

const SWITCH_ON: Kind = Kind::Switch {
    on: Some("1"),
    off: None,
};
/// On unless turned off, for the automatic behaviours a title can decline.
const SWITCH_OFF: Kind = Kind::Switch {
    on: None,
    off: Some("0"),
};

pub const OPTIONS: &[LaunchOption] = &[
    LaunchOption {
        key: "FERESTRE_NATIVE_WAYLAND",
        section: Section::Display,
        title: "Native Wayland",
        help: "Use Wine's Wayland driver instead of XWayland. Only for games tested with it; it also enables automatic HDR.",
        kind: SWITCH_ON,
        capability: None,
    },
    LaunchOption {
        key: "FERESTRE_GAMESCOPE",
        section: Section::Display,
        title: "Gamescope",
        help: "Run the game inside Gamescope. Experimental.",
        kind: Kind::Choice(&[
            (None, "Automatic"),
            (Some("1"), "Use Gamescope"),
            (Some("0"), "Never use Gamescope"),
        ]),
        capability: None,
    },
    LaunchOption {
        key: "PROTON_ENABLE_HDR",
        section: Section::Display,
        title: "Automatic HDR",
        help: "Turn on HDR when the display path and monitor support it.",
        kind: SWITCH_OFF,
        capability: None,
    },
    LaunchOption {
        key: "FERESTRE_PERFORMANCE",
        section: Section::Display,
        title: "Performance profile while playing",
        help: "Hold the CachyOS performance profile, or use GameMode elsewhere, while the game runs.",
        kind: SWITCH_OFF,
        capability: None,
    },
    LaunchOption {
        key: "FERESTRE_DESKTOP_INTEGRATION",
        section: Section::Display,
        title: "Desktop integration",
        help: "Choose the display path, HDR and performance profile automatically. Off keeps the older launch path.",
        kind: SWITCH_OFF,
        capability: None,
    },
    LaunchOption {
        key: "PROTON_ENABLE_HIDRAW",
        section: Section::Controllers,
        title: "Raw HID for PlayStation controllers",
        help: "Give the game the controller's own HID device, for titles that show PlayStation button prompts only that way.",
        kind: SWITCH_ON,
        capability: None,
    },
    LaunchOption {
        key: "FERESTRE_WINHTTP_HTTP2",
        section: Section::Compatibility,
        title: "HTTP/2 for WinHTTP",
        help: "Speak HTTP/2 to services that require it, as Gears of War: E-Day's do.",
        kind: SWITCH_ON,
        capability: Some("winhttp.http2"),
    },
    LaunchOption {
        key: "XGR_STRICT_MANUAL_QUEUES",
        section: Section::Compatibility,
        title: "Strict manual task queues",
        help: "Run a title's manually dispatched work only when the title dispatches it, on its own thread.",
        kind: SWITCH_ON,
        capability: Some("xgameruntime.manual-queue-thread-affinity"),
    },
    LaunchOption {
        key: "XGR_PARALLEL_THREADPOOL_QUEUES",
        section: Section::Compatibility,
        title: "Parallel thread-pool queues",
        help: "Run thread-pool work concurrently, for titles whose callbacks wait on each other.",
        kind: SWITCH_ON,
        capability: None,
    },
    LaunchOption {
        key: "XGR_STORE_CATALOG",
        section: Section::Compatibility,
        title: "Live Store catalog",
        help: "Answer Store product queries with live Microsoft catalog data.",
        kind: SWITCH_ON,
        capability: Some("xgameruntime.store-catalog"),
    },
    LaunchOption {
        key: "FERESTRE_IMAGE_VIEW",
        section: Section::Compatibility,
        title: "Game files at their real paths",
        help: "Show the licensed game files at their installed paths in a private view, for anti-cheat that checks where the game is.",
        kind: Kind::Choice(&[(None, "Automatic"), (Some("real-path"), "Always")]),
        capability: None,
    },
    LaunchOption {
        key: "FERESTRE_EAC_LINUX_LIBRARY",
        section: Section::AntiCheat,
        title: "Easy Anti-Cheat Linux library",
        help: "Download EAC's Linux library, checked against its known hash, for a package that does not ship it.",
        kind: Kind::Choice(&[(None, "Not needed"), (Some("pinned"), "Download it")]),
        capability: None,
    },
];

pub fn find(key: &str) -> Option<&'static LaunchOption> {
    OPTIONS.iter().find(|o| o.key == key)
}

/// A recipe's environment, split into the values the controls show and the
/// rest, in the order the editor lists them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub chosen: BTreeMap<&'static str, String>,
    pub other: Vec<(String, String)>,
}

pub fn split(env: &BTreeMap<String, String>) -> Split {
    let mut result = Split::default();
    for (key, value) in env {
        match find(key) {
            Some(option) if option.represents(value) => {
                result.chosen.insert(option.key, value.clone());
            }
            _ => result.other.push((key.clone(), value.clone())),
        }
    }
    result
}

/// Put the two halves back together, refusing a variable named twice or a
/// name that cannot be one.
pub fn join(
    chosen: &BTreeMap<&'static str, String>,
    other: &[(String, String)],
) -> Result<BTreeMap<String, String>, String> {
    let mut env: BTreeMap<String, String> = chosen
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    for (key, value) in other {
        let key = key.trim();
        if key.is_empty() {
            if value.trim().is_empty() {
                continue; // a row added and left empty
            }
            return Err(format!(
                "The variable set to {:?} has no name.",
                value.trim()
            ));
        }
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || key.starts_with(|c: char| c.is_ascii_digit())
        {
            return Err(format!(
                "{key:?} is not a variable name: use letters, digits and underscores."
            ));
        }
        if chosen.contains_key(key) {
            let title = find(key).map_or(key, |o| o.title);
            return Err(format!(
                "{key} is the \"{title}\" setting above; change it there or remove this variable."
            ));
        }
        if env
            .insert(key.to_string(), value.trim().to_string())
            .is_some()
        {
            return Err(format!("{key} is listed twice."));
        }
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn every_option_is_well_formed() {
        let mut keys = std::collections::HashSet::new();
        for option in OPTIONS {
            assert!(keys.insert(option.key), "{} is listed twice", option.key);
            match option.kind {
                Kind::Switch { on, off } => assert_ne!(on, off, "{}", option.key),
                Kind::Choice(choices) => {
                    assert!(choices.len() >= 2, "{}", option.key);
                    assert_eq!(
                        choices[0].0, None,
                        "{}: the default comes first",
                        option.key
                    );
                    assert!(
                        choices[1..].iter().all(|(v, _)| v.is_some()),
                        "{}",
                        option.key
                    );
                }
            }
        }
    }

    /// The point of the list: every setting a shipped recipe uses appears as a
    /// control, so nobody meets one only as a bare variable.
    #[test]
    fn every_shipped_recipe_setting_has_a_control() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles");
        if !dir.exists() {
            return;
        }
        for recipe in crate::recipe::Recipe::load_dir(&dir).expect("recipes parse") {
            let split = split(&recipe.launch.env);
            assert!(
                split.other.is_empty(),
                "{} sets {:?}, which the editor would show as a bare variable",
                recipe.title.slug,
                split.other
            );
        }
    }

    /// Every capability an option names exists, so the editor's "needs a
    /// runtime with" note points at something real.
    #[test]
    fn named_capabilities_exist() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../titles/capabilities.toml");
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for option in OPTIONS {
            if let Some(capability) = option.capability {
                assert!(
                    text.contains(&format!("name = \"{capability}\"")),
                    "{} names unknown capability {capability}",
                    option.key
                );
            }
        }
    }

    #[test]
    fn a_split_environment_joins_back_unchanged() {
        let original = env(&[
            ("FERESTRE_NATIVE_WAYLAND", "1"),
            ("FERESTRE_GAMESCOPE", "0"),
            ("DXVK_HUD", "fps"),
            ("FERESTRE_EAC_LINUX_LIBRARY", "/home/me/eac.so"),
        ]);
        let split = split(&original);
        assert_eq!(split.chosen.len(), 2);
        assert_eq!(
            split.other,
            vec![
                ("DXVK_HUD".to_string(), "fps".to_string()),
                // A path is not one of the menu's values, so it stays as typed.
                (
                    "FERESTRE_EAC_LINUX_LIBRARY".to_string(),
                    "/home/me/eac.so".to_string()
                ),
            ]
        );
        assert_eq!(join(&split.chosen, &split.other).unwrap(), original);
    }

    #[test]
    fn joining_refuses_what_would_not_round_trip() {
        let mut chosen = BTreeMap::new();
        chosen.insert("FERESTRE_NATIVE_WAYLAND", "1".to_string());
        let row = |k: &str, v: &str| vec![(k.to_string(), v.to_string())];

        let twice = [row("A", "1"), row("A", "2")].concat();
        assert!(join(&chosen, &twice).unwrap_err().contains("twice"));
        assert!(join(&chosen, &row("FERESTRE_NATIVE_WAYLAND", "1"))
            .unwrap_err()
            .contains("Native Wayland"));
        assert!(join(&chosen, &row("BAD NAME", "1")).is_err());
        assert!(join(&chosen, &row("1ABC", "1")).is_err());
        assert!(join(&chosen, &row("", "value")).is_err());
        assert_eq!(
            join(&chosen, &row("  ", "  ")).unwrap().len(),
            1,
            "an empty row is ignored"
        );
    }
}

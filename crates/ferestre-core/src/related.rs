//! Installed content associated by the packages' own MicrosoftGame.config.
//! Installation discovery is separate from Store entitlement and licensing.
use crate::install;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Package {
    pub product: String,
    pub dir: PathBuf,
    pub content: Option<String>,
}

pub fn installed(state: &Path, product: &str, game: &Path) -> Vec<Package> {
    let Some(identity) = install::package_identity(game) else {
        return vec![];
    };
    install::all(state)
        .into_iter()
        .filter_map(|record| {
            if record.product_id.len() != 12
                || !record.product_id.bytes().all(|b| b.is_ascii_alphanumeric())
                || record.product_id.eq_ignore_ascii_case(product)
                || record.is_stale()
                || install::is_partial(&record.dir)
            {
                return None;
            }
            let xml = std::fs::read_to_string(install::game_config(&record.dir)?).ok()?;
            let allowed = xml
                .split("<AllowedProduct>")
                .skip(1)
                .filter_map(|s| s.split_once("</AllowedProduct>").map(|p| p.0.trim()))
                .any(|id| id.eq_ignore_ascii_case(product));
            let dependency = xml
                .split("<MainPackageDependency")
                .skip(1)
                .filter_map(|s| s.split_once('>').map(|p| p.0))
                .any(|s| {
                    s.contains(&format!("Name=\"{identity}\""))
                        || s.contains(&format!("Name='{identity}'"))
                });
            if !allowed || !dependency {
                return None;
            }
            let dir = record.dir.canonicalize().ok()?;
            if dir.to_str()?.contains(['\n', '\r']) {
                return None;
            }
            Some(Package {
                product: record.product_id,
                content: record.content_ids.first().cloned(),
                dir,
            })
        })
        .take(128)
        .collect()
}

pub fn environment(packages: &[Package]) -> String {
    packages
        .iter()
        .filter_map(|p| p.dir.to_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Content identities are supplied to the credential service, which obtains a
/// separate authenticated licence for each one. Installation alone is no grant.
pub fn service_binding(packages: &[Package]) -> String {
    let binding: std::collections::BTreeMap<_, _> = packages
        .iter()
        .filter_map(|p| p.content.as_ref().map(|content| (&p.product, content)))
        .collect();
    serde_json::to_string(&binding).expect("string map")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_complete_related_content_is_bound_even_outside_the_default_library() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let game = root.path().join("game");
        std::fs::create_dir(&game).unwrap();
        std::fs::write(
            game.join("MicrosoftGame.config"),
            "<Game><Identity Name=\"Fixture.Parent\"/></Game>",
        )
        .unwrap();
        for (product, allowed, dependency, partial) in [
            ("9TESTADDON01", "9TESTPARENT0", "Fixture.Parent", false),
            ("9TESTADDON02", "9OTHERPARENT", "Fixture.Parent", false),
            ("9TESTADDON03", "9TESTPARENT0", "Fixture.Other", false),
            ("9TESTADDON04", "9TESTPARENT0", "Fixture.Parent", true),
        ] {
            let dir = root.path().join("other-library").join(product);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("MicrosoftGame.config"),format!("<Game><AllowedProducts><AllowedProduct>{allowed}</AllowedProduct></AllowedProducts><DesktopRegistration><MainPackageDependency Name=\"{dependency}\"/></DesktopRegistration></Game>")).unwrap();
            std::fs::write(
                dir.join(if partial {
                    install::PARTIAL_CONTAINER
                } else {
                    ".xodus-streaming.msixvc"
                }),
                b"fixture",
            )
            .unwrap();
            let record = install::Record {
                product_id: product.into(),
                dir,
                content_ids: vec![],
                installed_at: None,
                package_version: None,
            };
            install::save(&state, &record).unwrap();
        }
        let packages = installed(&state, "9TESTPARENT0", &game);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].product, "9TESTADDON01");
        let paths = environment(&packages);
        assert!(paths.contains("other-library"));
        assert!(paths.ends_with("9TESTADDON01"));
        assert_eq!(service_binding(&packages), "{}");
        let mut packages = packages;
        packages[0].content = Some("00000000-0000-0000-0000-000000000001".into());
        assert!(service_binding(&packages).contains("9TESTADDON01"));
    }
}

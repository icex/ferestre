//! Store ownership and Xbox player selection. Tokens belong to the client.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const CAPABILITY: &str = "xgameruntime.store-player-accounts";
pub const CLIENT_CAPABILITY: &str = "store-player-accounts";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub label: String,
    pub signed_in: bool,
    pub legacy_saves: bool,
    pub save_namespace: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Profiles {
    pub schema: u32,
    pub capabilities: Vec<String>,
    pub default_profile: Option<String>,
    pub profiles: Vec<Profile>,
}

fn valid_hex(s: &str, length: usize) -> bool {
    s.len() == length
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub fn parse_profiles(bytes: &[u8]) -> Result<Profiles> {
    let profiles: Profiles =
        serde_json::from_slice(bytes).context("Invalid account profile response")?;
    if profiles.schema != 1 || !profiles.capabilities.iter().any(|s| s == CLIENT_CAPABILITY) {
        bail!("Update the Xodus client to use separate Store and Player accounts");
    }
    let mut seen = std::collections::BTreeSet::new();
    for profile in &profiles.profiles {
        if !valid_hex(&profile.id, 32)
            || !valid_hex(&profile.save_namespace, 64)
            || profile.label.is_empty()
            || !seen.insert(&profile.id)
        {
            bail!("Invalid or duplicate account profile");
        }
    }
    if profiles
        .default_profile
        .as_ref()
        .is_some_and(|id| !seen.contains(id))
    {
        bail!("Default account profile is unavailable");
    }
    Ok(profiles)
}

pub fn fetch(client: &Path) -> Result<Profiles> {
    let output = Command::new(client)
        .args(["account-profiles", "--json"])
        .output()
        .context("Cannot read account profiles")?;
    if !output.status.success() {
        bail!("Cannot read account profiles; update the client and check that your keychain is unlocked");
    }
    parse_profiles(&output.stdout)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub store: String,
    pub player: String,
    pub generation: u64,
}

impl Selection {
    pub fn split(&self) -> bool {
        self.store != self.player
    }

    pub fn validate(&self) -> Result<()> {
        if !valid_hex(&self.store, 32) || !valid_hex(&self.player, 32) {
            bail!("Invalid Store or Player account selection");
        }
        Ok(())
    }

    pub fn selected<'a>(&self, profiles: &'a Profiles) -> Result<(&'a Profile, &'a Profile)> {
        self.validate()?;
        let role = |id: &str, role: &str| -> Result<&'a Profile> {
            profiles.profiles.iter().find(|p| p.id==id && p.signed_in)
                .with_context(||format!("{role} account is signed out or unavailable. Open Accounts and sign in again."))
        };
        Ok((role(&self.store, "Store")?, role(&self.player, "Player")?))
    }

    pub fn apply(&self, command: &mut Command) {
        command
            .env("XODUS_ACCOUNT_PROFILE", &self.store)
            .env("XODUS_STORE_PROFILE", &self.store)
            .env("XODUS_PLAYER_PROFILE", &self.player);
    }

    /// Hash the immutable pair and generation alongside the service fingerprint.
    pub fn service_key(&self, service_id: &str) -> String {
        format!(
            "{:x}",
            Sha256::digest(format!(
                "ferestre-accounts-v1:{service_id}:{}:{}:{}",
                self.store, self.player, self.generation
            ))
        )
    }
}

pub fn load(config_dir: &Path) -> Result<Option<Selection>> {
    let bytes = match std::fs::read(config_dir.join("account-roles.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let selection: Selection =
        serde_json::from_slice(&bytes).context("Invalid account role settings")?;
    selection.validate()?;
    Ok(Some(selection))
}

pub fn launch_selection(config_dir: &Path) -> Result<Option<Selection>> {
    match std::env::var("FERESTRE_ACCOUNT_ROLES") {
        Ok(value) => {
            let roles: Selection =
                serde_json::from_str(&value).context("Invalid launch account snapshot")?;
            roles.validate()?;
            Ok(Some(roles))
        }
        Err(std::env::VarError::NotPresent) => load(config_dir),
        Err(error) => Err(error.into()),
    }
}

pub fn save(config_dir: &Path, store: &str, player: &str) -> Result<Selection> {
    let generation = load(config_dir)?
        .map_or(0, |s| s.generation)
        .checked_add(1)
        .context("Account selection generation exhausted")?;
    let selection = Selection {
        store: store.into(),
        player: player.into(),
        generation,
    };
    selection.validate()?;
    crate::cache::write(
        &config_dir.join("account-roles.json"),
        &serde_json::to_vec(&selection)?,
    )?;
    Ok(selection)
}

pub fn private_state(state: &Path, selection: Option<&Selection>) -> PathBuf {
    selection.map_or_else(
        || state.to_owned(),
        |s| state.join("accounts").join(&s.store),
    )
}

pub fn player_prefix(original: &Path, player: &Profile) -> Result<PathBuf> {
    if player.legacy_saves {
        return Ok(original.to_owned());
    }
    if !valid_hex(&player.save_namespace, 64) {
        bail!("Invalid Player save namespace");
    }
    let parent = original.parent().context("Invalid title prefix path")?;
    let name = original.file_name().context("Invalid title prefix name")?;
    Ok(parent
        .join("players")
        .join(&player.save_namespace)
        .join(name))
}

pub fn require_service(binary: &Path) -> Result<()> {
    // Old services ignore argv and would start instead of answering a probe.
    // Inspect the new protocol marker before executing --capabilities.
    let bytes = std::fs::read(binary).context("Cannot read the Xbox service")?;
    let marker = b"FERESTRE_ACCOUNT_ROLES_SERVICE_V1";
    if !bytes.windows(marker.len()).any(|part| part == marker) {
        bail!("Update the Xodus service before launching with separate accounts");
    }
    let output = Command::new(binary)
        .arg("--capabilities")
        .output()
        .context("Cannot check Xbox account service support")?;
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    if !output.status.success()
        || value["schema"] != 1
        || !value["capabilities"]
            .as_array()
            .is_some_and(|a| a.iter().any(|s| s == CLIENT_CAPABILITY))
    {
        bail!("Update the Xodus service before launching with separate accounts");
    }
    Ok(())
}

pub fn service_fingerprint(binary: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(binary)?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWNER: &str = "11111111111111111111111111111111";
    const PLAYER: &str = "22222222222222222222222222222222";
    #[test]
    fn pairs_keep_cache_service_and_player_saves_separate() {
        let root = tempfile::tempdir().unwrap();
        let a = save(root.path(), OWNER, PLAYER).unwrap();
        let b = save(root.path(), PLAYER, OWNER).unwrap();
        assert_ne!(
            a.service_key("fixture-service"),
            b.service_key("fixture-service")
        );
        assert_ne!(
            a.service_key("fixture-service"),
            a.service_key("updated-service")
        );
        let c = save(root.path(), OWNER, PLAYER).unwrap();
        assert_ne!(
            a.service_key("fixture-service"),
            c.service_key("fixture-service")
        );
        assert_eq!(load(root.path()).unwrap(), Some(c));
        assert_ne!(
            private_state(root.path(), Some(&a)),
            private_state(root.path(), Some(&b))
        );
        let mut profile = Profile {
            id: PLAYER.into(),
            label: "Player fixture".into(),
            signed_in: true,
            legacy_saves: false,
            save_namespace: "a".repeat(64),
        };
        let prefix = Path::new("/games/prefixes/title");
        assert_ne!(player_prefix(prefix, &profile).unwrap(), prefix);
        profile.legacy_saves = true;
        assert_eq!(player_prefix(prefix, &profile).unwrap(), prefix);
        assert!(save(root.path(), "../escape", PLAYER).is_err());
    }
    #[test]
    fn signed_out_roles_and_old_protocols_are_refused() {
        let json = serde_json::json!({"schema":1,"capabilities":[CLIENT_CAPABILITY],"default_profile":OWNER,
            "profiles":[{"id":OWNER,"label":"Owner fixture","signed_in":true,"legacy_saves":true,"save_namespace":"a".repeat(64)},
                        {"id":PLAYER,"label":"Player fixture","signed_in":false,"legacy_saves":false,"save_namespace":"b".repeat(64)}]});
        let profiles = parse_profiles(&serde_json::to_vec(&json).unwrap()).unwrap();
        let pair = Selection {
            store: OWNER.into(),
            player: PLAYER.into(),
            generation: 1,
        };
        assert!(pair
            .selected(&profiles)
            .unwrap_err()
            .to_string()
            .contains("Player"));
        assert!(parse_profiles(br#"{"schema":0,"profiles":[]}"#).is_err());
    }
}

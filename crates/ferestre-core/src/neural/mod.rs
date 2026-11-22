//! Neural rendering: DLSSNR-AMD installed into a game's folder.
//!
//! Everything here that is not UI: the pinned package Ferestre builds and
//! hosts ([`package`]), and the names and pinned values the rest of the
//! feature shares. The NVIDIA model is never part of a package; it comes from
//! the person's own `nvngx_dlssnr.dll`, accepted only by the hashes below.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod package;

/// A package's architecture, as its file name spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Arch {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "i686")]
    I686,
}

impl Arch {
    pub const ALL: [Arch; 2] = [Arch::X86_64, Arch::I686];

    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::I686 => "i686",
        }
    }
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Arch {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "x86_64" => Ok(Arch::X86_64),
            "i686" => Ok(Arch::I686),
            _ => bail!("unknown architecture {s:?} (x86_64 or i686)"),
        }
    }
}

/// How DLSSNR-AMD hooks into a game: upstream `install.sh`'s routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Route {
    #[serde(rename = "optiscaler")]
    Optiscaler,
    #[serde(rename = "reshade-feeder")]
    ReshadeFeeder,
    #[serde(rename = "reshade-feeder-vulkan")]
    ReshadeFeederVulkan,
    #[serde(rename = "reshade-feeder-dx9")]
    ReshadeFeederDx9,
}

impl Route {
    pub const ALL: [Route; 4] = [
        Route::Optiscaler,
        Route::ReshadeFeeder,
        Route::ReshadeFeederVulkan,
        Route::ReshadeFeederDx9,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Route::Optiscaler => "optiscaler",
            Route::ReshadeFeeder => "reshade-feeder",
            Route::ReshadeFeederVulkan => "reshade-feeder-vulkan",
            Route::ReshadeFeederDx9 => "reshade-feeder-dx9",
        }
    }
}

impl std::fmt::Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Route {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Route::ALL
            .into_iter()
            .find(|r| r.as_str() == s)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown route {s:?} (one of {})",
                    Route::ALL.map(Route::as_str).join(", ")
                )
            })
    }
}

/// The network DLSSNR-AMD starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Network {
    #[serde(rename = "CC_SMALL_FP8")]
    SmallFp8,
    #[serde(rename = "CC_SMALL_FP8_EXPERIMENTAL")]
    SmallFp8Experimental,
    #[serde(rename = "CC_SILVER_AARDWOLD")]
    SilverAardwold,
    #[serde(rename = "CC_SILVER_AARDWOLD_OPTIMIZED")]
    SilverAardwoldOptimized,
}

impl Network {
    pub const ALL: [Network; 4] = [
        Network::SmallFp8,
        Network::SmallFp8Experimental,
        Network::SilverAardwold,
        Network::SilverAardwoldOptimized,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Network::SmallFp8 => "CC_SMALL_FP8",
            Network::SmallFp8Experimental => "CC_SMALL_FP8_EXPERIMENTAL",
            Network::SilverAardwold => "CC_SILVER_AARDWOLD",
            Network::SilverAardwoldOptimized => "CC_SILVER_AARDWOLD_OPTIMIZED",
        }
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Network {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Network::ALL
            .into_iter()
            .find(|n| n.as_str() == s)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown network {s:?} (one of {})",
                    Network::ALL.map(Network::as_str).join(", ")
                )
            })
    }
}

/// An accepted `nvngx_dlssnr.dll`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DllVersion {
    pub version: String,
    pub sha256: String,
    /// Whether it carries `CC_SMALL_FP8` as well as `CC_SILVER_AARDWOLD`.
    pub small_model: bool,
}

/// A model file extracted from an accepted DLL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

/// What the DLL and the models must be. Owned, so tests can build one from
/// hashes they compute at run time.
#[derive(Debug, Clone)]
pub struct Expect {
    pub dlls: Vec<DllVersion>,
    pub aardwold: ModelFile,
    pub small: ModelFile,
}

/// `(version, sha256, carries the small model)`.
const PINNED_DLLS: [(&str, &str, bool); 2] = [
    (
        "310.9.16",
        "2bc19bc09a7b89dd44f73e2ac0108fcc586e2154bdd0830f242f4f040e28789f",
        true,
    ),
    (
        "310.8.0",
        "e16bcf15e16e13f527491cdf7845b2fe6521a738d8f7c9c721866a8496e1fc8e",
        false,
    ),
];

/// `(file, sha256, size)`.
const PINNED_AARDWOLD: (&str, &str, u64) = (
    "CC_SILVER_AARDWOLD.bin",
    "2b41c888cf4155b8958c665ba64018ab0bd25c85fc71a2b6db86d0d04d1f7fbd",
    147_756_560,
);
const PINNED_SMALL: (&str, &str, u64) = (
    "CC_SMALL_FP8.bin",
    "7e966e2639dbec6596aa0bac3b8761137d2c40ca8cb4aca30df7ca10043c6e33",
    113_917_248,
);

impl Expect {
    pub fn pinned() -> Expect {
        let model = |(file, sha256, size): (&str, &str, u64)| ModelFile {
            file: file.to_string(),
            sha256: sha256.to_string(),
            size,
        };
        Expect {
            dlls: PINNED_DLLS
                .iter()
                .map(|&(version, sha256, small_model)| DllVersion {
                    version: version.to_string(),
                    sha256: sha256.to_string(),
                    small_model,
                })
                .collect(),
            aardwold: model(PINNED_AARDWOLD),
            small: model(PINNED_SMALL),
        }
    }
}

/// Where neural rendering keeps its packages, downloads and models.
pub fn root(state_dir: &Path) -> PathBuf {
    state_dir.join("dlssnr")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_through_text_and_serde() {
        for route in Route::ALL {
            assert_eq!(route.as_str().parse::<Route>().unwrap(), route);
            assert_eq!(
                serde_json::to_string(&route).unwrap(),
                format!("\"{}\"", route.as_str())
            );
        }
        for network in Network::ALL {
            assert_eq!(network.as_str().parse::<Network>().unwrap(), network);
            assert_eq!(
                serde_json::to_string(&network).unwrap(),
                format!("\"{}\"", network.as_str())
            );
        }
        for arch in Arch::ALL {
            assert_eq!(arch.as_str().parse::<Arch>().unwrap(), arch);
            assert_eq!(
                serde_json::to_string(&arch).unwrap(),
                format!("\"{}\"", arch.as_str())
            );
        }
        assert!("dxgi".parse::<Route>().is_err());
    }

    #[test]
    fn the_pinned_expectation_is_the_constraints_table() {
        let expect = Expect::pinned();
        assert_eq!(expect.dlls.len(), 2);
        assert!(expect.dlls[0].small_model && !expect.dlls[1].small_model);
        assert_eq!(expect.aardwold.size, 147_756_560);
        assert_eq!(expect.small.file, "CC_SMALL_FP8.bin");
        assert_eq!(
            root(Path::new("/s")),
            PathBuf::from("/s/dlssnr"),
            "under the state directory"
        );
    }
}

//! What Microsoft's display catalog says about one game, for its details page.
//!
//! Asked for when the page opens and not cached: ratings change, and nothing
//! else in Ferestre needs it.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// What players gave it over one span of time.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerRating {
    pub span: String,
    pub average: f64,
    pub count: u64,
}

/// One thing the game needs, at least and ideally. Either may be empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub what: &'static str,
    pub minimum: String,
    pub recommended: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgeRating {
    pub system: String,
    pub rating: String,
    pub descriptors: Vec<String>,
}

/// A labelled value. Values are never empty.
pub type Row = (&'static str, String);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Details {
    pub description: String,
    /// Image addresses with no size on them: the banner, then screenshots.
    pub pictures: Vec<String>,
    pub developer: String,
    pub publisher: String,
    pub categories: Vec<String>,
    pub dates: Vec<Row>,
    pub release_notes: String,
    pub players: Vec<PlayerRating>,
    /// The catalog's own names, such as `XblAchievements`; see [`feature`].
    pub features: Vec<String>,
    pub purchases: Vec<Row>,
    pub requirements: Vec<Requirement>,
    pub requirement_notes: Vec<Row>,
    pub ages: Vec<AgeRating>,
    pub package: Vec<Row>,
    pub languages: Vec<String>,
    pub links: Vec<Row>,
    pub copyright: String,
}

/// Ask the catalog. A local game's id is not the Store's and is never sent.
pub fn fetch(product_id: &str, market: &str, language: &str) -> Result<Details> {
    if !crate::recipe::is_store_product_id(product_id) {
        bail!("{product_id} is not a Store product");
    }
    let url = crate::catalog::request_url(&[product_id.to_string()], market, language);
    parse(&crate::http::get_text(&url)?)
}

pub fn parse(json: &str) -> Result<Details> {
    let document: Value = serde_json::from_str(json).context("the catalog's answer")?;
    let product = &document["Products"][0];
    if !product.is_object() {
        bail!("the catalog has nothing about this game");
    }
    let local = &product["LocalizedProperties"][0];
    let market = &product["MarketProperties"][0];
    let properties = &product["Properties"];
    let sku = full_sku(product);
    let sku_properties = &sku["Properties"];
    let package = &sku_properties["Packages"][0];

    let description = [
        &local["ProductDescription"],
        &local["ShortDescription"],
        &sku["LocalizedProperties"][0]["SkuDescription"],
    ]
    .into_iter()
    .map(text)
    .find(|t| !t.is_empty())
    .unwrap_or_default();

    let notes = &sku["LocalizedProperties"][0];
    let legal = &notes["LegalText"];
    let hardware = &sku_properties["HardwareProperties"];

    let mut categories = list(&properties["Categories"]);
    let subcategory = text(&properties["Subcategory"]);
    if !subcategory.is_empty() && !categories.contains(&subcategory) {
        categories.push(subcategory);
    }

    let players = array(&market["UsageData"])
        .iter()
        .filter_map(|usage| {
            let count = usage["RatingCount"].as_u64().filter(|c| *c > 0)?;
            let span = text(&usage["AggregateTimeSpan"]);
            Some(PlayerRating {
                span: match span.as_str() {
                    "7Days" => "Last 7 days".into(),
                    "30Days" => "Last 30 days".into(),
                    "AllTime" => "All time".into(),
                    _ => span,
                },
                average: usage["AverageRating"].as_f64()?,
                count,
            })
        })
        .collect();

    let mut features: Vec<String> = array(&properties["Attributes"])
        .iter()
        .map(|a| text(&a["Name"]))
        .filter(|n| !n.is_empty())
        .collect();
    if sku_properties["XboxXPA"].as_bool() == Some(true) && !features.iter().any(|f| f == "XPA") {
        features.push("XPA".into());
    }

    let (minimum, minimum_codes) = hardware_codes(&hardware["MinimumHardware"]);
    let (recommended, recommended_codes) = hardware_codes(&hardware["RecommendedHardware"]);
    let mut requirements: Vec<Requirement> = [
        ("Processor", "MinimumProcessor", "RecommendedProcessor"),
        ("Graphics", "MinimumGraphics", "RecommendedGraphics"),
    ]
    .into_iter()
    .map(|(what, least, ideal)| Requirement {
        what,
        minimum: text(&hardware[least]),
        recommended: text(&hardware[ideal]),
    })
    .collect();
    for (index, what) in HARDWARE.into_iter().enumerate() {
        requirements.push(Requirement {
            what,
            minimum: minimum[index].join(", "),
            recommended: recommended[index].join(", "),
        });
    }
    requirements.push(Requirement {
        what: "Other (Store codes)",
        minimum: minimum_codes.join(", "),
        recommended: recommended_codes.join(", "),
    });
    requirements.retain(|r| !r.minimum.is_empty() || !r.recommended.is_empty());

    let ages = array(&market["ContentRatings"])
        .iter()
        .map(|rating| AgeRating {
            system: text(&rating["RatingSystem"]),
            rating: without_prefix(&text(&rating["RatingId"])),
            descriptors: ["RatingDescriptors", "InteractiveElements"]
                .iter()
                .flat_map(|key| array(&rating[*key]))
                .map(|v| descriptor(&without_prefix(&text(v))))
                .collect(),
        })
        .filter(|r| !r.rating.is_empty())
        .collect();

    let size = package["MaxDownloadSizeInBytes"]
        .as_u64()
        .filter(|bytes| *bytes > 0)
        .map(|bytes| format!("{:.1} GB", bytes as f64 / 1e9))
        .unwrap_or_default();

    Ok(Details {
        description,
        pictures: pictures(local),
        developer: text(&local["DeveloperName"]),
        publisher: text(&local["PublisherName"]),
        categories,
        dates: rows([
            ("Released", date(&market["OriginalReleaseDate"])),
            ("Last update", date(&sku_properties["LastUpdateDate"])),
            ("Store entry changed", date(&product["LastModifiedDate"])),
        ]),
        release_notes: text(&notes["ReleaseNotes"]),
        players,
        features,
        purchases: rows([
            (
                "Add-ons",
                yes(&properties["ExtendedClientMetadata"]["HasAddOns"]),
            ),
            (
                "In-app purchases",
                yes(&sku_properties["HasThirdPartyIAPs"]),
            ),
            (
                "Optimized for consoles",
                consoles(&properties["XboxConsoleGenOptimized"]),
            ),
            (
                "Plays on consoles",
                consoles(&properties["XboxConsoleGenCompatible"]),
            ),
        ]),
        requirements,
        requirement_notes: rows([
            ("Minimum", text(&notes["MinimumNotes"])),
            ("Recommended", text(&notes["RecommendedNotes"])),
        ]),
        ages,
        package: rows([
            ("Download size", size),
            ("Format", text(&package["PackageFormat"])),
            ("Architecture", list(&package["Architectures"]).join(", ")),
            ("Type", text(&product["ProductType"])),
            ("Product id", text(&product["ProductId"])),
            ("Package family", text(&properties["PackageFamilyName"])),
            ("Content id", text(&package["ContentId"])),
        ]),
        languages: list(&package["Languages"]),
        links: rows([
            ("Website", text(&local["PublisherWebsiteUri"])),
            ("Support", text(&local["SupportUri"])),
            ("Privacy policy", text(&legal["PrivacyPolicyUri"])),
            ("Terms of use", first_text(legal, &["TouUri", "Tou"])),
            ("Licence terms", text(&legal["AdditionalLicenseTerms"])),
        ]),
        copyright: text(&legal["Copyright"]),
    })
}

fn rows<const N: usize>(all: [Row; N]) -> Vec<Row> {
    all.into_iter().filter(|(_, v)| !v.is_empty()).collect()
}

/// A feature as people write it; one not known here by its words.
pub fn feature(name: &str) -> String {
    match name {
        "SinglePlayer" => "Single player".into(),
        "XblOnlineMultiPlayer" => "Online multiplayer".into(),
        "XblLocalMultiPlayer" => "Local multiplayer".into(),
        "XblCrossPlatformMultiPlayer" => "Cross-platform multiplayer".into(),
        "XboxLiveCrossGenMP" => "Cross-generation multiplayer".into(),
        "XblOnlineCoop" => "Online co-op".into(),
        "XblLocalCoop" => "Local co-op".into(),
        "XblCrossPlatformCoop" => "Cross-platform co-op".into(),
        "SharedSplitScreen" => "Shared or split screen".into(),
        "XblAchievements" => "Achievements".into(),
        "XblCloudSaves" => "Cloud saves".into(),
        "XblClubs" => "Clubs".into(),
        "XblPresence" => "Presence".into(),
        "XboxLive" => "Xbox network".into(),
        "XPA" => "Xbox Play Anywhere".into(),
        "Capability4k" => "4K Ultra HD".into(),
        "CapabilityHDR" => "HDR".into(),
        "RayTracing" => "Ray tracing".into(),
        "DolbyAtmos" => "Dolby Atmos".into(),
        "BroadcastSupport" => "Broadcasting".into(),
        "VREnabled" => "Virtual reality".into(),
        "MixedRealitySeated" => "Mixed reality, seated".into(),
        "ConsoleGen9Optimized" => "Optimized for Xbox Series X|S".into(),
        "ConsoleCrossGen" => "Cross-generation".into(),
        _ => sentence(name.trim_start_matches("Xbl")),
    }
}

/// Where a rating board's ratings apply; empty for one not known here.
pub fn rating_region(system: &str) -> &'static str {
    match system {
        "ESRB" => "United States and Canada",
        "PEGI" => "Europe",
        "USK" => "Germany",
        "CERO" => "Japan",
        "GRB" => "South Korea",
        "CSRR" => "Taiwan",
        "DJCTQ" => "Brazil",
        "COB-AU" => "Australia",
        "OFLC-NZ" => "New Zealand",
        "FPB" => "South Africa",
        "IARC" => "International",
        "PCBP" => "Russia",
        _ => "",
    }
}

/// A rating board's short word for its reason, such as `MilVio`, in full. A
/// part not known here stays as it is written.
fn descriptor(code: &str) -> String {
    match code {
        "UseInt" => return "Users interact".into(),
        "InGamPur" => return "In-game purchases".into(),
        "OnlIntAndCha" => return "Online interactivity and chat".into(),
        "UseOfAlcAndTob" => return "Use of alcohol and tobacco".into(),
        "UrgAct" => return "Urges to act".into(),
        _ => {}
    }
    let words: Vec<String> = spaced(code)
        .split(' ')
        .map(|part| {
            match part {
                "Vio" => "violence",
                "Mil" => "mild",
                "Fan" => "fantasy",
                "Str" => "strong",
                "Lan" => "language",
                "Ina" => "inappropriate",
                "Blo" => "blood",
                "Gor" => "gore",
                "Lyr" => "lyrics",
                "Sug" => "suggestive",
                "The" => "themes",
                "Sca" => "scary",
                "Sce" => "scenes",
                "Sim" => "simulated",
                "Gam" | "Gmb" => "gambling",
                "Fea" => "fear",
                "Hor" => "horror",
                "Cri" => "crime",
                "Dru" => "drugs",
                "Leg" => "legal",
                "Tob" => "tobacco",
                "Alc" => "alcohol",
                other => other,
            }
            .to_string()
        })
        .collect();
    capitalised(&words.join(" "))
}

/// What the Store's hardware codes are about, in the order
/// [`hardware_codes`] gives them.
const HARDWARE: [&str; 3] = ["Memory", "Video memory", "Input"];

/// The codes with a known meaning by kind, and the rest as written.
fn hardware_codes(value: &Value) -> ([Vec<String>; 3], Vec<String>) {
    let mut known: [Vec<String>; 3] = Default::default();
    let mut rest = Vec::new();
    for code in list(value) {
        let gigabytes = |prefix: char| {
            let number: u32 = code.strip_prefix(prefix)?.parse().ok()?;
            Some(format!("{number} GB"))
        };
        if let Some(memory) = gigabytes('m') {
            known[0].push(memory);
        } else if let Some(video) = gigabytes('v') {
            known[1].push(video);
        } else {
            match code.as_str() {
                "kbd" => known[2].push("Keyboard".into()),
                "mse" => known[2].push("Mouse".into()),
                "xgp" => known[2].push("Xbox controller".into()),
                _ => rest.push(code),
            }
        }
    }
    (known, rest)
}

fn consoles(value: &Value) -> String {
    let names: Vec<String> = list(value)
        .into_iter()
        .map(|name| match name.as_str() {
            "ConsoleGen8" => "Xbox One".into(),
            "ConsoleGen9" => "Xbox Series X|S".into(),
            _ => name,
        })
        .collect();
    names.join(", ")
}

/// The most pictures shown, the banner included.
const MAX_PICTURES: usize = 12;

/// One banner, then the PC screenshots, or the console's when there are none.
fn pictures(local: &Value) -> Vec<String> {
    let images = array(&local["Images"]);
    let of = |purpose: &str, position: &str| -> Vec<String> {
        images
            .iter()
            .filter(|i| text(&i["ImagePurpose"]) == purpose)
            .filter(|i| text(&i["ImagePositionInfo"]).starts_with(position))
            .map(|i| crate::catalog::absolute(&text(&i["Uri"])))
            .filter(|uri| uri.starts_with("https://"))
            .collect()
    };
    let mut all: Vec<String> = ["TitledHeroArt", "SuperHeroArt", "Hero"]
        .iter()
        .find_map(|purpose| of(purpose, "").into_iter().next())
        .into_iter()
        .collect();
    let mut shots = of("Screenshot", "Desktop");
    if shots.is_empty() {
        shots = of("Screenshot", "");
    }
    for shot in shots {
        if !all.contains(&shot) {
            all.push(shot);
        }
    }
    all.truncate(MAX_PICTURES);
    all
}

/// Fetch one picture into `dir`, unless it is there already.
pub fn picture(dir: &Path, base: &str) -> Result<PathBuf> {
    let url = crate::http::with_params(
        base,
        &[
            ("q", "90".into()),
            ("format", "jpg".into()),
            ("w", "1280".into()),
            ("h", "720".into()),
        ],
    );
    let dest = dir.join(format!("{:x}.jpg", Sha256::digest(url.as_bytes())));
    crate::http::download(&url, &dest)?;
    Ok(dest)
}

/// The web address in `value`, which may have words before it.
pub fn link(value: &str) -> Option<&str> {
    value
        .split_whitespace()
        .find(|word| word.starts_with("https://") || word.starts_with("http://"))
}

/// The game itself rather than its trial or pre-order, with packages if any has.
fn full_sku(product: &Value) -> &Value {
    let skus: Vec<&Value> = array(&product["DisplaySkuAvailabilities"])
        .iter()
        .map(|s| &s["Sku"])
        .collect();
    let packaged = |s: &&&Value| !array(&s["Properties"]["Packages"]).is_empty();
    let full = |s: &&&Value| s["Properties"]["IsTrial"].as_bool() != Some(true);
    skus.iter()
        .filter(packaged)
        .find(full)
        .or_else(|| skus.iter().find(packaged))
        .or_else(|| skus.first())
        .copied()
        .unwrap_or(&Value::Null)
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

fn first_text(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .map(|key| text(&value[*key]))
        .find(|t| !t.is_empty())
        .unwrap_or_default()
}

fn list(value: &Value) -> Vec<String> {
    array(value)
        .iter()
        .map(text)
        .filter(|t| !t.is_empty())
        .collect()
}

fn yes(value: &Value) -> String {
    match value.as_bool() {
        Some(true) => "Yes".into(),
        _ => String::new(),
    }
}

/// A date without its time; the catalog writes "none" as the year 9998 or 1.
fn date(value: &Value) -> String {
    let text = text(value);
    match text.split_once('T') {
        Some((day, _)) if !day.starts_with("9998") && !day.starts_with("0001") => day.into(),
        _ => String::new(),
    }
}

/// `ESRB:E10` as `E10`.
fn without_prefix(text: &str) -> String {
    text.split_once(':').map_or(text, |(_, rest)| rest).into()
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `SomeNewThing` as `Some new thing`; words in capitals stay.
fn sentence(name: &str) -> String {
    let words: Vec<String> = spaced(name)
        .split(' ')
        .enumerate()
        .map(|(i, word)| {
            let shouting = word.len() > 1 && word.chars().all(|c| !c.is_ascii_lowercase());
            if i == 0 || shouting {
                word.to_string()
            } else {
                word.to_ascii_lowercase()
            }
        })
        .collect();
    words.join(" ")
}

/// `SinglePlayer` as `Single Player`; a run of capitals stays together.
fn spaced(name: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() && previous_lower {
            out.push(' ');
        }
        previous_lower = c.is_ascii_lowercase();
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = r#"{"Products":[{
        "ProductId":"9ZZTESTGAME1","ProductType":"Game","LastModifiedDate":"2026-01-02T03:04:05Z",
        "LocalizedProperties":[{"DeveloperName":"Dev","PublisherName":"Pub","ProductDescription":"All about it."}],
        "MarketProperties":[{"OriginalReleaseDate":"9998-12-30T00:00:00Z",
            "UsageData":[{"AggregateTimeSpan":"AllTime","AverageRating":4.75,"RatingCount":12},
                         {"AggregateTimeSpan":"7Days","AverageRating":0,"RatingCount":0}],
            "ContentRatings":[{"RatingSystem":"ESRB","RatingId":"ESRB:E10","RatingDescriptors":["ESRB:FanVio"]}]}],
        "Properties":{"Categories":["Racing"],"Attributes":[{"Name":"SinglePlayer"},{"Name":"XPA"}]},
        "DisplaySkuAvailabilities":[
            {"Sku":{"Properties":{"IsTrial":true,"Packages":[{"PackageFormat":"Trial"}]}}},
            {"Sku":{"Properties":{"IsTrial":false,"XboxXPA":true,
                "HardwareProperties":{"MinimumGraphics":"RX 6500 XT","MinimumHardware":["dcc","m08","mT0"]},
                "Packages":[{"PackageFormat":"MSIXVC","MaxDownloadSizeInBytes":2500000000}]}}}]}]}"#;

    #[test]
    fn the_catalogs_answer_is_read_without_empty_lines() {
        let d = parse(PAYLOAD).unwrap();
        assert_eq!(d.description, "All about it.");
        assert_eq!(d.developer, "Dev");
        assert_eq!(d.dates.len(), 1, "no release date: {:?}", d.dates);
        assert_eq!(d.players.len(), 1, "a span nobody rated is left out");
        assert_eq!(d.features, ["SinglePlayer", "XPA"]);
        assert_eq!(
            d.requirements,
            [
                Requirement {
                    what: "Graphics",
                    minimum: "RX 6500 XT".into(),
                    recommended: String::new()
                },
                Requirement {
                    what: "Memory",
                    minimum: "8 GB".into(),
                    recommended: String::new()
                },
                Requirement {
                    what: "Other (Store codes)",
                    minimum: "dcc, mT0".into(),
                    recommended: String::new()
                },
            ]
        );
        assert_eq!(d.ages[0].rating, "E10");
        assert_eq!(d.ages[0].descriptors, ["Fantasy violence"]);
        let package = |label| d.package.iter().find(|(l, _)| *l == label).unwrap();
        assert_eq!(package("Format").1, "MSIXVC", "the game, not its trial");
        assert_eq!(package("Download size").1, "2.5 GB");
    }

    #[test]
    fn catalog_words_are_written_out() {
        assert_eq!(feature("XblAchievements"), "Achievements");
        assert_eq!(feature("XblSomeNewThing"), "Some new thing");
        assert_eq!(descriptor("StrVio"), "Strong violence");
        assert_eq!(descriptor("AntSoc"), "Ant Soc", "unknown words stay");
    }

    #[test]
    fn the_banner_comes_before_the_pc_screenshots_once_each() {
        let local: Value = serde_json::from_str(
            r#"{"Images":[
            {"ImagePurpose":"Screenshot","ImagePositionInfo":"Xbox/1","Uri":"//img/x"},
            {"ImagePurpose":"Screenshot","ImagePositionInfo":"Desktop/1","Uri":"//img/a"},
            {"ImagePurpose":"Screenshot","ImagePositionInfo":"Desktop/2","Uri":"//img/a"},
            {"ImagePurpose":"Screenshot","ImagePositionInfo":"Desktop/3","Uri":"file:///etc"},
            {"ImagePurpose":"TitledHeroArt","Uri":"//img/hero"}]}"#,
        )
        .unwrap();
        assert_eq!(pictures(&local), ["https://img/hero", "https://img/a"]);
    }

    #[test]
    fn only_web_addresses_are_links() {
        assert_eq!(link("Terms: http://a.b/c?d=1"), Some("http://a.b/c?d=1"));
        assert_eq!(link("https://a.b"), Some("https://a.b"));
        assert_eq!(link("file:///etc/passwd"), None);
        assert_eq!(link("© 2025 Someone"), None);
    }

    #[test]
    fn nothing_found_and_a_local_game_are_errors() {
        assert!(parse(r#"{"Products":[]}"#).is_err());
        assert!(parse("not json").is_err());
        let local = fetch("LOCAL-STEAM-1", "US", "en-us").unwrap_err();
        assert!(local.to_string().contains("not a Store product"));
    }
}

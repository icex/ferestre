//! What Microsoft's display catalog says about one game, for its details page.
//!
//! Asked for when the page opens and not cached: ratings change, and nothing
//! else in Ferestre needs it.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// One heading of the page and its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    pub description: String,
    /// Image addresses with no size on them: the banner, then screenshots.
    pub pictures: Vec<String>,
    pub groups: Vec<Group>,
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

    let mut groups = Vec::new();
    let mut about = Rows::default();
    about.add("Developer", text(&local["DeveloperName"]));
    about.add("Publisher", text(&local["PublisherName"]));
    about.add("Category", strings(&properties["Categories"]));
    about.add("Subcategory", text(&properties["Subcategory"]));
    about.add("Release date", date(&market["OriginalReleaseDate"]));
    about.add("Last update", date(&sku_properties["LastUpdateDate"]));
    about.add("Catalog entry changed", date(&product["LastModifiedDate"]));
    about.add(
        "Release notes",
        text(&sku["LocalizedProperties"][0]["ReleaseNotes"]),
    );
    about.into_group("About", &mut groups);

    let mut reviews = Rows::default();
    for usage in array(&market["UsageData"]) {
        let count = usage["RatingCount"].as_u64().unwrap_or(0);
        if let (Some(rating), true) = (usage["AverageRating"].as_f64(), count > 0) {
            let span = text(&usage["AggregateTimeSpan"]);
            let span = match span.as_str() {
                "7Days" => "Last 7 days".to_string(),
                "30Days" => "Last 30 days".to_string(),
                "AllTime" => "All time".to_string(),
                _ => span,
            };
            reviews.add(&span, format!("{rating:.1} of 5 ({count} ratings)"));
        }
    }
    reviews.into_group("Player ratings", &mut groups);

    let mut features = Rows::default();
    let names: Vec<String> = array(&properties["Attributes"])
        .iter()
        .map(|a| spaced(text(&a["Name"]).trim_start_matches("Xbl")))
        .filter(|n| !n.is_empty())
        .collect();
    features.add("Features", names.join(", "));
    features.add(
        "Optimized for consoles",
        strings(&properties["XboxConsoleGenOptimized"]),
    );
    features.add(
        "Plays on consoles",
        strings(&properties["XboxConsoleGenCompatible"]),
    );
    features.add("Xbox Play Anywhere", yes(&sku_properties["XboxXPA"]));
    features.add(
        "Add-ons",
        yes(&properties["ExtendedClientMetadata"]["HasAddOns"]),
    );
    features.add(
        "In-app purchases",
        yes(&sku_properties["HasThirdPartyIAPs"]),
    );
    features.into_group("Features", &mut groups);

    let hardware = &sku_properties["HardwareProperties"];
    let mut needs = Rows::default();
    for (label, key) in [
        ("Minimum processor", "MinimumProcessor"),
        ("Recommended processor", "RecommendedProcessor"),
        ("Minimum graphics", "MinimumGraphics"),
        ("Recommended graphics", "RecommendedGraphics"),
    ] {
        needs.add(label, text(&hardware[key]));
    }
    needs.add("Minimum hardware", strings(&hardware["MinimumHardware"]));
    needs.add(
        "Recommended hardware",
        strings(&hardware["RecommendedHardware"]),
    );
    let notes = &sku["LocalizedProperties"][0];
    needs.add("Minimum notes", text(&notes["MinimumNotes"]));
    needs.add("Recommended notes", text(&notes["RecommendedNotes"]));
    needs.into_group("System requirements", &mut groups);

    let mut ratings = Rows::default();
    for rating in array(&market["ContentRatings"]) {
        let mut what = without_prefix(&text(&rating["RatingId"]));
        let extra: Vec<String> = ["RatingDescriptors", "InteractiveElements"]
            .iter()
            .flat_map(|key| array(&rating[*key]))
            .map(|v| spaced(&without_prefix(&text(v))))
            .collect();
        if !extra.is_empty() {
            what = format!("{what} ({})", extra.join(", "));
        }
        ratings.add(&text(&rating["RatingSystem"]), what);
    }
    ratings.into_group("Age ratings", &mut groups);

    let mut technical = Rows::default();
    technical.add("Product id", text(&product["ProductId"]));
    technical.add("Type", text(&product["ProductType"]));
    technical.add("Package family", text(&properties["PackageFamilyName"]));
    technical.add("Package format", text(&package["PackageFormat"]));
    technical.add("Architectures", strings(&package["Architectures"]));
    if let Some(bytes) = package["MaxDownloadSizeInBytes"]
        .as_u64()
        .filter(|b| *b > 0)
    {
        technical.add("Download size", format!("{:.1} GB", bytes as f64 / 1e9));
    }
    technical.add("Content id", text(&package["ContentId"]));
    technical.add("Languages", strings(&package["Languages"]));
    technical.into_group("Package", &mut groups);

    let legal = &sku["LocalizedProperties"][0]["LegalText"];
    let mut links = Rows::default();
    links.add("Website", text(&local["PublisherWebsiteUri"]));
    links.add("Support", text(&local["SupportUri"]));
    links.add("Privacy policy", text(&legal["PrivacyPolicyUri"]));
    links.add("Terms of use", first_text(legal, &["TouUri", "Tou"]));
    links.add("Licence terms", text(&legal["AdditionalLicenseTerms"]));
    links.add("Copyright", text(&legal["Copyright"]));
    links.into_group("Links and legal", &mut groups);

    Ok(Details {
        description,
        pictures: pictures(local),
        groups,
    })
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

#[derive(Default)]
struct Rows(Vec<(String, String)>);

impl Rows {
    fn add(&mut self, label: &str, value: String) {
        if !value.is_empty() {
            self.0.push((label.to_string(), value));
        }
    }

    fn into_group(self, title: &'static str, groups: &mut Vec<Group>) {
        if !self.0.is_empty() {
            groups.push(Group {
                title,
                rows: self.0,
            });
        }
    }
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

fn strings(value: &Value) -> String {
    let all: Vec<String> = array(value)
        .iter()
        .map(text)
        .filter(|t| !t.is_empty())
        .collect();
    all.join(", ")
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
                "HardwareProperties":{"MinimumGraphics":"RX 6500 XT"},
                "Packages":[{"PackageFormat":"MSIXVC","MaxDownloadSizeInBytes":2500000000}]}}}]}]}"#;

    fn row<'a>(d: &'a Details, group: &str, label: &str) -> Option<&'a str> {
        d.groups
            .iter()
            .find(|g| g.title == group)?
            .rows
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn the_catalogs_answer_becomes_groups_without_empty_lines() {
        let d = parse(PAYLOAD).unwrap();
        assert_eq!(d.description, "All about it.");
        assert_eq!(row(&d, "About", "Developer"), Some("Dev"));
        assert_eq!(row(&d, "About", "Release date"), None, "no real date");
        assert_eq!(row(&d, "Player ratings", "Last 7 days"), None);
        assert_eq!(row(&d, "Features", "Features"), Some("Single Player, XPA"));
        assert_eq!(row(&d, "Features", "Xbox Play Anywhere"), Some("Yes"));
        assert_eq!(
            row(&d, "System requirements", "Minimum graphics"),
            Some("RX 6500 XT")
        );
        assert_eq!(row(&d, "Age ratings", "ESRB"), Some("E10 (Fan Vio)"));
        assert_eq!(
            row(&d, "Package", "Package format"),
            Some("MSIXVC"),
            "the game, not its trial"
        );
        assert_eq!(row(&d, "Package", "Download size"), Some("2.5 GB"));
        assert!(d.groups.iter().all(|g| !g.rows.is_empty()));
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

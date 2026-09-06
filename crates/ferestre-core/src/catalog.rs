//! Names and cover art, from the Store's own catalog.
//!
//! The collections service answers "what does this account own" with product
//! ids and nothing else -- no name, no image, no size. A library of a hundred
//! titles rendered from that reads as a list of twelve-character codes, which
//! is useless to a person. DisplayCatalog fills in the rest.
//!
//! It is **anonymous**: no token, no account, no relying party. That matters
//! for more than convenience. It means the launcher can show a title nobody
//! signed in for, cache the answer on disk, and ship that cache around --
//! without any of it touching an account. Nothing here needs the client.
//!
//! What comes back is Microsoft's own payload, so parsing is deliberately
//! tolerant: unknown fields are ignored and a product missing the parts we want
//! is skipped rather than failing the batch, because one odd row in a hundred
//! must not cost someone their whole library listing.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The anonymous product endpoint. `v7.0` is what the Store client itself uses.
pub const ENDPOINT: &str = "https://displaycatalog.mp.microsoft.com/v7.0/products";

/// Ids per request. The service accepts more, but a long URL is a fragile
/// thing to depend on and twenty is what the Store client sends.
pub const BATCH: usize = 20;

/// Fallbacks, not policy: a market and language only decide which name and art
/// come back, and every market has the id. Override with `FERESTRE_MARKET` and
/// `FERESTRE_LANGUAGE` when a title reads better in its own.
pub const DEFAULT_MARKET: &str = "US";
pub const DEFAULT_LANGUAGE: &str = "en-us";

/// What the catalog knows about one product that a launcher would show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Product {
    pub product_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// Absolute URL with no size on it; [`image_url`] adds that. Store images
    /// are resized server-side, so a 128px icon is a 17 KB download rather than
    /// a 1080px one we then throw most of away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// What an install will actually cost, counting only the packages that run
    /// on a desktop. Summing every platform's package instead reports a title
    /// as twice its size, because most carry an Xbox build too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_bytes: Option<u64>,
    /// When the sku last changed, as the service spells it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<String>,
    /// The content ids of this title's **Windows desktop** packages, and the
    /// update key: a rebuilt package gets a new content id, while `Version`
    /// comes back as `"0"` on the anonymous endpoint and is worth nothing.
    ///
    /// Desktop only, and that filter is the whole correctness of update
    /// detection. Most titles ship a `Windows.Xbox` package beside the desktop
    /// one, with a different content id; comparing an install against the union
    /// finds an id it does not have and reports an update that does not exist.
    /// Measured against three installed titles, the union is wrong on two.
    ///
    /// Empty when the catalog lists no desktop package. That is a real answer,
    /// not a failure -- some titles are Xbox-only -- and it must stay empty
    /// rather than falling back to every platform, because
    /// [`crate::install::Record::update_available`] reads empty as "cannot
    /// tell", which is the truth.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_ids: Vec<String>,
    /// Whether the catalog listed any package at all. Lets a caller tell "no
    /// desktop build exists" from "no packages listed", which read the same
    /// from `content_ids` alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_packages: bool,
    /// The container the PC package ships in -- `MSIXVC` for a GDK title,
    /// `Appx`/`AppxBundle` for a UWP one. Only an MSIXVC is something this
    /// launcher can install and run today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_format: Option<String>,
}

impl Product {
    /// Whether this launcher could install it: a PC package, in the container
    /// its runtime knows how to open.
    pub fn is_runnable_here(&self) -> Option<bool> {
        let format = self.package_format.as_deref()?;
        Some(format.eq_ignore_ascii_case(RUNNABLE_FORMAT))
    }

    /// A size to show, or an empty string. Powers of ten, like a store page:
    /// nobody comparing "47 GB" against their free space means gibibytes.
    pub fn size_label(&self) -> String {
        match self.download_bytes {
            None | Some(0) => String::new(),
            Some(b) if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
            Some(b) => format!("{} MB", b / 1_000_000),
        }
    }
}

/// Build the query for one batch of ids.
pub fn request_url(ids: &[String], market: &str, language: &str) -> String {
    format!(
        "{ENDPOINT}?bigIds={}&market={}&languages={}",
        ids.join(","),
        market,
        language
    )
}

/// Split ids into requests, keeping the caller's order.
pub fn batches(ids: &[String]) -> impl Iterator<Item = &[String]> {
    ids.chunks(BATCH)
}

/// A store image URL at a given pixel size, resized by the CDN.
pub fn image_url(base: &str, px: u32) -> String {
    crate::http::with_params(
        base,
        &[
            ("q", "90".into()),
            ("format", "png".into()),
            ("w", px.to_string()),
            ("h", px.to_string()),
        ],
    )
}

// The wire types. Only the fields a launcher acts on, and every one optional:
// this is somebody else's payload and it gains and loses shape without notice.

#[derive(Deserialize)]
struct Response {
    #[serde(rename = "Products", default)]
    products: Vec<RawProduct>,
}

#[derive(Deserialize)]
struct RawProduct {
    #[serde(rename = "ProductId", default)]
    product_id: Option<String>,
    #[serde(rename = "LocalizedProperties", default)]
    localized: Vec<Localized>,
    #[serde(rename = "DisplaySkuAvailabilities", default)]
    skus: Vec<SkuAvailability>,
}

#[derive(Deserialize)]
struct Localized {
    #[serde(rename = "ProductTitle", default)]
    title: Option<String>,
    #[serde(rename = "PublisherName", default)]
    publisher: Option<String>,
    #[serde(rename = "Images", default)]
    images: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    #[serde(rename = "ImagePurpose", default)]
    purpose: Option<String>,
    #[serde(rename = "Uri", default)]
    uri: Option<String>,
    #[serde(rename = "Width", default)]
    width: Option<u32>,
    #[serde(rename = "Height", default)]
    height: Option<u32>,
}

#[derive(Deserialize)]
struct SkuAvailability {
    #[serde(rename = "Sku", default)]
    sku: Option<Sku>,
}

#[derive(Deserialize)]
struct Sku {
    #[serde(rename = "Properties", default)]
    properties: Option<SkuProperties>,
}

#[derive(Deserialize)]
struct SkuProperties {
    #[serde(rename = "LastUpdateDate", default)]
    last_update: Option<String>,
    #[serde(rename = "Packages", default)]
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    #[serde(rename = "ContentId", default)]
    content_id: Option<String>,
    #[serde(rename = "MaxDownloadSizeInBytes", default)]
    download_bytes: Option<u64>,
    #[serde(rename = "PackageFormat", default)]
    format: Option<String>,
    #[serde(rename = "PlatformDependencies", default)]
    platforms: Vec<PlatformDependency>,
}

#[derive(Deserialize)]
struct PlatformDependency {
    #[serde(rename = "PlatformName", default)]
    name: Option<String>,
}

/// The platforms that mean "runs on a PC".
///
/// Both, and the second one matters: a UWP app declares `Windows.Universal` and
/// nothing else -- Candy Crush Saga has a Universal package, an Xbox one and two
/// phone ones, and no `Windows.Desktop` at all. Filtering on Desktop alone drops
/// such titles to zero packages, which reads as "not available" for something
/// the account owns and Windows runs perfectly well.
const DESKTOP: [&str; 2] = ["Windows.Desktop", "Windows.Universal"];

/// The package format this launcher's runtime can actually load.
///
/// The whole decrypt-and-launch path is built for MSIXVC. `Appx`, `AppxBundle`
/// and the `E`-prefixed Xbox variants are different containers, so a title whose
/// only PC package is one of those is not something this can run yet -- and
/// saying which format it is beats failing later with the client's own error.
pub const RUNNABLE_FORMAT: &str = "MSIXVC";

impl Package {
    fn is_desktop(&self) -> bool {
        self.platforms
            .iter()
            .any(|p| p.name.as_deref().is_some_and(|n| DESKTOP.contains(&n)))
    }
}

/// Purposes worth putting in a list row, best first. Square art comes first
/// because the row shows a square: a 720x1080 poster letterboxed into 48x48 is
/// worse than a smaller logo that fits.
const PURPOSE_ORDER: [&str; 7] = [
    "Logo",
    "BoxArt",
    "FeaturePromotionalSquareArt",
    "Poster",
    "BrandedKeyArt",
    "TitledHeroArt",
    "Hero",
];

/// Pick the art for a row: preferred purpose first, then the squarest, then the
/// largest. Not every title has every purpose -- one of the two titles this was
/// written against has no `Logo` at all -- so the fallback chain has to be real.
fn pick_image(images: &[Image]) -> Option<String> {
    images
        .iter()
        .filter(|i| i.uri.as_deref().is_some_and(|u| !u.is_empty()))
        .filter_map(|i| {
            let purpose = i.purpose.as_deref()?;
            let rank = PURPOSE_ORDER.iter().position(|p| *p == purpose)?;
            // Squareness in tenths, so it orders as an integer: 0 is square.
            let skew = match (i.width, i.height) {
                (Some(w), Some(h)) if w > 0 && h > 0 => {
                    ((w.max(h) as f64 / w.min(h) as f64 - 1.0) * 10.0) as u32
                }
                _ => u32::MAX,
            };
            let area = i.width.unwrap_or(0) as u64 * i.height.unwrap_or(0) as u64;
            Some((rank, skew, std::cmp::Reverse(area), i.uri.clone().unwrap()))
        })
        .min()
        .map(|(_, _, _, uri)| absolute(&uri))
}

/// Catalog image URIs come back protocol-relative (`//store-images...`).
fn absolute(uri: &str) -> String {
    match uri.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => uri.to_string(),
    }
}

/// Parse a DisplayCatalog response.
///
/// A product with no id or no name is dropped rather than failing the batch:
/// the caller asked about a hundred titles and one unusable row is not a reason
/// to show them none.
pub fn parse(json: &str) -> Result<Vec<Product>> {
    let response: Response =
        serde_json::from_str(json).map_err(|e| anyhow!("not a catalog response: {e}"))?;

    Ok(response
        .products
        .into_iter()
        .filter_map(|raw| {
            let product_id = raw.product_id.filter(|s| !s.is_empty())?;
            let localized = raw.localized.first();
            let name = localized
                .and_then(|l| l.title.clone())
                .filter(|s| !s.is_empty())?;

            let mut content_ids = Vec::new();
            let mut download_bytes = None;
            let mut last_update = None;
            let mut has_packages = false;
            let mut package_format: Option<String> = None;
            for sku in &raw.skus {
                let Some(properties) = sku.sku.as_ref().and_then(|s| s.properties.as_ref()) else {
                    continue;
                };
                last_update = last_update
                    .take()
                    .or_else(|| properties.last_update.clone());
                for package in &properties.packages {
                    has_packages = true;
                    // Xbox packages are listed beside the desktop one and are a
                    // different build with a different content id. Counting them
                    // is what makes an up-to-date title report an update.
                    if !package.is_desktop() {
                        continue;
                    }
                    // Every sku of a title lists the same packages -- a trial
                    // and the full sku point at one build -- so the same content
                    // id turns up repeatedly and only the distinct set means
                    // anything as an update key.
                    package_format = package_format
                        .take()
                        .or_else(|| package.format.clone().filter(|f| !f.is_empty()));
                    if let Some(id) = package.content_id.clone().filter(|s| !s.is_empty()) {
                        if !content_ids.contains(&id) {
                            content_ids.push(id);
                            download_bytes = Some(
                                download_bytes.unwrap_or(0) + package.download_bytes.unwrap_or(0),
                            );
                        }
                    }
                }
            }

            Some(Product {
                product_id,
                name,
                publisher: localized
                    .and_then(|l| l.publisher.clone())
                    .filter(|s| !s.is_empty()),
                image: localized.and_then(|l| pick_image(&l.images)),
                download_bytes: download_bytes.filter(|b| *b > 0),
                last_update,
                content_ids,
                has_packages,
                package_format,
            })
        })
        .collect())
}

/// What the cached form of a product means.
///
/// Bumped whenever the meaning of a cached field changes, not just its shape.
/// Version 2 is where `content_ids` became desktop-only: a version-1 file
/// parses perfectly and is *wrong*, because it holds the union across platforms,
/// which update detection reads as a permanent pending update. A cache that
/// survives a change of meaning is worse than no cache.
const CACHE_SCHEMA: u32 = 2;

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    schema: u32,
    product: Product,
}

/// A catalog answer kept on disk.
///
/// Names and art change about as often as a game is re-released, so this is a
/// cache with no expiry and an explicit refresh rather than a TTL that makes
/// someone wait for the network on a cold morning. A miss is not an error: the
/// window shows the product id until the fetch lands.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    /// `dir` is created lazily, on the first write.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Cache { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entry_path(&self, product_id: &str) -> Option<PathBuf> {
        // A product id is the key and it ends up in a filename, so anything
        // that is not one is refused rather than escaped: this is data from a
        // network service, and `../../..` in a cache key is how that becomes a
        // write outside the cache.
        if product_id.is_empty()
            || !product_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return None;
        }
        Some(
            self.dir
                .join(format!("{}.json", product_id.to_ascii_uppercase())),
        )
    }

    pub fn get(&self, product_id: &str) -> Option<Product> {
        let path = self.entry_path(product_id)?;
        let text = std::fs::read_to_string(path).ok()?;
        let entry: CacheEntry = serde_json::from_str(&text).ok()?;
        // An entry written by a different build is discarded rather than
        // trusted; the caller treats it as a miss and fetches it again.
        (entry.schema == CACHE_SCHEMA).then_some(entry.product)
    }

    pub fn put(&self, product: &Product) -> Result<()> {
        let path = self
            .entry_path(&product.product_id)
            .ok_or_else(|| anyhow!("not a usable product id: {:?}", product.product_id))?;
        std::fs::create_dir_all(&self.dir)?;
        let entry = CacheEntry {
            schema: CACHE_SCHEMA,
            product: product.clone(),
        };
        std::fs::write(path, serde_json::to_vec_pretty(&entry)?)?;
        Ok(())
    }

    /// Split ids into what the cache already knows and what has to be fetched,
    /// keeping the caller's order in both.
    pub fn split(&self, ids: &[String]) -> (Vec<Product>, Vec<String>) {
        let mut hits = Vec::new();
        let mut misses = Vec::new();
        for id in ids {
            match self.get(id) {
                Some(product) => hits.push(product),
                None => misses.push(id.clone()),
            }
        }
        (hits, misses)
    }

    /// Where a title's icon is kept once fetched.
    pub fn image_path(&self, product_id: &str, px: u32) -> Option<PathBuf> {
        let base = self.entry_path(product_id)?;
        let stem = base.file_stem()?.to_string_lossy().to_string();
        Some(self.dir.join("images").join(format!("{stem}-{px}.png")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a real response, cut down to what is read. The two products
    /// differ on purpose: one has a square `Logo`, the other has none, which is
    /// exactly the case that broke a naive picker.
    const PAYLOAD: &str = r#"{
      "Products": [
        {
          "ProductId": "9ZZTESTGAME1",
          "LastModifiedDate": "2026-08-28T18:26:01.6677099Z",
          "LocalizedProperties": [{
            "ProductTitle": "Test Game One",
            "PublisherName": "Test Studios",
            "DeveloperName": "Someone Else",
            "Images": [
              {"ImagePurpose": "Screenshot", "Uri": "//img/shot", "Width": 3840, "Height": 2160},
              {"ImagePurpose": "Poster", "Uri": "//img/poster", "Width": 720, "Height": 1080},
              {"ImagePurpose": "Logo", "Uri": "//img/logo", "Width": 300, "Height": 300}
            ]
          }],
          "DisplaySkuAvailabilities": [
            {"Sku": {"SkuId": "0010", "Properties": {
              "LastUpdateDate": "2026-08-28T18:26:01.0000000Z",
              "Packages": [{"ContentId": "content-a", "MaxDownloadSizeInBytes": 2490064896,
                          "PlatformDependencies": [{"PlatformName": "Windows.Desktop"}]}]
            }}},
            {"Sku": {"SkuId": "0011", "Properties": {
              "LastUpdateDate": "2026-08-28T18:26:01.0000000Z",
              "Packages": [{"ContentId": "content-a", "MaxDownloadSizeInBytes": 2490064896,
                          "PlatformDependencies": [{"PlatformName": "Windows.Desktop"}]}]
            }}}
          ]
        },
        {
          "ProductId": "9ZZTESTGAME2",
          "LocalizedProperties": [{
            "ProductTitle": "Test Game Two",
            "Images": [
              {"ImagePurpose": "BrandedKeyArt", "Uri": "//img/key", "Width": 584, "Height": 800},
              {"ImagePurpose": "Poster", "Uri": "//img/poster2", "Width": 1440, "Height": 2160},
              {"ImagePurpose": "BoxArt", "Uri": "//img/box", "Width": 2160, "Height": 2160}
            ]
          }],
          "DisplaySkuAvailabilities": [
            {"Sku": {"Properties": {"Packages": [
              {"ContentId": "content-b", "MaxDownloadSizeInBytes": 47272595456,
               "PackageFormat": "MSIXVC",
               "PlatformDependencies": [{"PlatformName": "Windows.Desktop"}]},
              {"ContentId": "content-c", "MaxDownloadSizeInBytes": 49074888704,
               "PackageFormat": "MSIXVC",
               "PlatformDependencies": [{"PlatformName": "Windows.Xbox"}]}
            ]}}}
          ]
        },
        {
          "ProductId": "9ZZTESTJUNK3",
          "LocalizedProperties": [{"ProductTitle": ""}]
        }
      ]
    }"#;

    #[test]
    fn a_response_becomes_products() {
        let products = parse(PAYLOAD).expect("parses");
        assert_eq!(
            products.len(),
            2,
            "the nameless product is dropped, not fatal"
        );

        let one = &products[0];
        assert_eq!(one.product_id, "9ZZTESTGAME1");
        assert_eq!(one.name, "Test Game One");
        assert_eq!(one.publisher.as_deref(), Some("Test Studios"));
        assert_eq!(
            one.last_update.as_deref(),
            Some("2026-08-28T18:26:01.0000000Z")
        );
    }

    /// Every sku of a title lists the same package. Counting it once per sku
    /// would treble the download size shown on a three-sku title.
    #[test]
    fn a_package_listed_under_several_skus_counts_once() {
        let products = parse(PAYLOAD).expect("parses");
        assert_eq!(products[0].content_ids, vec!["content-a".to_string()]);
        assert_eq!(products[0].download_bytes, Some(2490064896));
        assert_eq!(products[0].size_label(), "2.5 GB");
    }

    /// The case that was wrong, and the reason update detection could not work.
    ///
    /// Most titles list an Xbox package beside the desktop one, with its own
    /// content id and its own size. Counting both reports a title as twice its
    /// size and, worse, gives update detection an id the install can never have
    /// -- a permanent "update available" that no download clears. Measured
    /// against the three titles installed on the development machine, taking
    /// the union is wrong on two of them.
    #[test]
    fn an_xbox_package_beside_the_desktop_one_is_not_counted() {
        let products = parse(PAYLOAD).expect("parses");
        assert_eq!(
            products[1].content_ids,
            vec!["content-b"],
            "the Windows.Xbox package is a different build and is not what gets installed"
        );
        assert_eq!(products[1].download_bytes, Some(47272595456));
        assert_eq!(products[1].size_label(), "47.3 GB");
        assert!(products[1].has_packages);
    }

    /// A UWP app declares `Windows.Universal` and nothing else. Filtering on
    /// `Windows.Desktop` alone dropped Candy Crush Saga to zero packages --
    /// "not available" for something the account owns and Windows runs.
    #[test]
    fn a_universal_package_counts_as_a_pc_package() {
        let uwp = r#"{"Products":[{
          "ProductId": "9ZZTESTUWP01",
          "LocalizedProperties": [{"ProductTitle": "A UWP App"}],
          "DisplaySkuAvailabilities": [{"Sku": {"Properties": {"Packages": [
            {"ContentId": "content-u", "MaxDownloadSizeInBytes": 177700000,
             "PackageFormat": "Appx",
             "PlatformDependencies": [{"PlatformName": "Windows.Universal"}]},
            {"ContentId": "content-x", "MaxDownloadSizeInBytes": 209200000,
             "PackageFormat": "EAppx",
             "PlatformDependencies": [{"PlatformName": "Windows.Xbox"}]},
            {"ContentId": "content-p", "MaxDownloadSizeInBytes": 56300000,
             "PackageFormat": "Xap",
             "PlatformDependencies": [{"PlatformName": "Windows.WindowsPhone8x"}]}
          ]}}}]
        }]}"#;
        let products = parse(uwp).expect("parses");
        assert_eq!(
            products[0].content_ids,
            vec!["content-u"],
            "the PC package, and only it"
        );
        assert_eq!(products[0].size_label(), "177 MB");
    }

    /// Being installable and being runnable here are different questions. A UWP
    /// Appx is a PC package this launcher cannot open: the whole
    /// decrypt-and-launch path is built for MSIXVC.
    #[test]
    fn the_package_format_says_whether_this_launcher_could_run_it() {
        let products = parse(PAYLOAD).expect("parses");
        assert_eq!(products[1].package_format.as_deref(), Some("MSIXVC"));
        assert_eq!(products[1].is_runnable_here(), Some(true));

        let uwp = r#"{"Products":[{
          "ProductId": "9ZZTESTUWP01",
          "LocalizedProperties": [{"ProductTitle": "A UWP App"}],
          "DisplaySkuAvailabilities": [{"Sku": {"Properties": {"Packages": [
            {"ContentId": "content-u", "PackageFormat": "Appx",
             "PlatformDependencies": [{"PlatformName": "Windows.Universal"}]}
          ]}}}]
        }]}"#;
        let products = parse(uwp).expect("parses");
        assert_eq!(products[0].package_format.as_deref(), Some("Appx"));
        assert_eq!(products[0].is_runnable_here(), Some(false));

        // Nothing listed is not the same as "no".
        let bare = parse(
            r#"{"Products":[{"ProductId":"9ZZTESTBARE1",
          "LocalizedProperties":[{"ProductTitle":"Bare"}]}]}"#,
        )
        .expect("parses");
        assert_eq!(bare[0].is_runnable_here(), None);
    }

    /// Some titles are Xbox-only. The honest answer is an empty set, which
    /// update detection reads as "cannot tell" -- never a fallback to every
    /// platform, which would resurrect the phantom update on every one of them.
    #[test]
    fn a_title_with_no_desktop_package_yields_nothing_rather_than_falling_back() {
        let xbox_only = r#"{"Products":[{
          "ProductId": "9ZZTESTXBOX1",
          "LocalizedProperties": [{"ProductTitle": "Console Only"}],
          "DisplaySkuAvailabilities": [{"Sku": {"Properties": {"Packages": [
            {"ContentId": "content-x", "MaxDownloadSizeInBytes": 78400000000,
             "PlatformDependencies": [{"PlatformName": "Windows.Xbox"}]}
          ]}}}]
        }]}"#;
        let products = parse(xbox_only).expect("parses");
        assert!(products[0].content_ids.is_empty());
        assert_eq!(
            products[0].download_bytes, None,
            "no desktop package, no size"
        );
        assert!(
            products[0].has_packages,
            "packages exist, just none for this platform -- a caller can tell the two apart"
        );
    }

    /// A package that names no platform at all is not assumed to be desktop.
    #[test]
    fn a_package_with_no_platform_is_not_guessed_at() {
        let vague = r#"{"Products":[{
          "ProductId": "9ZZTESTVAGU1",
          "LocalizedProperties": [{"ProductTitle": "Unspecified"}],
          "DisplaySkuAvailabilities": [{"Sku": {"Properties": {"Packages": [
            {"ContentId": "content-v", "MaxDownloadSizeInBytes": 100}
          ]}}}]
        }]}"#;
        let products = parse(vague).expect("parses");
        assert!(products[0].content_ids.is_empty());
        assert!(products[0].has_packages);
    }

    #[test]
    fn a_square_logo_wins_and_a_missing_one_falls_back_to_box_art() {
        let products = parse(PAYLOAD).expect("parses");
        assert_eq!(products[0].image.as_deref(), Some("https://img/logo"));
        assert_eq!(
            products[1].image.as_deref(),
            Some("https://img/box"),
            "no Logo, so the square BoxArt beats the taller poster and key art"
        );
    }

    #[test]
    fn an_empty_response_is_not_an_error_but_junk_is() {
        assert!(parse(r#"{"Products":[]}"#).expect("parses").is_empty());
        assert!(parse("<html>503</html>").is_err());
    }

    #[test]
    fn requests_are_batched_and_shaped() {
        let ids: Vec<String> = (0..45).map(|i| format!("9ZZTEST{i:05}")).collect();
        let batched: Vec<usize> = batches(&ids).map(<[String]>::len).collect();
        assert_eq!(batched, vec![20, 20, 5]);

        let url = request_url(&ids[..2], "GB", "en-gb");
        assert!(url.starts_with(ENDPOINT), "{url}");
        assert!(url.contains("bigIds=9ZZTEST00000,9ZZTEST00001"), "{url}");
        assert!(
            url.contains("market=GB") && url.contains("languages=en-gb"),
            "{url}"
        );
    }

    #[test]
    fn image_urls_ask_the_cdn_for_the_size_we_draw() {
        assert_eq!(
            image_url("https://img/logo", 128),
            "https://img/logo?q=90&format=png&w=128&h=128"
        );
    }

    #[test]
    fn size_labels_use_the_units_a_store_page_uses() {
        let mut p = parse(PAYLOAD).expect("parses").remove(0);
        p.download_bytes = Some(0);
        assert_eq!(p.size_label(), "");
        p.download_bytes = None;
        assert_eq!(p.size_label(), "");
        p.download_bytes = Some(734_003_200);
        assert_eq!(p.size_label(), "734 MB");
    }

    /// The cache key comes off the network and ends up in a filename.
    #[test]
    fn a_product_id_that_is_not_one_never_becomes_a_path() {
        let cache = Cache::new("/nonexistent/cache");
        assert!(cache.entry_path("../../etc/passwd").is_none());
        assert!(cache.entry_path("9ZZ/TEST").is_none());
        assert!(cache.entry_path("").is_none());
        assert_eq!(
            cache.entry_path("9zztestgame1"),
            Some(PathBuf::from("/nonexistent/cache/9ZZTESTGAME1.json")),
            "ids are cased inconsistently between services; the key is not"
        );
        assert!(cache.image_path("../evil", 128).is_none());
    }

    /// The bug this exists to stop: `content_ids` changed meaning from "every
    /// platform" to "desktop only". A file written before that change parses
    /// cleanly and is wrong, and being wrong here is a permanent phantom update
    /// on most titles.
    #[test]
    fn a_cache_entry_from_another_schema_is_a_miss_not_a_wrong_answer() {
        let dir =
            std::env::temp_dir().join(format!("ferestre-cache-schema-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");
        let cache = Cache::new(&dir);

        // Exactly what the previous build wrote: a bare product, no schema.
        std::fs::write(
            dir.join("9ZZTESTGAME1.json"),
            r#"{"product_id":"9ZZTESTGAME1","name":"Old","content_ids":["a","b"]}"#,
        )
        .expect("writes");
        assert_eq!(cache.get("9ZZTESTGAME1"), None, "no schema, so not trusted");

        // And a future one, which this build equally cannot interpret.
        std::fs::write(
            dir.join("9ZZTESTGAME2.json"),
            r#"{"schema":99,"product":{"product_id":"9ZZTESTGAME2","name":"Future"}}"#,
        )
        .expect("writes");
        assert_eq!(cache.get("9ZZTESTGAME2"), None);

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }

    #[test]
    fn a_cache_round_trip_keeps_what_the_window_draws() {
        let dir =
            std::env::temp_dir().join(format!("ferestre-catalog-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::new(&dir);

        let products = parse(PAYLOAD).expect("parses");
        cache.put(&products[0]).expect("writes");

        let ids = vec!["9ZZTESTGAME1".to_string(), "9ZZTESTGAME2".to_string()];
        let (hits, misses) = cache.split(&ids);
        assert_eq!(hits, vec![products[0].clone()]);
        assert_eq!(misses, vec!["9ZZTESTGAME2".to_string()]);

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }
}

/// Fetching. Kept apart from everything above so the parsing, the batching, the
/// image choice and the cache are all testable without a network.
mod net {
    use super::*;

    /// Look up every id, a batch at a time.
    ///
    /// A batch that fails is reported and the rest continue: with a hundred
    /// titles, one bad response should cost five rows, not the library.
    pub fn fetch(ids: &[String], market: &str, language: &str) -> (Vec<Product>, Vec<String>) {
        let mut products = Vec::new();
        let mut failures = Vec::new();
        for batch in batches(ids) {
            let url = request_url(batch, market, language);
            match crate::http::get_text(&url).and_then(|body| parse(&body)) {
                Ok(mut got) => products.append(&mut got),
                Err(e) => failures.push(e.to_string()),
            }
        }
        (products, failures)
    }
}

/// Look up names and art for these ids, consulting the cache first and writing
/// back what it had to fetch.
///
/// Returns the products it could account for and, separately, the errors --
/// because a launcher showing ninety-five of a hundred titles should say so,
/// not pretend the other five do not exist.
pub fn resolve(
    cache: &Cache,
    ids: &[String],
    market: &str,
    language: &str,
) -> (Vec<Product>, Vec<String>) {
    let (mut products, misses) = cache.split(ids);
    if misses.is_empty() {
        return (products, Vec::new());
    }
    let (fetched, failures) = net::fetch(&misses, market, language);
    for product in &fetched {
        // A cache that cannot be written is not worth failing a lookup over.
        let _ = cache.put(product);
    }
    products.extend(fetched);
    (products, failures)
}

/// Make sure a title's icon is on disk at this size, and say where.
pub fn icon(cache: &Cache, product: &Product, px: u32) -> Result<PathBuf> {
    let dest = cache
        .image_path(&product.product_id, px)
        .ok_or_else(|| anyhow!("not a usable product id: {:?}", product.product_id))?;
    let base = product
        .image
        .as_deref()
        .ok_or_else(|| anyhow!("{} has no art in the catalog", product.product_id))?;
    crate::http::download(&image_url(base, px), &dest)?;
    Ok(dest)
}

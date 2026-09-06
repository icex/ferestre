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
/// come back, and every market has the id. Override with `XGDK_MARKET` and
/// `XGDK_LANGUAGE` when a title reads better in its own.
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
    /// Largest package download, in bytes. What an install will actually cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_bytes: Option<u64>,
    /// When the sku last changed, as the service spells it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<String>,
    /// One per package. **This is the update key**: a rebuilt package gets a
    /// new content id, while `Version` comes back as `"0"` on the anonymous
    /// endpoint and is worth nothing. Record it at install time and an update
    /// is "the catalog now lists a content id we do not have".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_ids: Vec<String>,
}

impl Product {
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
    format!("{base}?q=90&w={px}&h={px}&format=png")
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
            for sku in &raw.skus {
                let Some(properties) = sku.sku.as_ref().and_then(|s| s.properties.as_ref()) else {
                    continue;
                };
                last_update = last_update.take().or_else(|| properties.last_update.clone());
                for package in &properties.packages {
                    // Every sku of a title lists the same packages -- a trial
                    // and the full sku point at one build -- so the same content
                    // id turns up repeatedly and only the distinct set means
                    // anything as an update key.
                    if let Some(id) = package.content_id.clone().filter(|s| !s.is_empty()) {
                        if !content_ids.contains(&id) {
                            content_ids.push(id);
                            download_bytes =
                                Some(download_bytes.unwrap_or(0) + package.download_bytes.unwrap_or(0));
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
            })
        })
        .collect())
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
        Some(self.dir.join(format!("{}.json", product_id.to_ascii_uppercase())))
    }

    pub fn get(&self, product_id: &str) -> Option<Product> {
        let path = self.entry_path(product_id)?;
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn put(&self, product: &Product) -> Result<()> {
        let path = self
            .entry_path(&product.product_id)
            .ok_or_else(|| anyhow!("not a usable product id: {:?}", product.product_id))?;
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(path, serde_json::to_vec_pretty(product)?)?;
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
              "Packages": [{"ContentId": "content-a", "MaxDownloadSizeInBytes": 2490064896}]
            }}},
            {"Sku": {"SkuId": "0011", "Properties": {
              "LastUpdateDate": "2026-08-28T18:26:01.0000000Z",
              "Packages": [{"ContentId": "content-a", "MaxDownloadSizeInBytes": 2490064896}]
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
              {"ContentId": "content-b", "MaxDownloadSizeInBytes": 47272595456},
              {"ContentId": "content-c", "MaxDownloadSizeInBytes": 49074888704}
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
        assert_eq!(products.len(), 2, "the nameless product is dropped, not fatal");

        let one = &products[0];
        assert_eq!(one.product_id, "9ZZTESTGAME1");
        assert_eq!(one.name, "Test Game One");
        assert_eq!(one.publisher.as_deref(), Some("Test Studios"));
        assert_eq!(one.last_update.as_deref(), Some("2026-08-28T18:26:01.0000000Z"));
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

    /// A title split across packages costs the sum to install.
    #[test]
    fn several_packages_add_up() {
        let products = parse(PAYLOAD).expect("parses");
        assert_eq!(products[1].content_ids, vec!["content-b", "content-c"]);
        assert_eq!(products[1].download_bytes, Some(47272595456 + 49074888704));
        assert_eq!(products[1].size_label(), "96.3 GB");
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
        assert!(url.contains("market=GB") && url.contains("languages=en-gb"), "{url}");
    }

    #[test]
    fn image_urls_ask_the_cdn_for_the_size_we_draw() {
        assert_eq!(
            image_url("https://img/logo", 128),
            "https://img/logo?q=90&w=128&h=128&format=png"
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

    #[test]
    fn a_cache_round_trip_keeps_what_the_window_draws() {
        let dir = std::env::temp_dir().join(format!("xgdk-catalog-test-{}", std::process::id()));
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

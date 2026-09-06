//! The parser against real catalog records, not hand-written ones.
//!
//! Every other catalog test states a shape and checks the parser reads it. This
//! one goes the other way: it holds fifteen records as `displaycatalog.mp
//! .microsoft.com` actually returned them, trimmed to the fields the parser
//! looks at, and asserts that none of them comes out without an answer.
//!
//! It exists because thirteen of a hundred and one products in a real library
//! had no download size, and every one of them was a different reason -- a
//! Windows 8 store app, a title that only ships for the console, a bundle that
//! carries no packages of its own. Hand-written fixtures had all three of those
//! wrong at once, and agreed with the code while doing it.
//!
//! Refresh it with:
//!
//! ```text
//! curl "https://displaycatalog.mp.microsoft.com/v7.0/products?bigIds=<ids>&market=US&languages=en-us"
//! ```

use ferestre_core::catalog::parse;

const SHAPES: &str = include_str!("fixtures/catalog-shapes.json");

/// The whole point, in one assertion: a product never comes out size-less
/// *and* silent about why.
///
/// Three answers are acceptable, and a row can be written for each of them --
/// here is the download, there is no PC build, or this is a bundle and the size
/// belongs to a child. A fourth state, "no size and no reason", is what the
/// launcher used to show for thirteen titles.
#[test]
fn every_real_record_has_a_size_or_a_reason() {
    let products = parse(SHAPES).expect("the captured records parse");
    assert_eq!(products.len(), 15, "all fifteen survive parsing");

    let mut unexplained = Vec::new();
    for product in &products {
        let explained = product.download_bytes.is_some()
            || !product.has_pc_package
            || !product.bundled_ids.is_empty();
        if !explained {
            unexplained.push(format!("{} ({})", product.name, product.product_id));
        }
    }
    assert!(
        unexplained.is_empty(),
        "these have no size and nothing to say about why: {unexplained:?}"
    );
}

/// The four categories, named, so a regression says which one broke rather than
/// only that the count changed.
#[test]
fn the_real_records_land_in_the_categories_they_should() {
    let products = parse(SHAPES).expect("the captured records parse");
    let by_id = |id: &str| {
        products
            .iter()
            .find(|p| p.product_id == id)
            .unwrap_or_else(|| panic!("{id} is in the fixture"))
    };

    // A Windows 8 store app: one PC package, in the oldest platform we count.
    let windows8x = by_id("9WZDNCRFJ3T0");
    assert!(windows8x.has_pc_package);
    assert_eq!(windows8x.size_label(), "43 MB");
    assert!(
        windows8x.content_ids.is_empty(),
        "and it is still not an update key"
    );

    // A Universal package beside a Windows 8 one: the regression that would
    // report a phantom update forever, and a doubled download size.
    let both = by_id("9NBLGGH4VVNH");
    assert_eq!(
        both.content_ids.len(),
        1,
        "one update key: {:?}",
        both.content_ids
    );

    // Console only. No PC download exists, which is an answer, not a gap.
    let console = by_id("C125W9BG2K0V");
    assert!(console.has_packages && !console.has_pc_package);
    assert_eq!(console.download_bytes, None);

    // A bundle: no packages of its own, and children to follow.
    let bundle = by_id("9NXP44L49SHJ");
    assert!(!bundle.has_packages);
    assert!(
        bundle.bundled_ids.len() >= 2,
        "children: {:?}",
        bundle.bundled_ids
    );
}

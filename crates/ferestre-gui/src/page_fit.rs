//! How large a list's tiles are: whole columns that use all of the view's
//! width, and whole rows that fill its height, with tiles near one size.
use super::*;

/// However small the view, this many tiles are laid out for.
const LEAST: usize = 4;
pub(crate) const MAX_COLUMNS: i32 = 30;
/// A tile's width: the one to be near, and how far from it one may be.
struct Sizes {
    ideal: f64,
    narrowest: i32,
    widest: i32,
    /// How flat a cover may be.
    flattest: f64,
}
const DESKTOP: Sizes = Sizes {
    ideal: 190.0,
    narrowest: 156,
    widest: 236,
    flattest: 1.26,
};
const HANDHELD: Sizes = Sizes {
    ideal: 184.0,
    narrowest: 144,
    widest: 224,
    flattest: 1.38,
};
/// A cover's height for its width. It is 3:2; one a little off it is cropped.
const COVER: f64 = 1.5;
const TALLEST: f64 = 1.62;

/// A tile's width and its cover's height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tile {
    pub width: i32,
    pub cover: i32,
    /// For a handheld: the words are on the cover.
    pub compact: bool,
}

impl Tile {
    pub fn row_spacing(&self) -> i32 {
        if self.compact {
            16
        } else {
            22
        }
    }

    pub fn column_spacing(&self) -> i32 {
        if self.compact {
            16
        } else {
            18
        }
    }
}

/// The tile for a shelf.
pub(crate) fn shelf_tile(ui: &Ui) -> Tile {
    if ui.window.has_css_class(handheld::CLASS) {
        Tile {
            width: 160,
            cover: 240,
            compact: true,
        }
    } else {
        TILE
    }
}

pub(crate) const TILE: Tile = Tile {
    width: 190,
    cover: 285,
    compact: false,
};

/// Until the view is measured.
pub(crate) const FIRST: Fit = Fit {
    columns: 10,
    rows: 2,
    tile: TILE,
};

/// How a view of a list is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fit {
    pub columns: usize,
    pub rows: usize,
    pub tile: Tile,
}

/// A resize reports every size on the way.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(120);

/// The layout that uses most of `width` by `height` with tiles nearest the
/// ideal, for tiles with `text` under their cover.
fn fit(width: i32, height: i32, text: i32, compact: bool) -> Fit {
    let sizes = if compact { HANDHELD } else { DESKTOP };
    let (across, down) = {
        let tile = Tile { compact, ..TILE };
        (tile.column_spacing(), tile.row_spacing())
    };
    let mut best: Option<(f64, Fit)> = None;
    for columns in 1..=MAX_COLUMNS {
        let each = (width - (columns - 1) * across) / columns;
        if each < sizes.narrowest && columns > 1 {
            break;
        }
        let tile_width = each.min(sizes.widest);
        let unused = f64::from((each - tile_width) * columns) / f64::from(width.max(1));
        let (low, high) = (
            (f64::from(tile_width) * sizes.flattest) as i32,
            (f64::from(tile_width) * TALLEST) as i32,
        );
        for rows in 1.. {
            let cover = (height - (rows - 1) * down) / rows - text;
            // One row is drawn even where it has to be scrolled to.
            if cover < low && rows > 1 {
                break;
            }
            let ratio = |cover: i32| f64::from(cover) / f64::from(tile_width.max(1));
            let shown = if cover < low {
                (f64::from(tile_width) * COVER) as i32
            } else {
                cover.min(high)
            };
            let spare = f64::from((cover - shown) * rows).abs() / f64::from(height.max(1));
            let score = 1.5 * (f64::from(tile_width) / sizes.ideal).ln().abs()
                + (ratio(shown) / COVER).ln().abs()
                + 3.0 * (unused + spare);
            if best.is_none_or(|(least, _)| score < least) {
                let fit = Fit {
                    columns: columns as usize,
                    rows: (rows as usize).max(LEAST.div_ceil(columns as usize)),
                    tile: Tile {
                        width: tile_width,
                        cover: shown,
                        compact,
                    },
                };
                best = Some((score, fit));
            }
        }
    }
    best.map_or(
        Fit {
            columns: 1,
            rows: LEAST,
            tile: Tile { compact, ..TILE },
        },
        |(_, fit)| fit,
    )
}

/// What stands above the list on its page, and how much of it is the hero.
fn above(grid: &endless::Endless, page: &gtk::Box, width: i32) -> (i32, i32) {
    let (mut above, mut hero) = (page.margin_top(), 0);
    let mut child = page.first_child();
    while let Some(widget) = child {
        if &widget == grid.upcast_ref::<gtk::Widget>() {
            break;
        }
        if widget.is_visible() {
            let height = widget.measure(gtk::Orientation::Vertical, width).1 + page.spacing();
            above += height;
            if widget.has_css_class("hero") {
                hero = height;
            }
        }
        child = widget.next_sibling();
    }
    (above, hero)
}

/// The layout for the list on the page now, and where it starts. The hero is
/// scrolled away, so tiles are sized for the view without it. `None` without a
/// list or before the window has a size.
fn measured(ui: &Ui) -> Option<(Fit, i32)> {
    let grid = ui.endless.borrow().upgrade()?;
    let page = grid.parent()?.downcast::<gtk::Box>().ok()?;
    let width = ui.scroll.width() - page.margin_start() - page.margin_end();
    let view = ui.scroll.height();
    if width <= 0 || view <= 0 {
        return None;
    }
    let (above, hero) = above(&grid, &page, width);
    let room = view - (above - hero) - page.margin_bottom();
    let compact = ui.window.has_css_class(handheld::CLASS);
    let text = if compact { 0 } else { ui.tile_text.get() };
    Some((fit(width, room, text, compact), above))
}

/// Lay out the list just drawn and make the tiles in view.
pub(crate) fn lay_out(ui: &Ui) {
    let Some(grid) = ui.endless.borrow().upgrade() else {
        return;
    };
    // Again when its text turns out taller than the last list's.
    for _ in 0..2 {
        let (fit, above) = measured(ui).unwrap_or((ui.tile.get(), 0));
        ui.tile.set(fit);
        grid.set_fit(fit);
        endless::sync(ui, Some(above));
        let text = grid.text();
        if fit.tile.compact || ui.tile_text.replace(text) == text {
            break;
        }
    }
}

/// Draw the list again when its tiles change size with the view.
pub(crate) fn follow_size(ui: &Ui) {
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let changed: Rc<dyn Fn()> = {
        let ui = ui.clone();
        Rc::new(move || {
            if let Some(old) = pending.take() {
                old.remove();
            }
            let (ui, done) = (ui.clone(), pending.clone());
            let id = glib::timeout_add_local_once(SETTLE, move || {
                done.take();
                let changed = handheld::update(&ui);
                let new = measured(&ui).is_some_and(|(fit, _)| fit != ui.tile.get());
                if (changed || new) && !*ui.closing.borrow() {
                    render_soon(&ui);
                }
            });
            pending.replace(Some(id));
        })
    };
    for adjustment in [ui.scroll.hadjustment(), ui.scroll.vadjustment()] {
        let changed = changed.clone();
        adjustment.connect_page_size_notify(move |_| changed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a layout leaves unused across and down.
    fn spare(width: i32, height: i32, fit: Fit) -> (i32, i32) {
        let (c, r) = (fit.columns as i32, fit.rows as i32);
        (
            width - c * fit.tile.width - (c - 1) * fit.tile.column_spacing(),
            height - r * (fit.tile.cover + 98) - (r - 1) * fit.tile.row_spacing(),
        )
    }

    #[test]
    fn tiles_use_all_the_room_at_a_sensible_size() {
        for width in (1000..5000).step_by(37) {
            for height in (740..1500).step_by(41) {
                let fit = fit(width, height, 98, false);
                let (across, down) = spare(width, height, fit);
                assert!(
                    (0..=width / 25).contains(&across),
                    "{width}x{height} {fit:?}"
                );
                assert!(
                    (0..=height / 9).contains(&down),
                    "{width}x{height} {fit:?} {down}"
                );
                assert!((DESKTOP.narrowest..=DESKTOP.widest).contains(&fit.tile.width));
            }
        }
    }

    #[test]
    fn a_wide_desktop_window_has_two_rows() {
        let fit = fit(2127, 760, 98, false);
        assert_eq!(fit.rows, 2, "{fit:?}");
        assert!(fit.columns * fit.rows >= 20, "{fit:?}");
    }

    #[test]
    fn a_small_view_still_has_four() {
        for small in [fit(1216, -50, 98, false), fit(420, 300, 98, false)] {
            assert!(small.columns * small.rows >= 4, "{small:?}");
        }
        assert_eq!(fit(100, 100, 98, false).columns, 1);
        assert_eq!(fit(100, 100, 98, false).rows, 4);
    }

    /// 720p and 800p handhelds, under the sections and above the hints.
    #[test]
    fn a_handheld_shows_two_rows_of_covers() {
        for (width, height) in [(1216, 560), (1216, 640)] {
            let fit = fit(width, height, 0, true);
            assert_eq!(fit.rows, 2, "{fit:?}");
            assert!((5..=8).contains(&fit.columns), "{fit:?}");
        }
    }
}

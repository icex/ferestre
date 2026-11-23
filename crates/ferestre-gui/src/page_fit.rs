//! How many tiles a page of a list holds, and how large they are: whole rows
//! and columns that use all of the view's room, with tiles near one size. The
//! page buttons stand below the list, outside what
//! scrolls, so they are in view whatever the page holds.
use super::*;

/// However small the view, a page is worth turning to.
const LEAST: usize = 4;
pub(crate) const ROW_SPACING: i32 = 22;
pub(crate) const COLUMN_SPACING: i32 = 18;
pub(crate) const MAX_COLUMNS: i32 = 30;
/// A tile's width: the one to be near, and how far from it one may be.
const IDEAL: f64 = 190.0;
const NARROWEST: i32 = 156;
const WIDEST: i32 = 236;
/// A cover's height for its width. It is 3:2; one a little off it is cropped.
const COVER: f64 = 1.5;
const FLATTEST: f64 = 1.26;
const TALLEST: f64 = 1.62;

/// A tile's width and its cover's height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tile {
    pub width: i32,
    pub cover: i32,
}

pub(crate) const TILE: Tile = Tile {
    width: 190,
    cover: 285,
};

/// Until the view is measured.
pub(crate) const FIRST: Fit = Fit {
    columns: 10,
    rows: 2,
    tile: TILE,
};

/// How a page is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fit {
    pub columns: usize,
    pub rows: usize,
    pub tile: Tile,
}

impl Fit {
    fn tiles(&self) -> usize {
        self.columns * self.rows
    }
}
/// A resize reports every size on the way.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(120);

/// The page buttons. Kept over redraws, and so is the focus on them.
#[derive(Clone)]
pub(crate) struct Pager {
    pub root: gtk::Box,
    bar: gtk::Box,
    previous: gtk::Button,
    next: gtk::Button,
    counter: gtk::Label,
}

impl Pager {
    pub fn new() -> Pager {
        let button = |icon: &str, label: &str| {
            let button = gtk::Button::from_icon_name(icon);
            button.set_tooltip_text(Some(label));
            button.update_property(&[gtk::accessible::Property::Label(label)]);
            button
        };
        let previous = button("go-previous-symbolic", "Previous page");
        let next = button("go-next-symbolic", "Next page");
        let counter = gtk::Label::builder().css_classes(["dim-label"]).build();
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        bar.append(&previous);
        bar.append(&counter);
        bar.append(&next);
        // The wrapper is what hides: the bar can then be measured while it is.
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&bar);
        root.set_visible(false);
        Pager {
            root,
            bar,
            previous,
            next,
            counter,
        }
    }

    pub fn connect(&self, ui: &Ui) {
        for (button, forward) in [(&self.previous, false), (&self.next, true)] {
            let ui = ui.clone();
            button.connect_clicked(move |_| {
                {
                    let mut model = ui.model.borrow_mut();
                    model.page = if forward {
                        model.page + 1
                    } else {
                        model.page.saturating_sub(1)
                    };
                }
                ui.scroll.vadjustment().set_value(0.0);
                render(&ui);
            });
        }
    }

    pub fn hide(&self) {
        self.root.set_visible(false);
    }

    /// Only when there is more than one page: a pager under a list that
    /// already fits is noise.
    pub fn show(&self, label: &str, has_previous: bool, has_next: bool) {
        let focused = |b: &gtk::Button| b.has_focus();
        let (on_previous, on_next) = (focused(&self.previous), focused(&self.next));
        self.counter.set_label(label);
        self.previous.set_sensitive(has_previous);
        self.next.set_sensitive(has_next);
        self.root.set_visible(has_previous || has_next);
        // The last page must not take the focus away from the page buttons.
        if on_next && !has_next && has_previous {
            self.previous.grab_focus();
        } else if on_previous && !has_previous && has_next {
            self.next.grab_focus();
        }
    }
}

/// The layout that uses most of `width` by `height` with tiles nearest the
/// ideal, for tiles with `text` under their cover.
fn fit(width: i32, height: i32, text: i32) -> Fit {
    let mut best: Option<(f64, Fit)> = None;
    for columns in 1..=MAX_COLUMNS {
        let each = (width - (columns - 1) * COLUMN_SPACING) / columns;
        if each < NARROWEST && columns > 1 {
            break;
        }
        let tile_width = each.min(WIDEST);
        let unused = f64::from((each - tile_width) * columns) / f64::from(width.max(1));
        let (low, high) = (
            (f64::from(tile_width) * FLATTEST) as i32,
            (f64::from(tile_width) * TALLEST) as i32,
        );
        for rows in 1.. {
            let cover = (height - (rows - 1) * ROW_SPACING) / rows - text;
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
            let score = 1.5 * (f64::from(tile_width) / IDEAL).ln().abs()
                + (ratio(shown) / COVER).ln().abs()
                + 3.0 * (unused + spare);
            if best.is_none_or(|(least, _)| score < least) {
                let fit = Fit {
                    columns: columns as usize,
                    rows: (rows as usize).max(LEAST.div_ceil(columns as usize)),
                    tile: Tile {
                        width: tile_width,
                        cover: shown,
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
            tile: TILE,
        },
        |(_, fit)| fit,
    )
}

/// The layouts of the page drawn now and of the others; `None` without tiles
/// or before the window has a size.
fn measured(ui: &Ui) -> Option<(Fit, Fit)> {
    fn find(root: &gtk::Widget, class: &str) -> Option<gtk::Widget> {
        if root.has_css_class(class) {
            return Some(root.clone());
        }
        let mut child = root.first_child();
        while let Some(widget) = child {
            if let Some(found) = find(&widget, class) {
                return Some(found);
            }
            child = widget.next_sibling();
        }
        None
    }
    let grid = find(ui.content.upcast_ref(), "paged-grid")?
        .downcast::<gtk::FlowBox>()
        .ok()?;
    let page = grid.parent()?.downcast::<gtk::Box>().ok()?;
    let width = ui.scroll.width() - page.margin_start() - page.margin_end();
    let view = ui.scroll.parent()?.height();
    if width <= 0 || view <= 0 {
        return None;
    }
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
    let pager = ui.pager.bar.measure(gtk::Orientation::Vertical, -1).1;
    let drawn = ui.tile.get();
    let mut tallest = 0;
    let mut index = 0;
    while let Some(cell) = grid.child_at_index(index) {
        tallest = tallest.max(cell.measure(gtk::Orientation::Vertical, drawn.tile.width).1);
        index += 1;
    }
    let text = (tallest - drawn.tile.cover).max(0);
    let room = view - pager - above - page.margin_bottom();
    let rest = fit(width, room + hero, text);
    let first = if hero > 0 {
        fit(width, room, text)
    } else {
        rest
    };
    Some((first, rest))
}

/// The page that shows what was first on `page`, when the sizes change.
fn same_place(page: usize, old: (usize, usize), new: (usize, usize)) -> usize {
    let start = model::page_start(page, old.0, old.1);
    if start < new.0 {
        0
    } else {
        1 + (start - new.0) / new.1.max(1)
    }
}

/// Take the sizes of what was just drawn; true when it must be drawn again.
pub(crate) fn update(ui: &Ui) -> bool {
    let Some((first, rest)) = measured(ui) else {
        return false;
    };
    let mut model = ui.model.borrow_mut();
    let old = model.page_size;
    // Drawn without the hero the first page has above its list, a later page
    // cannot say how many that one holds.
    let before = if model.page > 0 && model.section == Section::GamePass {
        old.0.min(rest.tiles())
    } else {
        first.tiles()
    };
    let new = (before, rest.tiles());
    if new == old && ui.tile.get() == first {
        return false;
    }
    ui.tile.set(first);
    model.page = same_place(model.page, old, new);
    model.page_size = new;
    true
}

/// Draw the list again when the view comes to hold more or fewer tiles.
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
                if !*ui.closing.borrow() && update(&ui) {
                    render(&ui);
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
            width - c * fit.tile.width - (c - 1) * COLUMN_SPACING,
            height - r * (fit.tile.cover + 98) - (r - 1) * ROW_SPACING,
        )
    }

    #[test]
    fn tiles_use_all_the_room_at_a_sensible_size() {
        for width in (1000..5000).step_by(37) {
            for height in (740..1500).step_by(41) {
                let fit = fit(width, height, 98);
                let (across, down) = spare(width, height, fit);
                assert!(
                    (0..=width / 25).contains(&across),
                    "{width}x{height} {fit:?}"
                );
                assert!(
                    (0..=height / 9).contains(&down),
                    "{width}x{height} {fit:?} {down}"
                );
                assert!((NARROWEST..=WIDEST).contains(&fit.tile.width));
            }
        }
    }

    #[test]
    fn a_wide_desktop_window_has_two_rows() {
        let fit = fit(2127, 760, 98);
        assert_eq!(fit.rows, 2, "{fit:?}");
        assert!(fit.tiles() >= 20, "{fit:?}");
    }

    #[test]
    fn a_small_view_still_has_four() {
        assert!(fit(1216, -50, 98).tiles() >= 4);
        assert!(fit(420, 300, 98).tiles() >= 4);
        assert_eq!(fit(100, 100, 98).columns, 1);
        assert_eq!(fit(100, 100, 98).rows, 4);
    }

    #[test]
    fn resizing_stays_at_the_same_titles() {
        assert_eq!(same_place(0, (20, 20), (5, 10)), 0);
        // Titles 21-40 begin at index 20: 5, then pages of 10.
        assert_eq!(same_place(1, (20, 20), (5, 10)), 2);
        assert_eq!(same_place(2, (5, 10), (20, 20)), 0);
    }
}

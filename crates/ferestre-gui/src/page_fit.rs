//! How many tiles a page of a list holds: as many whole rows of them as the
//! view has room for. The page buttons stand below the list, outside what
//! scrolls, so they are in view whatever the page holds.
use super::*;

/// However small the view, a page is worth turning to.
const LEAST: usize = 4;
pub(crate) const ROW_SPACING: i32 = 22;
pub(crate) const COLUMN_SPACING: i32 = 18;
pub(crate) const MAX_COLUMNS: i32 = 14;
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

/// The tiles that fit `width` by `height`, for tiles of this size.
fn tiles(width: i32, height: i32, tile: (i32, i32)) -> usize {
    let along =
        |room: i32, each: i32, spacing: i32| ((room + spacing) / (each.max(1) + spacing)).max(1);
    let columns = along(width, tile.0, COLUMN_SPACING).min(MAX_COLUMNS) as usize;
    let rows = along(height, tile.1, ROW_SPACING) as usize;
    // Whole rows, also when one row is fewer than the least.
    let rows = rows.max(LEAST.div_ceil(columns));
    columns * rows
}

/// What the page sizes should be for what is drawn now; `None` without tiles
/// or before the window has a size.
fn measured(ui: &Ui) -> Option<(usize, usize)> {
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
    let mut tile = (0, 0);
    let mut index = 0;
    while let Some(cell) = grid.child_at_index(index) {
        tile.0 = tile.0.max(cell.measure(gtk::Orientation::Horizontal, -1).1);
        index += 1;
    }
    for i in 0..index {
        if let Some(cell) = grid.child_at_index(i) {
            tile.1 = tile
                .1
                .max(cell.measure(gtk::Orientation::Vertical, tile.0).1);
        }
    }
    let room = view - pager - above;
    let rest = tiles(width, room + hero, tile);
    let first = if hero > 0 {
        tiles(width, room, tile)
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
    // A later page does not show the hero the first one has above its list.
    let first = if model.page > 0 && model.section == Section::GamePass {
        old.0.min(rest)
    } else {
        first
    };
    let new = (first, rest);
    if new == old {
        return false;
    }
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

    #[test]
    fn a_page_is_the_whole_rows_that_fit() {
        // A 1280x720 handheld, full screen, one row of 196x400.
        assert_eq!(tiles(1216, 500, (196, 400)), 5);
        assert_eq!(tiles(1216, 821, (196, 400)), 5);
        assert_eq!(tiles(1216, 822, (196, 400)), 10);
        // A very wide screen stops at the grid's columns.
        assert_eq!(tiles(5000, 1300, (196, 400)), 42);
    }

    #[test]
    fn a_small_view_still_has_four() {
        assert_eq!(tiles(1216, -50, (196, 400)), 5);
        assert_eq!(tiles(420, 300, (196, 400)), 4);
        assert_eq!(tiles(640, 300, (196, 400)), 6);
        assert_eq!(tiles(100, 100, (196, 400)), 4);
    }

    #[test]
    fn resizing_stays_at_the_same_titles() {
        assert_eq!(same_place(0, (20, 20), (5, 10)), 0);
        // Titles 21-40 begin at index 20: 5, then pages of 10.
        assert_eq!(same_place(1, (20, 20), (5, 10)), 2);
        assert_eq!(same_place(2, (5, 10), (20, 20)), 0);
    }
}

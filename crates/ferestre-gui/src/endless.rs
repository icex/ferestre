//! A list's tiles, all on one scrolled page. Only the tiles near the view are
//! there: the rest are room, and are made as scrolling or the focus comes near
//! them. The rows next to the view are made at once, so a controller moving
//! the focus always has a next tile, and the ones further out while idle.
use super::*;
use std::collections::BTreeMap;

type Make = Rc<dyn Fn(usize, page_fit::Tile) -> gtk::Widget>;
/// Told of the tiles near the view when they change.
type Near = Rc<dyn Fn(std::ops::Range<usize>)>;

mod imp {
    use super::*;
    use gtk::subclass::prelude::*;

    pub struct Endless {
        pub(super) cards: RefCell<BTreeMap<usize, gtk::Widget>>,
        pub(super) total: Cell<usize>,
        pub(super) fit: Cell<page_fit::Fit>,
        /// The tallest text under a cover so far.
        pub(super) text: Cell<i32>,
        /// Kept wherever the view is: what has the focus, or is to get it.
        pub(super) pinned: Cell<Option<usize>>,
        pub(super) make: RefCell<Option<Make>>,
        pub(super) near: RefCell<Option<Near>>,
        pub(super) told: RefCell<std::ops::Range<usize>>,
        pub(super) idle: Cell<bool>,
        /// The rows to have while idle.
        pub(super) rows: RefCell<std::ops::Range<usize>>,
    }

    impl Default for Endless {
        fn default() -> Self {
            Endless {
                cards: RefCell::default(),
                total: Cell::new(0),
                fit: Cell::new(page_fit::FIRST),
                text: Cell::new(0),
                pinned: Cell::new(None),
                make: RefCell::default(),
                near: RefCell::default(),
                told: RefCell::new(0..0),
                idle: Cell::new(false),
                rows: RefCell::new(0..0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Endless {
        const NAME: &'static str = "FerestreEndless";
        type Type = super::Endless;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Endless {
        fn dispose(&self) {
            for card in self.cards.take().into_values() {
                card.unparent();
            }
            self.make.take();
            self.near.take();
        }
    }

    impl WidgetImpl for Endless {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let fit = self.fit.get();
            match orientation {
                // One column asks for no more: a narrower window is laid out anew.
                gtk::Orientation::Horizontal => (
                    fit.tile.width.min(page_fit::TILE.width),
                    fit.tile.width,
                    -1,
                    -1,
                ),
                _ => {
                    let height = self.obj().full_height();
                    (height, height, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let obj = self.obj();
            for (index, card) in self.cards.borrow().iter() {
                obj.place(*index, card, width);
            }
        }
    }
}

glib::wrapper! {
    pub struct Endless(ObjectSubclass<imp::Endless>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Endless {
    fn imp(&self) -> &imp::Endless {
        gtk::subclass::prelude::ObjectSubclassIsExt::imp(self)
    }

    /// `pinned` is made wherever it is.
    pub(crate) fn new(
        total: usize,
        text: i32,
        pinned: Option<usize>,
        make: impl Fn(usize, page_fit::Tile) -> gtk::Widget + 'static,
        near: impl Fn(std::ops::Range<usize>) + 'static,
    ) -> Endless {
        let grid: Endless = glib::Object::new();
        grid.set_accessible_role(gtk::AccessibleRole::Group);
        grid.update_property(&[gtk::accessible::Property::Label("Games")]);
        grid.add_css_class("game-grid");
        grid.set_hexpand(true);
        let imp = grid.imp();
        imp.total.set(total);
        imp.text.set(text);
        imp.pinned.set(pinned);
        imp.make.replace(Some(Rc::new(make)));
        imp.near.replace(Some(Rc::new(near)));
        grid
    }

    /// Put a tile where it goes in a list this wide.
    fn place(&self, index: usize, card: &gtk::Widget, width: i32) {
        let fit = self.imp().fit.get();
        let columns = fit.columns.max(1);
        let step = match columns {
            1 => 0,
            n => ((width - fit.tile.width) / (n as i32 - 1))
                .min(fit.tile.width + 2 * fit.tile.column_spacing()),
        };
        // GTK wants a size asked for before it is given.
        let _ = card.measure(gtk::Orientation::Horizontal, -1);
        let _ = card.measure(gtk::Orientation::Vertical, fit.tile.width);
        card.size_allocate(
            &gtk::Allocation::new(
                (index % columns) as i32 * step,
                (index / columns) as i32 * self.stride(),
                fit.tile.width,
                self.row_height(),
            ),
            -1,
        );
    }

    /// The tile `focus` is in.
    fn index_of(&self, focus: &gtk::Widget) -> Option<usize> {
        self.imp()
            .cards
            .borrow()
            .iter()
            .find(|(_, card)| focus.is_ancestor(*card))
            .map(|(index, _)| *index)
    }

    /// The tallest text under a cover so far.
    pub(crate) fn text(&self) -> i32 {
        self.imp().text.get()
    }

    /// Lay out anew. Tiles of another size are dropped.
    pub(crate) fn set_fit(&self, fit: page_fit::Fit) {
        if self.imp().fit.replace(fit) != fit {
            self.clear();
        }
        self.queue_resize();
    }

    pub(crate) fn clear(&self) {
        for card in self.imp().cards.take().into_values() {
            card.unparent();
        }
        self.imp().told.replace(0..0);
    }

    fn row_height(&self) -> i32 {
        let tile = self.imp().fit.get().tile;
        // A compact tile's words are on its cover.
        tile.cover
            + if tile.compact {
                0
            } else {
                self.imp().text.get()
            }
    }

    fn stride(&self) -> i32 {
        self.row_height() + self.imp().fit.get().tile.row_spacing()
    }

    fn rows(&self) -> usize {
        self.imp()
            .total
            .get()
            .div_ceil(self.imp().fit.get().columns.max(1))
    }

    fn full_height(&self) -> i32 {
        let rows = self.rows() as i32;
        (rows * self.stride() - self.imp().fit.get().tile.row_spacing()).max(0)
    }

    fn row_tiles(&self, row: usize) -> std::ops::Range<usize> {
        let columns = self.imp().fit.get().columns.max(1);
        (row * columns).min(self.imp().total.get())
            ..((row + 1) * columns).min(self.imp().total.get())
    }

    /// Make a row's tiles; false when they are all there.
    fn make_row(&self, row: usize) -> bool {
        let mut made = false;
        for index in self.row_tiles(row) {
            made |= self.make_tile(index);
        }
        made
    }

    fn make_tile(&self, index: usize) -> bool {
        let imp = self.imp();
        if index >= imp.total.get() || imp.cards.borrow().contains_key(&index) {
            return false;
        }
        let Some(make) = imp.make.borrow().clone() else {
            return false;
        };
        let tile = imp.fit.get().tile;
        let card = make(index, tile);
        // In the list's order, which is the order Tab goes in.
        let next = imp
            .cards
            .borrow()
            .range(index + 1..)
            .next()
            .map(|(_, card)| card.clone());
        card.insert_before(self, next.as_ref());
        let text = card.measure(gtk::Orientation::Vertical, tile.width).1 - tile.cover;
        imp.text.set(imp.text.get().max(text));
        // At once, so the focus can move to it before the next layout.
        if self.width() > 0 {
            self.place(index, &card, self.width());
        }
        imp.cards.borrow_mut().insert(index, card);
        true
    }

    /// The rows from `top` to `bottom`, in this widget.
    fn rows_between(&self, top: f64, bottom: f64) -> std::ops::Range<usize> {
        let stride = f64::from(self.stride().max(1));
        let first = (top / stride).floor().max(0.0) as usize;
        let last = (bottom / stride).ceil().max(0.0) as usize;
        first.min(self.rows())..last.min(self.rows())
    }

    /// Have the tiles for a view from `top` to `bottom`, in this widget.
    fn show(&self, top: f64, bottom: f64, focus: Option<&gtk::Widget>) {
        let imp = self.imp();
        let view = (bottom - top).max(1.0);
        let shown = self.rows_between(top, bottom);
        let mut made = false;
        for row in shown.start.saturating_sub(1)..(shown.end + 1).min(self.rows()) {
            made |= self.make_row(row);
        }
        if let Some(pinned) = imp.pinned.get() {
            made |= self.make_tile(pinned);
        }
        // Far ones go, except where the focus is.
        let near = self.rows_between(top - view, bottom + view);
        let keep = self.rows_between(top - 2.0 * view, bottom + 2.0 * view);
        let columns = imp.fit.get().columns.max(1);
        let far: Vec<usize> = imp
            .cards
            .borrow()
            .iter()
            .filter(|(index, card)| {
                !keep.contains(&(**index / columns))
                    && imp.pinned.get() != Some(**index)
                    && !focus.is_some_and(|f| f.is_ancestor(*card))
            })
            .map(|(index, _)| *index)
            .collect();
        for index in far {
            let card = imp.cards.borrow_mut().remove(&index);
            if let Some(card) = card {
                card.unparent();
            }
        }
        if made {
            self.queue_resize();
        }
        let tiles = self.row_tiles(near.start).start
            ..near
                .end
                .checked_sub(1)
                .map_or(0, |last| self.row_tiles(last).end);
        if *imp.told.borrow() != tiles {
            imp.told.replace(tiles.clone());
            let tell = imp.near.borrow().clone();
            if let Some(tell) = tell {
                tell(tiles);
            }
        }
        self.make_while_idle(near);
    }

    /// The nearby rows not there yet, one at a time.
    fn make_while_idle(&self, near: std::ops::Range<usize>) {
        self.imp().rows.replace(near);
        if self.missing_row().is_none() || self.imp().idle.replace(true) {
            return;
        }
        glib::idle_add_local_full(
            glib::Priority::DEFAULT_IDLE,
            glib::clone!(
                #[weak(rename_to = grid)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || match grid.missing_row() {
                    Some(row) if grid.is_mapped() => {
                        grid.make_row(row);
                        grid.queue_resize();
                        glib::ControlFlow::Continue
                    }
                    _ => {
                        grid.imp().idle.set(false);
                        glib::ControlFlow::Break
                    }
                }
            ),
        );
    }

    fn missing_row(&self) -> Option<usize> {
        let cards = self.imp().cards.borrow();
        let rows = self.imp().rows.borrow().clone();
        rows.into_iter()
            .find(|row| self.row_tiles(*row).any(|i| !cards.contains_key(&i)))
    }
}

/// Have the tiles the view shows. `above` is where the list starts in what
/// scrolls, for one that has not been allocated.
pub(crate) fn sync(ui: &Ui, above: Option<i32>) {
    let Some(grid) = ui.endless.borrow().upgrade() else {
        return;
    };
    let start = above.map(f64::from).or_else(|| {
        let at = grid.compute_point(&ui.content, &gtk::graphene::Point::new(0.0, 0.0))?;
        (grid.height() > 0).then(|| f64::from(at.y()))
    });
    let Some(start) = start else { return };
    let adjustment = ui.scroll.vadjustment();
    let view = match adjustment.page_size() {
        size if size > 1.0 => size,
        _ => 900.0,
    };
    let top = adjustment.value() - start;
    let focus = GtkWindowExt::focus(&ui.window);
    grid.show(top, top + view, focus.as_ref());
}

/// Follow the view. One handler for the window: lists come and go.
pub(crate) fn follow(ui: &Ui) {
    let pending: Rc<Cell<bool>> = Rc::default();
    let changed: Rc<dyn Fn()> = {
        let ui = ui.clone();
        Rc::new(move || {
            if pending.replace(true) {
                return;
            }
            let (ui, pending) = (ui.clone(), pending.clone());
            // Out of the layout that may be telling, and before the frame.
            glib::idle_add_local_full(glib::Priority::HIGH_IDLE, move || {
                pending.set(false);
                if !*ui.closing.borrow() {
                    sync(&ui, None);
                }
                glib::ControlFlow::Break
            });
        })
    };
    let adjustment = ui.scroll.vadjustment();
    let value = changed.clone();
    adjustment.connect_value_changed(move |_| value());
    adjustment.connect_changed(move |_| changed());
}

/// Have the rows next to the focus, which may be out of view.
pub(crate) fn around(ui: &Ui, focus: &gtk::Widget) {
    let Some(grid) = ui.endless.borrow().upgrade() else {
        return;
    };
    let Some(index) = grid.index_of(focus) else {
        return;
    };
    let row = index / grid.imp().fit.get().columns.max(1);
    for row in row.saturating_sub(1)..(row + 2).min(grid.rows()) {
        grid.make_row(row);
    }
}

/// Move the focus a view further through the list, or back. False when the
/// focus is not in one.
pub(crate) fn jump(ui: &Ui, focus: &gtk::Widget, down: bool) -> bool {
    let Some(grid) = ui.endless.borrow().upgrade() else {
        return false;
    };
    let Some(index) = grid.index_of(focus) else {
        return false;
    };
    let columns = grid.imp().fit.get().columns.max(1);
    let rows = (ui.scroll.vadjustment().page_size() / f64::from(grid.stride().max(1))) as usize;
    let to = jump_to(index, columns, rows, grid.imp().total.get(), down);
    grid.make_row(to / columns);
    let card = grid.imp().cards.borrow().get(&to).cloned();
    if let Some(card) = card {
        // Its game first, and not the menu over it.
        fn primary(root: &gtk::Widget) -> Option<gtk::Widget> {
            if root.widget_name().ends_with("-primary") {
                return Some(root.clone());
            }
            let mut child = root.first_child();
            while let Some(widget) = child {
                if let Some(found) = primary(&widget) {
                    return Some(found);
                }
                child = widget.next_sibling();
            }
            None
        }
        if !primary(&card).is_some_and(|button| button.grab_focus()) {
            card.child_focus(gtk::DirectionType::TabForward);
        }
    }
    true
}

/// The tile `rows` rows from `index`, in its column where there is one.
fn jump_to(index: usize, columns: usize, rows: usize, total: usize, down: bool) -> usize {
    let (row, column) = (index / columns, index % columns);
    let last = total.saturating_sub(1);
    let row = if down {
        (row + rows.max(1)).min(last / columns)
    } else {
        row.saturating_sub(rows.max(1))
    };
    (row * columns + column).min(last)
}

/// The product id in a tile's control's name, `game-<id>-primary` and such.
pub(crate) fn product_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("game-")?;
    Some(rest.rsplit_once('-')?.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tiles_controls_name_their_product() {
        assert_eq!(product_of("game-9ZZA-primary"), Some("9ZZA"));
        assert_eq!(product_of("game-LOCAL-X-Y-more"), Some("LOCAL-X-Y"));
        assert_eq!(product_of("feature-9ZZA-primary"), None);
    }

    #[test]
    fn a_jump_keeps_to_its_column_and_the_list() {
        assert_eq!(jump_to(2, 5, 2, 43, true), 12);
        assert_eq!(jump_to(39, 5, 2, 43, true), 42);
        assert_eq!(jump_to(7, 5, 2, 43, false), 2);
        assert_eq!(jump_to(0, 5, 2, 1, true), 0);
    }
}

//! Home: a featured hero, quick links, and shelves of the library.
//!
//! From the top: a hero carousel of up to five titles (what is running or was
//! played last, then updates, then Game Pass titles with banner art), with the
//! neighbours peeking dimmed at either side; outlined quick links; "Jump back
//! in", the installed titles as landscape tiles, most recently played first;
//! shelves of PC Game Pass and the rest of the library; and two-up featured
//! cards. Every part shows only when it has something real to show.
//!
//! A title appears at most once among the `game-` controls on the page, so
//! their names ("Halo · Play") stay unique for the controller and for
//! assistive technology. The hero and featured cards are `feature-` controls,
//! which the controller does not pick as the page's first focus.
use super::*;

/// Shelves hold this many tiles at most; "See all" opens the full list.
const SHELF: usize = 12;
const HERO_ITEMS: usize = 5;

/// One title in the hero: the row, its banner and its one-line tagline.
#[derive(Clone)]
pub(crate) struct Feature {
    row: LibraryRow,
    art: Option<PathBuf>,
    cover: Option<PathBuf>,
    tagline: String,
}

fn tagline(model: &Model, row: &LibraryRow, gamepass: bool) -> String {
    if row.action == Action::Stop {
        return "Running now".into();
    }
    if row.has_update() {
        return "Update available".into();
    }
    if row.installed {
        let summary = model
            .recipe_for(&row.product_id)
            .map(|r| r.status.summary.trim().to_string())
            .filter(|s| (20..=90).contains(&s.len()));
        return summary.unwrap_or_else(|| "Installed · Ready to play".into());
    }
    if gamepass {
        "In PC Game Pass".into()
    } else {
        "In your library".into()
    }
}

/// The titles the carousel would show if their banners were on disk, in the
/// order [`features`] picks from, so their landscape art can be fetched.
pub(crate) fn hero_candidates(library: &[LibraryRow], gamepass: &[LibraryRow]) -> Vec<String> {
    const LOOK_AHEAD: usize = 12;
    let mut ids: Vec<String> = Vec::new();
    let mut push = |row: &LibraryRow| {
        let id = row.product_id.to_ascii_uppercase();
        if ids.len() < LOOK_AHEAD && !ids.contains(&id) {
            ids.push(id);
        }
    };
    library
        .iter()
        .filter(|r| r.installed)
        .take(2)
        .for_each(&mut push);
    library
        .iter()
        .filter(|r| r.has_update())
        .for_each(&mut push);
    gamepass.iter().filter(|r| !r.installed).for_each(&mut push);
    library.iter().for_each(&mut push);
    ids
}

fn features(model: &Model, library: &[LibraryRow], gamepass: &[LibraryRow]) -> Vec<Feature> {
    // Only art that is on disk: a title shown on a path with nothing behind
    // it was a blank banner in the carousel.
    let hero = |id: &str| model.wide_art(id);
    let mut out: Vec<Feature> = Vec::new();
    let mut add = |row: &LibraryRow, gp: bool, need_art: bool| {
        if out.len() >= HERO_ITEMS
            || out
                .iter()
                .any(|f| f.row.product_id.eq_ignore_ascii_case(&row.product_id))
        {
            return;
        }
        let art = hero(&row.product_id);
        let cover = model
            .icons
            .get(&row.product_id.to_ascii_uppercase())
            .filter(|path| path.is_file())
            .cloned();
        // Titles chosen to fill the carousel need a banner of their own; the
        // ones it is about (running, recent, with an update) may stand on
        // their cover instead. A title with no art at all is left out rather
        // than shown as an empty card.
        if (need_art && art.is_none()) || (art.is_none() && cover.is_none()) {
            return;
        }
        out.push(Feature {
            row: row.clone(),
            cover,
            art,
            tagline: tagline(model, row, gp),
        });
    };
    let mut installed: Vec<&LibraryRow> = library.iter().filter(|r| r.installed).collect();
    installed.sort_by_key(|r| {
        (
            r.action != Action::Stop,
            std::cmp::Reverse(model.last_played(&r.product_id)),
        )
    });
    for row in installed.iter().take(2) {
        add(row, false, false);
    }
    for row in library.iter().filter(|r| r.has_update()) {
        add(row, false, false);
    }
    for row in gamepass.iter().filter(|r| !r.installed) {
        add(row, true, true);
    }
    for row in library.iter() {
        add(row, false, true);
    }
    out
}

/// Hold a widget to a size: art decoded at 1280 px wide would otherwise ask
/// for 1280 px, and a box gives children their natural size first.
pub(crate) fn sized(child: &impl IsA<gtk::Widget>, width: Option<i32>, height: i32) -> gtk::Widget {
    child.set_size_request(width.unwrap_or(-1), height);
    let tall = adw::Clamp::builder()
        .orientation(gtk::Orientation::Vertical)
        .maximum_size(height)
        .tightening_threshold(height)
        .child(child)
        .build();
    match width {
        Some(width) => adw::Clamp::builder()
            .maximum_size(width)
            .tightening_threshold(width)
            .child(&tall)
            .build()
            .upcast(),
        None => tall.upcast(),
    }
}

/// Draws the hero, turned by so many places.
type Fill = Rc<dyn Fn(&Ui, isize)>;

/// The hero carousel, for Home and Game Pass.
pub(crate) fn hero(ui: &Ui, page: &gtk::Box, items: Vec<Feature>) {
    if items.is_empty() {
        return;
    }
    let items = Rc::new(items);
    let index = ui.hero_index.get() % items.len();
    ui.hero_index.set(index);
    let stage = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .css_classes(["hero-stage"])
        .build();
    // Sizes follow the allocated width, so a resize needs no rebuild.
    let layout = HeroLayout::default();
    stage.set_layout_manager(Some(layout.clone()));
    let dots = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .valign(gtk::Align::Center)
        .css_classes(["hero-dots"])
        .build();
    dots.set_accessible_role(gtk::AccessibleRole::Presentation);
    let previous = gtk::Button::from_icon_name("go-previous-symbolic");
    previous.set_widget_name("hero-previous");
    previous.set_tooltip_text(Some("Previous featured"));
    previous.update_property(&[gtk::accessible::Property::Label("Previous featured")]);
    previous.add_css_class("flat");
    previous.add_css_class("circular");
    let next = gtk::Button::from_icon_name("go-next-symbolic");
    next.set_widget_name("hero-next");
    next.set_tooltip_text(Some("Next featured"));
    next.update_property(&[gtk::accessible::Property::Label("Next featured")]);
    next.add_css_class("flat");
    next.add_css_class("circular");
    let controls = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .halign(gtk::Align::Center)
        .build();
    controls.append(&previous);
    controls.append(&dots);
    controls.append(&next);
    if items.len() < 2 {
        controls.set_visible(false);
    }
    // The turn in progress: it runs from where the last one had got to.
    let turn: Rc<RefCell<Option<adw::TimedAnimation>>> = Rc::default();
    let fill: Fill = {
        let (stage, dots, items) = (stage.clone(), dots.clone(), items.clone());
        Rc::new(move |ui: &Ui, step: isize| {
            let animate =
                step != 0 && gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
            let running = turn.take();
            let mut from = 0.0;
            if let Some(running) = running {
                from = running.value();
                running.pause();
            }
            let index = ui.hero_index.get() % items.len();
            let had_focus = GtkWindowExt::focus(&ui.window)
                .is_some_and(|f| f.is_ancestor(&stage) || f.has_css_class("hero-centre"));
            while let Some(child) = stage.first_child() {
                stage.remove(&child);
            }
            while let Some(child) = dots.first_child() {
                dots.remove(&child);
            }
            let n = items.len();
            // The title that leaves the stage on this turn, shown until it has.
            let leaving = |by: usize, next: bool| {
                let gone = peek(ui, &items[(index + by) % n], next);
                gone.set_can_target(false);
                gone.set_tooltip_text(None);
                gone.add_css_class("hero-leaving");
                gone
            };
            if animate && step > 0 {
                stage.append(&leaving(2 * n - 2, false));
            }
            if n > 1 {
                let side = peek(ui, &items[(index + n - 1) % n], false);
                if animate && step > 0 {
                    side.add_css_class("hero-was-centre");
                } else if animate {
                    side.add_css_class("hero-arriving");
                }
                stage.append(&side);
            }
            let centre = centre(ui, &items[index], index == 0);
            if animate {
                centre.add_css_class("hero-was-peek");
            }
            stage.append(&centre);
            if n > 1 {
                let side = peek(ui, &items[(index + 1) % n], true);
                if animate && step < 0 {
                    side.add_css_class("hero-was-centre");
                } else if animate {
                    side.add_css_class("hero-arriving");
                }
                stage.append(&side);
            }
            if animate && step < 0 {
                stage.append(&leaving(2, true));
            }
            layout.set_first(match (n > 1, animate && step > 0) {
                (false, _) => 0,
                (true, false) => -1,
                (true, true) => -2,
            });
            layout.set_offset(0.0);
            if animate {
                let from = (from + step as f64).clamp(-2.0, 2.0);
                layout.set_offset(from);
                let target = adw::CallbackAnimationTarget::new(glib::clone!(
                    #[weak]
                    layout,
                    move |value| layout.set_offset(value)
                ));
                let animation = adw::TimedAnimation::builder()
                    .widget(&stage)
                    .value_from(from)
                    .value_to(0.0)
                    .duration(HERO_TURN_MS)
                    .easing(adw::Easing::EaseOutCubic)
                    .target(&target)
                    .build();
                animation.connect_done(glib::clone!(
                    #[weak]
                    stage,
                    #[weak]
                    layout,
                    move |_| {
                        let mut child = stage.first_child();
                        while let Some(c) = child {
                            child = c.next_sibling();
                            if c.has_css_class("hero-leaving") {
                                stage.remove(&c);
                            }
                        }
                        layout.set_first(-1);
                    }
                ));
                animation.play();
                turn.replace(Some(animation));
            }
            for i in 0..n {
                let dot = gtk::Box::builder()
                    .css_classes(if i == index {
                        vec!["hero-dot", "active"]
                    } else {
                        vec!["hero-dot"]
                    })
                    .valign(gtk::Align::Center)
                    .build();
                dots.append(&dot);
            }
            if had_focus {
                centre.grab_focus();
            }
        })
    };
    fill(ui, 0);
    for (button, step) in [(previous, -1isize), (next, 1)] {
        let (ui, fill, n) = (ui.clone(), fill.clone(), items.len());
        button.connect_clicked(move |_| {
            let index = (ui.hero_index.get() as isize + step).rem_euclid(n as isize);
            ui.hero_index.set(index as usize);
            fill(&ui, step);
        });
    }
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .css_classes(["hero"])
        .build();
    column.append(&stage);
    column.append(&controls);
    page.append(&column);
}

fn art_picture(path: Option<&Path>, fallback: Option<&Path>, size: textures::Size) -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Cover)
        .hexpand(true)
        .vexpand(true)
        .build();
    if let Some(path) = path {
        textures::set_picture(&picture, path, size);
    } else if let Some(path) = fallback {
        textures::set_picture(&picture, path, textures::Size::Cover);
    } else {
        picture.add_css_class("art-loading");
    }
    picture
}

/// Lays out the hero stage: the centre banner at most [`HERO_MAX`] wide and
/// centred, each neighbour a fixed share of it at either side, and the height
/// from the centre's width. Whatever is left on a very wide window stays empty
/// margin, so the neighbours never outgrow the banner they frame.
mod hero_layout {
    use gtk::{glib, prelude::*, subclass::prelude::*};

    #[derive(Default)]
    pub struct Imp {
        /// Where the first child stands: 0 the centre, -1 left of it, -2 gone.
        pub first: std::cell::Cell<i32>,
        /// How far a turn still has to go, in places.
        pub offset: std::cell::Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Imp {
        const NAME: &'static str = "FerestreHeroLayout";
        type Type = super::HeroLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for Imp {}

    fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut out = Vec::new();
        let mut child = widget.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            if c.should_layout() {
                out.push(c);
            }
        }
        out
    }

    impl LayoutManagerImpl for Imp {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let kids = children(widget);
            let peeks = kids.len() > 1;
            match orientation {
                gtk::Orientation::Horizontal => {
                    // A title leaving during a turn asks for no room.
                    let kept = kids.len().min(3);
                    let min = kids
                        .iter()
                        .filter(|c| !c.has_css_class("hero-leaving"))
                        .map(|c| c.measure(orientation, -1).0)
                        .sum::<i32>()
                        + super::HERO_GAP * (kept.max(1) as i32 - 1);
                    let (_, _, natural) = super::hero_geometry(i32::MAX, peeks);
                    (min, natural.max(min), -1, -1)
                }
                gtk::Orientation::Vertical => {
                    let (centre, _, _) =
                        super::hero_geometry(if for_size < 0 { i32::MAX } else { for_size }, peeks);
                    let height = super::hero_height(centre);
                    let min = kids
                        .iter()
                        .map(|c| c.measure(orientation, -1).0)
                        .max()
                        .unwrap_or(0);
                    (height.max(min), height.max(min), -1, -1)
                }
                _ => (0, 0, -1, -1),
            }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let kids = children(widget);
            let peeks = kids.len() > 1;
            let (first, offset) = (f64::from(self.first.get()), self.offset.get());
            for (i, child) in kids.iter().enumerate() {
                let (x, wanted) = super::hero_place(width, peeks, first + i as f64 + offset);
                let w = wanted.max(child.measure(gtk::Orientation::Horizontal, height).0);
                let h = height.max(child.measure(gtk::Orientation::Vertical, w).0);
                child.size_allocate(&gtk::Allocation::new(x, 0, w, h), -1);
            }
        }
    }
}

glib::wrapper! {
    pub struct HeroLayout(ObjectSubclass<hero_layout::Imp>) @extends gtk::LayoutManager;
}

impl Default for HeroLayout {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl HeroLayout {
    fn set_first(&self, first: i32) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        self.imp().first.set(first);
        self.layout_changed();
    }

    fn set_offset(&self, offset: f64) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        self.imp().offset.set(offset);
        self.layout_changed();
    }
}

/// How long a turn of the hero takes.
const HERO_TURN_MS: u32 = 380;
const HERO_MAX: i32 = 1400;
const HERO_GAP: i32 = 16;
/// Each neighbour is this share of the centre banner's width.
const HERO_PEEK: f64 = 0.16;

/// For a stage `width` wide: the centre's width, each neighbour's, and the
/// width of the three together.
fn hero_geometry(width: i32, peeks: bool) -> (i32, i32, i32) {
    let width = f64::from(width.max(0));
    if !peeks {
        let centre = width.min(f64::from(HERO_MAX)) as i32;
        return (centre, 0, centre);
    }
    let gaps = f64::from(2 * HERO_GAP);
    let centre = ((width - gaps) / (1.0 + 2.0 * HERO_PEEK))
        .min(f64::from(HERO_MAX))
        .max(0.0);
    let peek = (centre * HERO_PEEK).floor();
    let centre = centre.floor();
    (
        centre as i32,
        peek as i32,
        (centre + 2.0 * peek + gaps) as i32,
    )
}

/// Left edge and width at `place`: 0 the centre, 1 its right neighbour, 2 past
/// that and half as wide, inside the stage's edge; between them, in between.
fn hero_place(width: i32, peeks: bool, place: f64) -> (i32, i32) {
    let (centre, peek, total) = hero_geometry(width, peeks);
    let left = f64::from(((width - total) / 2).max(0));
    let (centre, peek, total) = (f64::from(centre), f64::from(peek), f64::from(total));
    let gap = f64::from(HERO_GAP);
    if !peeks {
        return (left as i32, centre as i32);
    }
    let half = (peek / 2.0).floor();
    let stops = [
        (left, half),
        (left, peek),
        (left + peek + gap, centre),
        (left + total - peek, peek),
        (left + total - half, half),
    ];
    let at = (place + 2.0).clamp(0.0, 4.0);
    let (low, part) = (at.floor() as usize, at.fract());
    let (a, b) = (stops[low], stops[(low + 1).min(4)]);
    let mix = |a: f64, b: f64| (a + (b - a) * part).round() as i32;
    (mix(a.0, b.0), mix(a.1, b.1))
}

/// A wide banner, between 220 and 440 px tall.
fn hero_height(centre: i32) -> i32 {
    (f64::from(centre) / 2.6).clamp(220.0, 440.0) as i32
}

/// The centre banner: one button, the title's main action.
fn centre(ui: &Ui, feature: &Feature, first: bool) -> gtk::Widget {
    let row = &feature.row;
    let art = art_picture(
        feature.art.as_deref(),
        feature.cover.as_deref(),
        textures::Size::Hero,
    );
    ui.art_slots.borrow_mut().add_picture(
        format!("hero-{}", row.product_id.to_ascii_uppercase()),
        &art,
        textures::Size::Hero,
    );
    let overlay = gtk::Overlay::builder().css_classes(["hero-art"]).build();
    overlay.set_child(Some(&art));
    let caption = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .valign(gtk::Align::End)
        .halign(gtk::Align::Fill)
        .css_classes(["hero-caption"])
        .build();
    caption.append(
        &gtk::Label::builder()
            .label(&row.name)
            .xalign(0.0)
            .wrap(true)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["hero-title"])
            .build(),
    );
    caption.append(
        &gtk::Label::builder()
            .label(&feature.tagline)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["hero-tagline"])
            .build(),
    );
    overlay.add_overlay(&caption);
    let button = feature_button(ui, row, &overlay);
    button.set_overflow(gtk::Overflow::Hidden);
    button.add_css_class("hero-centre");
    // A card's action is centred in its row; the banner fills the stage, also
    // while a turn has it narrow.
    button.set_valign(gtk::Align::Fill);
    button.set_halign(gtk::Align::Fill);
    if first {
        button.add_css_class("hero-first");
    }
    button.upcast()
}

/// A dimmed neighbour; clicking it turns the carousel. Never focused: the
/// controller turns it with Left and Right on the centre instead.
fn peek(ui: &Ui, feature: &Feature, next: bool) -> gtk::Widget {
    let art = art_picture(
        feature.art.as_deref(),
        feature.cover.as_deref(),
        textures::Size::Hero,
    );
    let button = gtk::Button::builder()
        .child(&art)
        .can_focus(false)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["hero-peek"])
        .tooltip_text(&feature.row.name)
        .build();
    button.update_property(&[gtk::accessible::Property::Label(&format!(
        "{}: {}",
        if next {
            "Next featured"
        } else {
            "Previous featured"
        },
        feature.row.name
    ))]);
    let window = ui.window.clone();
    button.connect_clicked(move |_| {
        let target = if next { "hero-next" } else { "hero-previous" };
        if let Some(button) =
            named_widget(window.upcast_ref(), target).and_then(|w| w.downcast::<gtk::Button>().ok())
        {
            button.emit_clicked();
        }
    });
    button.upcast()
}

/// A title's primary action as a big art button, named `feature-<id>-primary`.
fn feature_button(ui: &Ui, row: &LibraryRow, child: &impl IsA<gtk::Widget>) -> gtk::Button {
    let (description, actions) = library_components(ui, row, false);
    let button = actions
        .last_child()
        .and_then(|w| w.downcast::<gtk::Button>().ok());
    let button = match button {
        Some(button) if row.action != Action::Stop => {
            actions.remove(&button);
            button
        }
        // A running title's card carries Resume and Stop; its banner opens
        // the library where they are.
        _ => {
            let button = gtk::Button::new();
            let ui = ui.clone();
            button.connect_clicked(move |_| select_section(&ui, Section::Installed));
            button
        }
    };
    button.set_widget_name(&format!("feature-{}-primary", row.product_id));
    button.set_child(Some(child));
    button.remove_css_class("suggested-action");
    button.remove_css_class("destructive-action");
    button.add_css_class("art-button");
    button.reset_relation(gtk::AccessibleRelation::LabelledBy);
    let action = if row.action == Action::Stop {
        "Running"
    } else {
        row.action.label()
    };
    button.update_property(&[
        gtk::accessible::Property::Label(&format!("{} · {action}", row.name)),
        gtk::accessible::Property::Description(&format!(
            "{action} · {}",
            description.subtitle().unwrap_or_default()
        )),
    ]);
    button.set_tooltip_text(Some(&row.name));
    button
}

/// A landscape tile for "Jump back in": the banner, the name, and what state
/// it is in. Named like a card (`game-<id>-primary`), because it is the
/// title's one card on Home.
fn landscape(ui: &Ui, row: &LibraryRow) -> gtk::Widget {
    let running = row.action == Action::Stop;
    let key = row.product_id.to_ascii_uppercase();
    // Only a banner on disk. A path to one never fetched (only featured
    // titles' are) was a blank tile in Jump back in, waiting on a slot
    // nothing fills; the cover's slot is filled once it arrives.
    let hero = ui.model.borrow().wide_art(&row.product_id);
    let cover = ui.model.borrow().icons.get(&key).cloned();
    let art = art_picture(hero.as_deref(), cover.as_deref(), textures::Size::Hero);
    if hero.is_some() {
        ui.art_slots
            .borrow_mut()
            .add_picture(format!("hero-{key}"), &art, textures::Size::Hero);
    } else {
        ui.art_slots
            .borrow_mut()
            .add_picture(key.clone(), &art, textures::Size::Cover);
    }
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&sized(&art, Some(320), 180)));
    let (state, class) = if running {
        let part = |p: &str| row.subtitle.split("  ·  ").any(|s| s == p);
        if part("starting") {
            ("Starting…", "ok")
        } else if part("stopping") {
            ("Stopping…", "ok")
        } else {
            ("● Running", "ok")
        }
    } else if row.has_update() {
        ("Update available", "update")
    } else if row.unavailable.is_some() {
        ("Unavailable", "warning")
    } else {
        match row.badge.map(|b| b.0) {
            Some("Issues") => ("Playable with issues", "warning"),
            Some("Menus only") => ("Menus only", "warning"),
            Some("Does not run") => ("Does not run", "error"),
            _ => ("Installed", "ok"),
        }
    };
    let caption = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .valign(gtk::Align::End)
        .css_classes(["tile-caption"])
        .build();
    caption.append(
        &gtk::Label::builder()
            .label(&row.name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(28)
            .css_classes(["tile-title"])
            .build(),
    );
    caption.append(
        &gtk::Label::builder()
            .label(state)
            .xalign(0.0)
            .halign(gtk::Align::Start)
            .css_classes(["tile-badge", class])
            .build(),
    );
    overlay.add_overlay(&caption);
    let (description, actions) = library_components(ui, row, false);
    if running {
        return running_landscape(row, &overlay, &actions);
    }
    let Some(primary) = actions
        .last_child()
        .and_then(|w| w.downcast::<gtk::Button>().ok())
    else {
        return game_card(ui, row).upcast();
    };
    actions.remove(&primary);
    primary.set_child(Some(&overlay));
    primary.remove_css_class("suggested-action");
    primary.remove_css_class("destructive-action");
    primary.add_css_class("art-button");
    primary.add_css_class("landscape-tile");
    primary.set_overflow(gtk::Overflow::Hidden);
    primary.reset_relation(gtk::AccessibleRelation::LabelledBy);
    primary.update_property(&[
        gtk::accessible::Property::Label(&format!("{} · {}", row.name, row.action.label())),
        gtk::accessible::Property::Description(&format!(
            "{} · {}",
            row.action.label(),
            description.subtitle().unwrap_or_default()
        )),
    ]);
    primary.set_tooltip_text(description.tooltip_text().as_deref());
    let holder = gtk::Overlay::new();
    holder.set_child(Some(&primary));
    if let Some(more) = actions.first_child() {
        actions.remove(&more);
        more.set_halign(gtk::Align::End);
        more.set_valign(gtk::Align::Start);
        more.set_margin_top(8);
        more.set_margin_end(8);
        more.add_css_class("game-more");
        holder.add_overlay(&more);
    }
    holder.upcast()
}

/// A running title's landscape tile: the banner with its Resume and Stop
/// laid over it, as separate controls (never buttons inside a button), and
/// its menu in the corner.
fn running_landscape(row: &LibraryRow, art: &gtk::Overlay, actions: &gtk::Box) -> gtk::Widget {
    let controls = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let mut more = None;
    let mut control = actions.first_child();
    while let Some(widget) = control {
        control = widget.next_sibling();
        if widget.is::<gtk::MenuButton>() {
            actions.remove(&widget);
            more = Some(widget);
        } else if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            actions.remove(&widget);
            widget.set_size_request(112, 40);
            widget.add_css_class("pill");
            widget.reset_relation(gtk::AccessibleRelation::LabelledBy);
            widget.update_property(&[gtk::accessible::Property::Label(&format!(
                "{} · {}",
                row.name,
                button.label().unwrap_or_default()
            ))]);
            controls.append(&widget);
        }
    }
    let layer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["game-launch-overlay"])
        .build();
    layer.set_accessible_role(gtk::AccessibleRole::Group);
    layer.update_property(&[gtk::accessible::Property::Label("Running overlay")]);
    controls.set_vexpand(true);
    layer.append(&controls);
    art.add_overlay(&layer);
    let holder = gtk::Overlay::new();
    holder.add_css_class("landscape-tile");
    holder.set_overflow(gtk::Overflow::Hidden);
    holder.set_widget_name(&format!("game-{}-primary", row.product_id));
    holder.set_child(Some(art));
    if let Some(more) = more {
        more.set_halign(gtk::Align::End);
        more.set_valign(gtk::Align::Start);
        more.set_margin_top(8);
        more.set_margin_end(8);
        more.add_css_class("game-more");
        holder.add_overlay(&more);
    }
    holder.upcast()
}

/// A shelf: a heading with "See all" and scroll buttons, over a row of tiles
/// that scrolls sideways and follows the focus.
pub(crate) fn shelf(
    ui: &Ui,
    page: &gtk::Box,
    title: &str,
    see_all: Option<Section>,
    tiles: Vec<gtk::Widget>,
) {
    if tiles.is_empty() {
        return;
    }
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .css_classes(["shelf-header"])
        .build();
    header.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["shelf-title"])
            .build(),
    );
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(16)
        .css_classes(["shelf-row"])
        .build();
    for tile in &tiles {
        // Art asks to expand; on a shelf a tile keeps its own width.
        tile.set_hexpand(false);
        tile.set_valign(gtk::Align::Start);
        row.append(tile);
    }
    let viewport = gtk::Viewport::builder()
        .scroll_to_focus(true)
        .child(&row)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&viewport)
        .css_classes(["shelf-scroller"])
        .build();
    // The wheel scrolls the page, not the shelf; sideways scrolling (a
    // touchpad, Shift+wheel) scrolls the shelf.
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    let outer = ui.scroll.vadjustment();
    let inner = scroller.hadjustment();
    wheel.connect_scroll(move |controller, dx, dy| {
        if dy.abs() <= dx.abs() {
            let step = if controller.unit() == gdk::ScrollUnit::Wheel {
                64.0
            } else {
                1.0
            };
            inner.set_value(inner.value() + dx * step);
            return glib::Propagation::Stop;
        }
        let step = if controller.unit() == gdk::ScrollUnit::Wheel {
            outer.page_size().powf(2.0 / 3.0)
        } else {
            1.0
        };
        let top = (outer.upper() - outer.page_size()).max(outer.lower());
        outer.set_value((outer.value() + dy * step).clamp(outer.lower(), top));
        glib::Propagation::Stop
    });
    scroller.add_controller(wheel);
    for (icon, label, step) in [
        ("go-previous-symbolic", "left", -1.0),
        ("go-next-symbolic", "right", 1.0),
    ] {
        let button = gtk::Button::from_icon_name(icon);
        let name = format!("Scroll {title} {label}");
        button.set_tooltip_text(Some(&name));
        button.update_property(&[gtk::accessible::Property::Label(&name)]);
        button.add_css_class("flat");
        button.add_css_class("circular");
        button.add_css_class("shelf-arrow");
        let adjustment = scroller.hadjustment();
        button.connect_clicked(move |_| {
            let top = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            let to = adjustment.value() + step * adjustment.page_size() * 0.8;
            adjustment.set_value(to.clamp(adjustment.lower(), top));
        });
        header.append(&button);
    }
    if let Some(section) = see_all {
        let button = gtk::Button::builder()
            .label("See all")
            .css_classes(["flat", "see-all"])
            .build();
        let name = format!("See all {title}");
        button.set_tooltip_text(Some(&name));
        button.update_property(&[gtk::accessible::Property::Label(&name)]);
        let ui = ui.clone();
        button.connect_clicked(move |_| select_section(&ui, section));
        header.append(&button);
    }
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .css_classes(["shelf"])
        .build();
    column.append(&header);
    column.append(&scroller);
    page.append(&column);
}

/// Outlined quick links under the hero.
fn pills(ui: &Ui, page: &gtk::Box, updates: usize) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::Center)
        .css_classes(["pill-row"])
        .build();
    let queued = ui.install_queue.borrow().jobs.len();
    let mut links = vec![
        ("All PC Game Pass games".to_string(), Section::GamePass),
        ("All your games".to_string(), Section::Library),
    ];
    if updates > 0 {
        links.push((format!("Updates ({updates})"), Section::Updates));
    }
    if queued > 0 {
        links.push((format!("Install queue ({queued})"), Section::Downloads));
    }
    for (label, section) in links {
        let button = gtk::Button::builder()
            .label(&label)
            .css_classes(["pill-outline"])
            .build();
        let ui = ui.clone();
        button.connect_clicked(move |_| select_section(&ui, section));
        row.append(&button);
    }
    page.append(&row);
}

/// Two-up landscape cards for Game Pass titles with banner art.
fn featured(ui: &Ui, page: &gtk::Box, rows: &[Feature]) {
    if rows.is_empty() {
        return;
    }
    page.append(
        &gtk::Label::builder()
            .label("Featured")
            .xalign(0.0)
            .css_classes(["shelf-title"])
            .build(),
    );
    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(2)
        .row_spacing(16)
        .column_spacing(16)
        .css_classes(["featured-grid"])
        .build();
    for (index, feature) in rows.iter().enumerate() {
        let art = art_picture(
            feature.art.as_deref(),
            feature.cover.as_deref(),
            textures::Size::Hero,
        );
        ui.art_slots.borrow_mut().add_picture(
            format!("hero-{}", feature.row.product_id.to_ascii_uppercase()),
            &art,
            textures::Size::Hero,
        );
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&sized(&art, None, 240)));
        let caption = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .valign(gtk::Align::End)
            .css_classes(["tile-caption", "featured-caption"])
            .build();
        caption.append(
            &gtk::Label::builder()
                .label(&feature.row.name)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["tile-title", "featured-title"])
                .build(),
        );
        caption.append(
            &gtk::Label::builder()
                .label(&feature.tagline)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["tile-subtitle"])
                .build(),
        );
        overlay.add_overlay(&caption);
        let button = feature_button(ui, &feature.row, &overlay);
        button.add_css_class("featured-card");
        button.set_overflow(gtk::Overflow::Hidden);
        grid.insert(&button, -1);
        if let Some(child) = grid.child_at_index(index as i32) {
            child.set_focusable(false);
        }
    }
    page.append(&grid);
}

/// Draw Home. False when Home should be the plain grid instead: a search is
/// running, or the window is fullscreen (the handheld layout).
pub(crate) fn render_home(ui: &Ui, page: &gtk::Box) -> bool {
    if ui.window.is_fullscreen() || !ui.model.borrow().query.trim().is_empty() {
        return false;
    }
    let downloading = ui
        .download
        .current
        .borrow()
        .as_ref()
        .map(|d| d.product_id.clone());
    let show_all = ui.model.borrow().show_unsupported;
    let keep = move |row: &LibraryRow| show_all || !row.known_uninstallable();
    let list = |section: Section| {
        draw_list(
            &ui.model.borrow(),
            &ui.running.borrow(),
            downloading.as_deref(),
            NameLookups {
                asked: &ui.asked_names.borrow(),
                fetching: ui.fetching_names.get(),
            },
            section,
            keep,
            Listing::All,
        )
    };
    let library = list(Section::Home);
    let gamepass = list(Section::GamePass);
    if !library.want_names.is_empty() {
        fetch_names(ui, library.want_names.clone());
    }
    if library.rows.is_empty() && gamepass.rows.is_empty() {
        empty(
            ui,
            page,
            "Nothing here yet",
            if ui.model.borrow().account.is_none() {
                "Sign in to see your library, or browse PC Game Pass."
            } else {
                "Nothing here yet."
            },
        );
        return true;
    }
    let mut seen = std::collections::HashSet::new();
    artwork::request_heroes(ui, hero_candidates(&library.rows, &gamepass.rows));
    let hero_items = features(&ui.model.borrow(), &library.rows, &gamepass.rows);
    for f in &hero_items {
        seen.insert(f.row.product_id.to_ascii_uppercase());
    }
    hero(ui, page, hero_items);
    let updates = library.rows.iter().filter(|r| r.has_update()).count();
    pills(ui, page, updates);

    let mut shown: Vec<LibraryRow> = Vec::new();
    let take = |rows: Vec<&LibraryRow>, shown: &mut Vec<LibraryRow>| -> Vec<LibraryRow> {
        let mut out = Vec::new();
        for row in rows {
            if out.len() >= SHELF {
                break;
            }
            if shown
                .iter()
                .any(|s: &LibraryRow| s.product_id.eq_ignore_ascii_case(&row.product_id))
            {
                continue;
            }
            shown.push(row.clone());
            out.push(row.clone());
        }
        out
    };
    let mut installed: Vec<&LibraryRow> = library.rows.iter().filter(|r| r.installed).collect();
    {
        let model = ui.model.borrow();
        installed.sort_by_key(|r| std::cmp::Reverse(model.last_played(&r.product_id)));
    }
    let jump = take(installed, &mut shown);
    let pass = take(
        gamepass.rows.iter().filter(|r| !r.installed).collect(),
        &mut shown,
    );
    let owned = take(
        library.rows.iter().filter(|r| !r.installed).collect(),
        &mut shown,
    );
    shelf(
        ui,
        page,
        "Jump back in",
        Some(Section::Installed),
        jump.iter().map(|r| landscape(ui, r)).collect(),
    );
    shelf(
        ui,
        page,
        "From PC Game Pass",
        Some(Section::GamePass),
        pass.iter().map(|r| game_card(ui, r).upcast()).collect(),
    );
    shelf(
        ui,
        page,
        "Your library",
        Some(Section::Library),
        owned.iter().map(|r| game_card(ui, r).upcast()).collect(),
    );
    // Featured: Game Pass titles with banners that the hero is not showing.
    let featured_rows: Vec<Feature> = {
        let model = ui.model.borrow();
        let cache = model.catalog_cache();
        gamepass
            .rows
            .iter()
            .filter(|r| !seen.contains(&r.product_id.to_ascii_uppercase()))
            .filter_map(|r| {
                let art = cache.as_ref().and_then(|c| c.hero_path(&r.product_id))?;
                Some(Feature {
                    row: r.clone(),
                    art: Some(art),
                    cover: None,
                    tagline: tagline(&model, r, true),
                })
            })
            .take(4)
            .collect()
    };
    featured(ui, page, &featured_rows);

    let visible: Vec<LibraryRow> = shown
        .into_iter()
        .chain(featured_rows.iter().map(|f| f.row.clone()))
        .collect();
    artwork::visible(ui, visible.iter().map(|r| r.product_id.clone()));
    fetch_disk_usage(ui, &visible);
    let want_icons: Vec<String> = {
        let model = ui.model.borrow();
        visible
            .iter()
            .filter(|r| !model.icons.contains_key(&r.product_id.to_ascii_uppercase()))
            .map(|r| r.product_id.clone())
            .collect()
    };
    if !want_icons.is_empty() {
        fetch_icons(ui, want_icons);
    }
    ui.showed_pending_names.set(false);
    true
}

/// An empty page: an icon, one sentence, and where to go instead.
pub(crate) fn empty(ui: &Ui, page: &gtk::Box, title: &str, description: &str) {
    let status = adw::StatusPage::builder()
        .icon_name("applications-games-symbolic")
        .title(title)
        .description(description)
        .css_classes(["compact"])
        .build();
    let button = gtk::Button::builder()
        .label("Browse PC Game Pass")
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    let ui = ui.clone();
    button.connect_clicked(move |_| select_section(&ui, Section::GamePass));
    status.set_child(Some(&button));
    page.append(&status);
}

/// The Game Pass page's hero: its titles with banner art, on the first page
/// of an unsearched list.
pub(crate) fn gamepass_hero(ui: &Ui, page: &gtk::Box) {
    {
        let model = ui.model.borrow();
        if ui.window.is_fullscreen() || !model.query.trim().is_empty() || model.page != 0 {
            return;
        }
    }
    let downloading = ui
        .download
        .current
        .borrow()
        .as_ref()
        .map(|d| d.product_id.clone());
    let show_all = ui.model.borrow().show_unsupported;
    let rows = draw_list(
        &ui.model.borrow(),
        &ui.running.borrow(),
        downloading.as_deref(),
        NameLookups {
            asked: &ui.asked_names.borrow(),
            fetching: ui.fetching_names.get(),
        },
        Section::GamePass,
        move |row| show_all || !row.known_uninstallable(),
        Listing::All,
    )
    .rows;
    artwork::request_heroes(ui, hero_candidates(&[], &rows));
    let items = features(&ui.model.borrow(), &[], &rows);
    hero(ui, page, items);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hero_turn_starts_and_ends_on_the_resting_places() {
        let (centre, peek, total) = hero_geometry(1000, true);
        let left = (1000 - total) / 2;
        assert_eq!(hero_place(1000, true, -1.0), (left, peek));
        assert_eq!(
            hero_place(1000, true, 0.0),
            (left + peek + HERO_GAP, centre)
        );
        assert_eq!(hero_place(1000, true, 1.0), (left + total - peek, peek));
        // Halfway it is between them, and nothing ever leaves the stage.
        let (x, w) = hero_place(1000, true, 0.5);
        assert!(x > left + peek + HERO_GAP && w < centre && w > peek);
        for place in -30..=30 {
            let (x, w) = hero_place(1000, true, f64::from(place) / 10.0);
            assert!(x >= left && x + w <= left + total + 1, "{place}");
        }
        assert_eq!(hero_place(1000, false, 0.0), (0, 1000));
    }
}

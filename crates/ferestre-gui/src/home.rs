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
const SHELF: usize = 20;
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

fn features(model: &Model, library: &[LibraryRow], gamepass: &[LibraryRow]) -> Vec<Feature> {
    let cache = model.catalog_cache();
    let hero = |id: &str| cache.as_ref().and_then(|c| c.hero_path(id));
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
        if need_art && art.is_none() {
            return;
        }
        out.push(Feature {
            row: row.clone(),
            cover: model
                .icons
                .get(&row.product_id.to_ascii_uppercase())
                .cloned(),
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

/// The hero carousel, for Home and Game Pass.
pub(crate) fn hero(ui: &Ui, page: &gtk::Box, items: Vec<Feature>) {
    if items.is_empty() {
        return;
    }
    let items = Rc::new(items);
    let height = (f64::from(ui.window.width()) * 0.24).clamp(280.0, 440.0) as i32;
    let index = ui.hero_index.get() % items.len();
    ui.hero_index.set(index);
    let stage = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(16)
        .halign(gtk::Align::Center)
        .css_classes(["hero-stage"])
        .build();
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
    let fill: Rc<dyn Fn(&Ui)> = {
        let (stage, dots, items) = (stage.clone(), dots.clone(), items.clone());
        Rc::new(move |ui: &Ui| {
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
            if n > 1 {
                stage.append(&peek(ui, &items[(index + n - 1) % n], false, height));
            }
            let centre = centre(ui, &items[index], index == 0, height);
            stage.append(&centre);
            if n > 1 {
                stage.append(&peek(ui, &items[(index + 1) % n], true, height));
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
    fill(ui);
    for (button, step) in [(previous, -1isize), (next, 1)] {
        let (ui, fill, n) = (ui.clone(), fill.clone(), items.len());
        button.connect_clicked(move |_| {
            let index = (ui.hero_index.get() as isize + step).rem_euclid(n as isize);
            ui.hero_index.set(index as usize);
            fill(&ui);
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

/// How wide the centre banner and each peeking neighbour are: the centre at
/// most 1400 px, the neighbours at least a tenth of the content each, and on
/// a very wide window the neighbours take what the centre leaves.
fn hero_widths(ui: &Ui) -> (i32, i32) {
    let sidebar = if ui.split.is_collapsed() {
        0
    } else if ui.compact.get() {
        72
    } else {
        270
    };
    let margin = if ui.window.width() > 2560 { 48 } else { 32 };
    let content = f64::from((ui.window.width() - sidebar - 2 * margin).max(320));
    let gaps = 32.0;
    let centre = 1400f64.min(content - 2.0 * (content * 0.1).max(56.0) - gaps);
    let peek = ((content - centre - gaps) / 2.0).max(56.0);
    (centre.max(200.0) as i32, peek as i32)
}

/// The centre banner: one button, the title's main action.
fn centre(ui: &Ui, feature: &Feature, first: bool, height: i32) -> gtk::Widget {
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
    overlay.set_child(Some(&sized(&art, None, height)));
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
    if first {
        button.add_css_class("hero-first");
    }
    button.set_hexpand(true);
    let (centre_width, _) = hero_widths(ui);
    let clamp = adw::Clamp::builder()
        .maximum_size(centre_width)
        .tightening_threshold(centre_width)
        .hexpand(true)
        .child(&button)
        .build();
    clamp.set_size_request(-1, 0);
    clamp.upcast()
}

/// A dimmed neighbour; clicking it turns the carousel. Never focused: the
/// controller turns it with Left and Right on the centre instead.
fn peek(ui: &Ui, feature: &Feature, next: bool, height: i32) -> gtk::Widget {
    let art = art_picture(
        feature.art.as_deref(),
        feature.cover.as_deref(),
        textures::Size::Hero,
    );
    // A sliver of the neighbour, wider on a wide window.
    let (_, width) = hero_widths(ui);
    let button = gtk::Button::builder()
        .child(&sized(&art, Some(width), height))
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
    if row.action == Action::Stop {
        // Resume and Stop are two controls; the portrait card draws them.
        return game_card(ui, row).upcast();
    }
    let key = row.product_id.to_ascii_uppercase();
    let hero = ui
        .model
        .borrow()
        .catalog_cache()
        .and_then(|c| c.hero_path(&row.product_id));
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
    let (state, class) = if row.has_update() {
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
    let items = features(&ui.model.borrow(), &[], &rows);
    hero(ui, page, items);
}

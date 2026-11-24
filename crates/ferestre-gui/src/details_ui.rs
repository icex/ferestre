//! One game's details: what the window knows, and what Microsoft's catalog says.
use super::*;
use ferestre_core::product_details::{self, AgeRating, Details, PlayerRating, Requirement, Row};

/// What a large button does, for a title with this id and name: a bundle's
/// is the game in it.
#[derive(Clone)]
pub enum Main {
    Launch(String, String),
    Install(String, String),
    Resume(String, String),
    Uninstall(String, String),
    Update(String, String),
    /// Describe how it starts.
    SetUp(String, String),
    /// Say why it cannot start, and whether another runtime may help.
    Blocked(String, String, String, bool),
    /// Back to the running game.
    Return(String, String),
    Stop(String, String),
}

impl Main {
    /// What there is to do for a row, the likeliest first.
    pub fn of(ui: &Ui, row: &LibraryRow) -> Vec<Main> {
        let (id, name) = row
            .install_as
            .clone()
            .unwrap_or_else(|| (row.product_id.clone(), row.name.clone()));
        match row.action {
            Action::Install if !row.local => return vec![Main::Install(id, name)],
            Action::Resume if !row.local => return vec![Main::Resume(id, name)],
            Action::Stop => {
                // A bundle row runs its child, under the child's id.
                let running = ui.running.borrow();
                let own = running.get(&row.product_id.to_ascii_uppercase());
                let id = match own {
                    Some(_) => row.product_id.clone(),
                    None => id,
                };
                let ready = running
                    .get(&id.to_ascii_uppercase())
                    .is_some_and(|launch| launch.ready);
                let mut all = Vec::new();
                if ready {
                    all.push(Main::Return(id.clone(), row.name.clone()));
                }
                all.push(Main::Stop(id, row.name.clone()));
                return all;
            }
            Action::Installing => return Vec::new(),
            _ => {}
        }
        let mut all = Vec::new();
        if matches!(row.action, Action::Play | Action::Update) {
            all.push(Main::Launch(id.clone(), name.clone()));
        }
        match &row.action {
            Action::Update => all.push(Main::Update(id, name)),
            Action::Adopt => all.push(Main::SetUp(row.product_id.clone(), row.name.clone())),
            Action::Blocked(reason) if row.has_recipe && !row.unsupported => {
                all.push(Main::Blocked(
                    row.product_id.clone(),
                    row.name.clone(),
                    reason.clone(),
                    row.blocked_by_runtime(),
                ));
            }
            _ => {}
        }
        // As the menu's Delete: what is on disk is the row's own.
        if row.installed && !row.local {
            all.push(Main::Uninstall(row.product_id.clone(), row.name.clone()));
        }
        all
    }

    fn label(&self) -> &'static str {
        match self {
            Main::Launch(..) => "Launch",
            Main::Install(..) => "Install",
            Main::Resume(..) => "Resume install",
            Main::Uninstall(..) => "Uninstall",
            Main::Update(..) => "Update",
            Main::SetUp(..) => "Set up",
            Main::Blocked(..) => "Launch",
            Main::Return(..) => "Resume",
            Main::Stop(..) => "Stop",
        }
    }

    fn button(&self, ui: &Ui, dialog: &adw::Dialog) -> gtk::Button {
        let (icons, class, name) = match self {
            Main::Launch(_, name) | Main::Blocked(_, name, ..) | Main::Return(_, name) => (
                [
                    "media-playback-start-symbolic",
                    "media-playback-start-symbolic",
                ],
                "suggested-action",
                name,
            ),
            Main::Install(_, name) | Main::Resume(_, name) => (
                ["folder-download-symbolic", "document-save-symbolic"],
                "suggested-action",
                name,
            ),
            Main::Update(_, name) => (
                [
                    "software-update-available-symbolic",
                    "folder-download-symbolic",
                ],
                "update-action",
                name,
            ),
            Main::SetUp(_, name) => (
                ["document-edit-symbolic", "document-properties-symbolic"],
                "suggested-action",
                name,
            ),
            Main::Stop(_, name) => (
                ["media-playback-stop-symbolic", "process-stop-symbolic"],
                "destructive-action",
                name,
            ),
            Main::Uninstall(_, name) => (
                ["user-trash-symbolic", "edit-delete-symbolic"],
                "destructive-action",
                name,
            ),
        };
        // Not an adw::ButtonContent: that names the button after its label.
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.set_halign(gtk::Align::Center);
        content.append(&icon(&icons, 18));
        content.append(&gtk::Label::new(Some(self.label())));
        let button = gtk::Button::builder()
            .child(&content)
            .width_request(220)
            .height_request(54)
            .css_classes(["pill", "details-main", class])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&format!(
            "{} {name}",
            self.label()
        ))]);
        let (ui, dialog, main) = (ui.clone(), dialog.downgrade(), self.clone());
        button.connect_clicked(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            match &main {
                Main::Launch(id, name) => spawn_title(&ui, id, name),
                Main::Install(id, name) => confirm_install(&ui, id, name),
                Main::Resume(id, name) => resume_download(&ui, id, name),
                Main::Uninstall(id, name) => confirm_remove(&ui, id, name),
                Main::Update(id, name) => update_download(&ui, id, name),
                Main::SetUp(id, name) => open_editor(&ui, id, name),
                Main::Blocked(id, name, reason, runtime) => {
                    explain_blocked(&ui, id, name, reason, *runtime)
                }
                Main::Return(id, name) => window_activation::resume(&ui, id, name),
                Main::Stop(id, name) => confirm_stop_title(&ui, id, name),
            }
        });
        button
    }
}

/// `known` is what the window knows already. A local game has no Store page,
/// and its id is not sent anywhere.
pub fn open(
    ui: &Ui,
    product_id: &str,
    name: &str,
    known: &str,
    report: &compat::Report,
    local: bool,
    main: &[Main],
) {
    let dialog = adw::Dialog::builder()
        .title(name)
        .content_width(900)
        .content_height(760)
        .build();
    let stack = adw::ViewStack::builder()
        .vexpand(true)
        // Only the tab shown is measured.
        .hhomogeneous(false)
        .vhomogeneous(false)
        .build();
    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Narrow)
        // Room for the focus outline, which the scroller would cut.
        .margin_top(10)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(10)
        .build();
    let tabs = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&switcher)
        // One tab is nothing to choose from.
        .visible(false)
        .build();

    let overview = page(&stack, "Overview", &["view-grid-symbolic"]);
    let top = gtk::Box::new(gtk::Orientation::Vertical, 0);
    overview.append(&top);
    let head = gtk::Box::new(gtk::Orientation::Vertical, 6);
    head.set_hexpand(true);
    head.append(&label(name, &["title-1"]));
    let makers = label("", &["dim-label"]);
    makers.set_visible(false);
    head.append(&makers);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    title.append(&head);
    overview.append(&title);

    // "v1 · 2 GB · Playable", then a paragraph about the runtime.
    let (state, runtime) = known.split_once("\n\n").unwrap_or((known, ""));
    let chips: Vec<String> = state.split("  ·  ").map(str::to_string).collect();
    overview.append(&chip_box(&chips));
    if !runtime.is_empty() {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&icon(&["applications-engineering-symbolic"], 16));
        line.append(&label(runtime, &["caption", "dim-label"]));
        overview.append(&line);
    }
    overview.append(&compat::section(report));
    let mut page_parts = None;
    if !local {
        let content = adw::ButtonContent::builder()
            .icon_name(icon_name(&[
                "adw-external-link-symbolic",
                "web-browser-symbolic",
            ]))
            .label("Open Store page")
            .build();
        let store = gtk::Button::builder()
            .child(&content)
            .valign(gtk::Align::Center)
            .build();
        store.update_property(&[gtk::accessible::Property::Label("Open Store page")]);
        let url = format!("{STORE_PAGE}{product_id}");
        store.connect_clicked(move |button| open_link(button, &url));
        title.append(&store);
        let shown = Page {
            stack: stack.clone(),
            tabs: tabs.clone(),
            overview,
            top,
            head,
            makers,
            bones: RefCell::default(),
        };
        page_parts = Some(shown);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.add_top_bar(&tabs);
    view.set_content(Some(&stack));
    dialog.set_child(Some(&view));
    // Under every tab, and where the focus starts.
    if !main.is_empty() {
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(14)
            .homogeneous(true)
            .halign(gtk::Align::Center)
            .margin_top(10)
            .margin_bottom(14)
            .margin_start(14)
            .margin_end(14)
            .build();
        for (index, what) in main.iter().enumerate() {
            let button = what.button(ui, &dialog);
            bar.append(&button);
            // Never what takes something away.
            if index == 0 && !matches!(what, Main::Uninstall(..) | Main::Stop(..)) {
                dialog.set_default_widget(Some(&button));
                dialog.set_focus(Some(&button));
            }
        }
        view.add_bottom_bar(&bar);
    }
    if let Some(parts) = &page_parts {
        parts.wait();
    }
    dialog.present(Some(&ui.window));
    // The dialog first: the answer may come at once, and drawing it must not
    // hold the dialog back.
    if let Some(parts) = page_parts {
        let (ui, product_id) = (ui.clone(), product_id.to_string());
        glib::timeout_add_local_once(OPENING, move || load(&ui, &product_id, parts));
    }
}

/// The parts the catalog's answer fills in.
struct Page {
    stack: adw::ViewStack,
    tabs: gtk::ScrolledWindow,
    overview: gtk::Box,
    /// Above the name, for the pictures.
    top: gtk::Box,
    head: gtk::Box,
    makers: gtk::Label,
    /// What stands where the answer goes, and what it is in.
    bones: RefCell<Vec<(gtk::Box, gtk::Box)>>,
}

impl Page {
    fn wait(&self) {
        let put = |parent: &gtk::Box, bone: gtk::Box| {
            parent.append(&bone);
            self.bones.borrow_mut().push((parent.clone(), bone));
        };
        put(&self.overview, skeleton());
        put(&self.top, bone(-1, PICTURE_HEIGHT));
        put(&self.head, bone(220, 14));
    }
}

/// The first of `names` the icon theme has.
fn icon_name(names: &[&'static str]) -> &'static str {
    let theme = gdk::Display::default().map(|d| gtk::IconTheme::for_display(&d));
    names
        .iter()
        .copied()
        .find(|name| theme.as_ref().is_some_and(|t| t.has_icon(name)))
        .unwrap_or("dialog-information-symbolic")
}

fn icon(names: &[&'static str], size: i32) -> gtk::Image {
    gtk::Image::builder()
        .icon_name(icon_name(names))
        .pixel_size(size)
        .valign(gtk::Align::Center)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build()
}

/// Add a tab and return the box its content goes in.
fn page(stack: &adw::ViewStack, title: &str, icons: &[&'static str]) -> gtk::Box {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(18)
        .margin_bottom(30)
        .margin_start(24)
        .margin_end(24)
        .build();
    let clamp = adw::Clamp::builder()
        .maximum_size(840)
        .tightening_threshold(700)
        .child(&content)
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    stack.add_titled_with_icon(&scroll, Some(title), title, icon_name(icons));
    content
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .css_classes(classes.to_vec())
        .build()
}

fn heading(text: &str) -> gtk::Label {
    let heading = label(text, &["title-4"]);
    heading.set_margin_top(6);
    heading
}

/// Text to read and copy.
fn body(text: &str) -> gtk::Label {
    let body = label(text, &["details-body"]);
    body.set_selectable(true);
    body.set_focusable(false);
    body
}

/// Children side by side, as many as fit, none of them something to select.
fn flow(children: impl IntoIterator<Item = gtk::Widget>, most: u32, same: bool) -> gtk::FlowBox {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(most)
        .homogeneous(same)
        .column_spacing(if same { 12 } else { 6 })
        .row_spacing(if same { 12 } else { 6 })
        .css_classes(["details-flow"])
        .build();
    for child in children {
        let cell = gtk::FlowBoxChild::builder()
            .child(&child)
            .focusable(false)
            .build();
        flow.append(&cell);
    }
    flow
}

fn chip_box(texts: &[String]) -> gtk::FlowBox {
    let chips = texts.iter().filter(|t| !t.is_empty()).map(|t| {
        let chip = label(t, &["details-chip", "caption"]);
        chip.set_xalign(0.5);
        chip.upcast()
    });
    let chips = flow(chips, 30, false);
    chips.set_halign(gtk::Align::Fill);
    chips
}

fn card(orientation: gtk::Orientation, spacing: i32) -> gtk::Box {
    gtk::Box::builder()
        .orientation(orientation)
        .spacing(spacing)
        .css_classes(["details-card"])
        .build()
}

/// An icon beside a small label over its value.
fn fact(icons: &[&'static str], what: &str, value: &str) -> gtk::Widget {
    let fact = card(gtk::Orientation::Horizontal, 14);
    fact.append(&icon(icons, 20));
    let words = gtk::Box::new(gtk::Orientation::Vertical, 2);
    words.append(&label(what, &["caption", "dim-label"]));
    words.append(&label(value, &["heading"]));
    fact.append(&words);
    fact.upcast()
}

fn player_card(rating: &PlayerRating) -> gtk::Widget {
    let all = card(gtk::Orientation::Vertical, 4);
    all.append(&label(&rating.span, &["caption", "dim-label"]));
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    line.append(&label(
        &format!("{:.1}", rating.average),
        &["details-figure"],
    ));
    let stars = label(&stars(rating.average), &["details-stars"]);
    stars.set_wrap(false);
    stars.update_property(&[gtk::accessible::Property::Label(&format!(
        "{:.1} of 5",
        rating.average
    ))]);
    line.append(&stars);
    all.append(&line);
    let noun = if rating.count == 1 {
        "rating"
    } else {
        "ratings"
    };
    all.append(&label(
        &format!("{} {noun}", thousands(rating.count)),
        &["caption", "dim-label"],
    ));
    all.upcast()
}

fn stars(average: f64) -> String {
    let full = average.round().clamp(0.0, 5.0) as usize;
    "★".repeat(full) + &"☆".repeat(5 - full)
}

fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(digit);
    }
    out
}

fn feature_icons(name: &str) -> &'static [&'static str] {
    match name {
        "SinglePlayer" => &["avatar-default-symbolic"],
        "XblAchievements" => &["starred-symbolic"],
        "XblCloudSaves" => &["weather-overcast-symbolic", "folder-remote-symbolic"],
        "Capability4k" | "CapabilityHDR" => &["video-display-symbolic"],
        "RayTracing" => &["weather-clear-symbolic", "display-brightness-symbolic"],
        "DolbyAtmos" => &["audio-speakers-symbolic", "audio-volume-high-symbolic"],
        "BroadcastSupport" => &["camera-video-symbolic", "media-record-symbolic"],
        "VREnabled" | "MixedRealitySeated" => &["view-reveal-symbolic"],
        "XboxLive" | "XblCrossPlatformMultiPlayer" | "XblCrossPlatformCoop" => {
            &["network-workgroup-symbolic"]
        }
        _ if name.contains("Coop") || name.contains("MultiPlayer") || name.contains("Split") => {
            &["system-users-symbolic"]
        }
        _ if name.contains("Console") || name == "XPA" => &["input-gaming-symbolic"],
        _ => &["object-select-symbolic"],
    }
}

fn row_icons(what: &str) -> &'static [&'static str] {
    match what {
        "Released" | "Last update" | "Store entry changed" => &["x-office-calendar-symbolic"],
        "Add-ons" => &["list-add-symbolic"],
        "In-app purchases" => &["emblem-important-symbolic"],
        "Optimized for consoles" | "Plays on consoles" => &["input-gaming-symbolic"],
        "Processor" => &["applications-engineering-symbolic"],
        "Graphics" | "Video memory" => &["video-display-symbolic"],
        "Memory" => &["media-flash-symbolic", "drive-harddisk-solidstate-symbolic"],
        "Input" => &["input-keyboard-symbolic"],
        "Download size" => &["folder-download-symbolic", "drive-harddisk-symbolic"],
        "Format" | "Package family" => &["package-x-generic-symbolic"],
        "Architecture" => &["applications-engineering-symbolic"],
        "Type" => &["input-gaming-symbolic"],
        "Website" => &["web-browser-symbolic"],
        "Support" => &["help-browser-symbolic", "help-about-symbolic"],
        "Privacy policy" => &["security-high-symbolic", "channel-secure-symbolic"],
        "Terms of use" | "Licence terms" => &["text-x-generic-symbolic"],
        _ => &["dialog-information-symbolic"],
    }
}

fn facts(rows: &[Row], most: u32) -> gtk::FlowBox {
    flow(
        rows.iter()
            .map(|(what, value)| fact(row_icons(what), what, value)),
        most,
        true,
    )
}

fn feature_tile(name: &str) -> gtk::Widget {
    let tile = card(gtk::Orientation::Horizontal, 14);
    tile.append(&icon(feature_icons(name), 20));
    tile.append(&label(&product_details::feature(name), &["heading"]));
    tile.upcast()
}

fn requirement_card(need: &Requirement) -> gtk::Box {
    let all = card(gtk::Orientation::Vertical, 12);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    title.append(&icon(row_icons(need.what), 20));
    title.append(&label(need.what, &["heading"]));
    all.append(&title);
    let columns = gtk::Box::builder().spacing(24).homogeneous(true).build();
    for (what, value) in [
        ("Minimum", &need.minimum),
        ("Recommended", &need.recommended),
    ] {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        column.append(&label(what, &["caption", "dim-label"]));
        column.append(&label(
            if value.is_empty() {
                "Not stated"
            } else {
                value
            },
            if value.is_empty() {
                &["dim-label"]
            } else {
                &[]
            },
        ));
        columns.append(&column);
    }
    all.append(&columns);
    all
}

fn age_card(age: &AgeRating) -> gtk::Widget {
    let all = card(gtk::Orientation::Horizontal, 16);
    let badge = gtk::Label::builder()
        .label(&age.rating)
        .width_chars(4)
        .valign(gtk::Align::Start)
        .css_classes(["details-badge"])
        .build();
    all.append(&badge);
    let words = gtk::Box::new(gtk::Orientation::Vertical, 4);
    words.set_hexpand(true);
    words.append(&label(&age.system, &["heading"]));
    let region = product_details::rating_region(&age.system);
    if !region.is_empty() {
        words.append(&label(region, &["caption"]));
    }
    if !age.descriptors.is_empty() {
        let reasons = label(&age.descriptors.join(" · "), &["caption", "dim-label"]);
        // Two cards to a line: the reasons wrap rather than widen their card.
        reasons.set_width_chars(20);
        reasons.set_max_width_chars(32);
        words.append(&reasons);
    }
    all.append(&words);
    all.upcast()
}

fn link_list(links: &[Row]) -> adw::PreferencesGroup {
    let list = adw::PreferencesGroup::new();
    for (what, value) in links {
        let row = plain_row(what, Some(value));
        row.add_prefix(&icon(row_icons(what), 20));
        if let Some(url) = product_details::link(value) {
            // The address shown is the one opened, without its scheme.
            let shown = url.split_once("://").map_or(url, |(_, rest)| rest);
            row.set_subtitle(shown.trim_end_matches('/'));
            let open = gtk::Button::builder()
                .icon_name(icon_name(&[
                    "adw-external-link-symbolic",
                    "go-next-symbolic",
                ]))
                .tooltip_text(url)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            open.update_property(&[gtk::accessible::Property::Label(&format!("Open {what}"))]);
            let url = url.to_string();
            open.connect_clicked(move |button| open_link(button, &url));
            row.add_suffix(&open);
            row.set_activatable_widget(Some(&open));
        } else {
            row.set_subtitle_selectable(true);
        }
        list.add(&row);
    }
    list
}

/// The banner and screenshots, one at a time. Each is fetched in turn while
/// the page is open; one that cannot be fetched is left out.
fn carousel(ui: &Ui, urls: Vec<String>) -> Option<gtk::Box> {
    let paths = ui.model.borrow().paths.clone()?;
    let dir = paths.cache_dir().join("catalog").join("pictures");
    if urls.is_empty() {
        return None;
    }
    let carousel = adw::Carousel::builder()
        .height_request(PICTURE_HEIGHT)
        .hexpand(true)
        // The wheel scrolls the page, which is longer than its pictures.
        .allow_scroll_wheel(false)
        .build();
    let turn = |icon: &str, label: &str, align: gtk::Align, step: i32| {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(label)
            .halign(align)
            .valign(gtk::Align::Center)
            .margin_start(8)
            .margin_end(8)
            .css_classes(["osd", "circular"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        let carousel = carousel.downgrade();
        button.connect_clicked(move |_| {
            let Some(carousel) = carousel.upgrade() else {
                return;
            };
            let count = carousel.n_pages() as i32;
            if count > 0 {
                let next = (carousel.position().round() as i32 + step).rem_euclid(count);
                carousel.scroll_to(&carousel.nth_page(next as u32), true);
            }
        });
        button
    };
    let stage = gtk::Overlay::builder()
        .child(&carousel)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["details-stage"])
        .build();
    stage.add_overlay(&turn(
        "go-previous-symbolic",
        "Previous picture",
        gtk::Align::Start,
        -1,
    ));
    stage.add_overlay(&turn(
        "go-next-symbolic",
        "Next picture",
        gtk::Align::End,
        1,
    ));
    let dots = adw::CarouselIndicatorDots::new();
    dots.set_carousel(Some(&carousel));
    let all = gtk::Box::new(gtk::Orientation::Vertical, 6);
    all.append(&stage);
    all.append(&dots);
    all.set_visible(false);
    let shown = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let waiting = bone(-1, PICTURE_HEIGHT);
    shown.append(&waiting);
    shown.append(&all);

    let (weak_all, weak) = (all.downgrade(), carousel.downgrade());
    glib::spawn_future_local(async move {
        for url in urls {
            let dir = dir.clone();
            let got = off_thread(move || product_details::picture(&dir, &url)).await;
            let (Some(all), Some(carousel)) = (weak_all.upgrade(), weak.upgrade()) else {
                return;
            };
            let Some(Ok(path)) = got else { continue };
            let picture = gtk::Picture::builder()
                .can_shrink(true)
                .content_fit(gtk::ContentFit::Cover)
                .hexpand(true)
                .build();
            picture.set_alternative_text(Some("Picture from the game’s Store page"));
            textures::set_picture(&picture, &path, textures::Size::Hero);
            carousel.append(&picture);
            // Until the first one is decoded, not only fetched.
            let reveal = {
                let (all, waiting) = (all.downgrade(), waiting.downgrade());
                move |picture: &gtk::Picture| {
                    if picture.paintable().is_none() {
                        return;
                    }
                    if let (Some(all), Some(waiting)) = (all.upgrade(), waiting.upgrade()) {
                        waiting.set_visible(false);
                        all.set_visible(true);
                    }
                }
            };
            picture.connect_paintable_notify(reveal.clone());
            reveal(&picture);
        }
        // None could be fetched.
        if weak.upgrade().is_some_and(|c| c.n_pages() == 0) {
            waiting.set_visible(false);
        }
    });
    Some(shown)
}

const PICTURE_HEIGHT: i32 = 400;

/// A grey block where something is about to be.
fn bone(width: i32, height: i32) -> gtk::Box {
    let bone = gtk::Box::builder()
        .width_request(width)
        .height_request(height)
        .css_classes(["skeleton"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    if width > 0 {
        bone.set_halign(gtk::Align::Start);
    } else {
        bone.set_hexpand(true);
    }
    bone
}

/// The shape of the overview below its name, until the catalog answers.
fn skeleton() -> gtk::Box {
    let all = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .accessible_role(gtk::AccessibleRole::Status)
        .build();
    all.update_property(&[gtk::accessible::Property::Label("Loading game details")]);
    all.append(&bone(190, 22));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 10);
    for end in [0, 40, 14, 260] {
        let line = bone(-1, 14);
        line.set_margin_end(end);
        text.append(&line);
    }
    all.append(&text);
    all.append(&bone(150, 22));
    let cards = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    cards.set_homogeneous(true);
    for _ in 0..3 {
        cards.append(&bone(-1, 84));
    }
    all.append(&cards);
    all
}

/// libadwaita's opening animation.
const OPENING: std::time::Duration = std::time::Duration::from_millis(260);

/// On a thread of its own: the shared pool may be busy with downloads.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (send, receive) = futures_channel::oneshot::channel();
    std::thread::Builder::new()
        .name("details".into())
        .spawn(move || {
            let _ = send.send(work());
        })
        .ok()?;
    receive.await.ok()
}

/// Let the frames in between be drawn.
async fn a_frame() {
    glib::timeout_future(std::time::Duration::from_millis(18)).await;
}

fn load(ui: &Ui, product_id: &str, shown: Page) {
    let (market, language) = ui.model.borrow().market();
    let ui = ui.clone();
    let product_id = product_id.to_string();
    glib::spawn_future_local(async move {
        let result = off_thread(move || {
            product_details::fetch(&product_id, &market, &language).map_err(|e| format!("{e:#}"))
        })
        .await
        .unwrap_or_else(|| Err("the request stopped".into()));
        // Closed meanwhile.
        if *ui.closing.borrow() || shown.stack.root().is_none() {
            return;
        }
        for (parent, bone) in shown.bones.take() {
            parent.remove(&bone);
        }
        match result {
            Ok(details) => {
                fill(&ui, &shown, details).await;
            }
            Err(error) => {
                let why = format!("Microsoft’s catalog could not be read: {error}");
                shown.overview.append(&label(&why, &["dim-label"]));
            }
        }
    });
}

/// A part at a time, with frames between them.
async fn fill(ui: &Ui, shown: &Page, details: Details) {
    let Page {
        stack, overview, ..
    } = shown;
    let mut makers = vec![details.developer.clone()];
    if details.publisher != details.developer {
        makers.push(details.publisher.clone());
    }
    makers.retain(|m| !m.is_empty());
    shown.makers.set_label(&makers.join("  ·  "));
    shown.makers.set_visible(!makers.is_empty());

    if let Some(pictures) = carousel(ui, details.pictures) {
        shown.top.append(&pictures);
    }
    if !details.categories.is_empty() {
        let chips = chip_box(&details.categories);
        chips.set_margin_top(4);
        shown.head.append(&chips);
    }
    if !details.description.is_empty() {
        overview.append(&heading("About this game"));
        overview.append(&body(&details.description));
    }
    if !details.players.is_empty() {
        overview.append(&heading("Player ratings"));
        overview.append(&flow(details.players.iter().map(player_card), 3, true));
    }
    if !details.dates.is_empty() {
        overview.append(&heading("Dates"));
        overview.append(&facts(&details.dates, 3));
    }
    if !details.release_notes.is_empty() {
        overview.append(&heading("Release notes"));
        overview.append(&body(&details.release_notes));
    }

    a_frame().await;
    let tab = |title: &str, icons: &[&'static str]| {
        shown.tabs.set_visible(true);
        page(stack, title, icons)
    };
    if !details.features.is_empty() || !details.purchases.is_empty() {
        let content = tab("Features", &["starred-symbolic"]);
        if !details.features.is_empty() {
            content.append(&heading("What it supports"));
            content.append(&flow(
                details.features.iter().map(|f| feature_tile(f)),
                3,
                true,
            ));
        }
        if !details.purchases.is_empty() {
            content.append(&heading("Purchases and consoles"));
            content.append(&facts(&details.purchases, 2));
        }
    }
    a_frame().await;
    if !details.requirements.is_empty() || !details.requirement_notes.is_empty() {
        let content = tab("Requirements", &["computer-symbolic"]);
        content.append(&heading("System requirements"));
        content.append(&label(
            "What the publisher lists for Windows.",
            &["dim-label"],
        ));
        for need in &details.requirements {
            content.append(&requirement_card(need));
        }
        for (what, value) in &details.requirement_notes {
            content.append(&heading(&format!("{what} notes")));
            content.append(&body(value));
        }
    }
    a_frame().await;
    if !details.ages.is_empty() {
        let content = tab("Age ratings", &["system-users-symbolic"]);
        content.append(&heading("Age ratings"));
        content.append(&flow(details.ages.iter().map(age_card), 2, true));
    }
    a_frame().await;
    if !details.package.is_empty() || !details.languages.is_empty() {
        let content = tab("Package", &["package-x-generic-symbolic"]);
        // Ids are long, and for copying; the rest reads better as cards.
        let (ids, short): (Vec<Row>, Vec<Row>) = details
            .package
            .into_iter()
            .partition(|(what, _)| what.ends_with(" id") || *what == "Package family");
        if !short.is_empty() {
            content.append(&heading("Package"));
            content.append(&facts(&short, 2));
        }
        if !details.languages.is_empty() {
            content.append(&heading("Languages"));
            content.append(&chip_box(&details.languages));
        }
        if !ids.is_empty() {
            let list = adw::PreferencesGroup::builder()
                .title("Identifiers")
                .build();
            for (what, value) in &ids {
                let row = plain_row(what, Some(value));
                row.add_css_class("property");
                row.add_css_class("details-id");
                row.set_subtitle_selectable(true);
                list.add(&row);
            }
            content.append(&list);
        }
    }
    a_frame().await;
    if !details.links.is_empty() || !details.copyright.is_empty() {
        let content = tab(
            "Links and legal",
            &["adw-external-link-symbolic", "web-browser-symbolic"],
        );
        if !details.links.is_empty() {
            content.append(&heading("Links"));
            content.append(&link_list(&details.links));
        }
        if !details.copyright.is_empty() {
            content.append(&label(&details.copyright, &["caption", "dim-label"]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratings_read_as_people_write_them() {
        assert_eq!(stars(4.8), "★★★★★");
        assert_eq!(stars(3.4), "★★★☆☆");
        assert_eq!(stars(9.0), "★★★★★");
        assert_eq!(thousands(190), "190");
        assert_eq!(thousands(2_252_544), "2\u{202f}252\u{202f}544");
    }
}

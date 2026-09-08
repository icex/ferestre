//! A window over the launcher.
//!
//! Deliberately thin, and deliberately dumb: every decision -- which recipe
//! applies, whether the runtime can satisfy it, what a refusal should say --
//! comes from [`model`], which is tested without a display, and from
//! `ferestre-core`, which the CLI uses too. Actions are performed by spawning the
//! `ferestre` binary rather than reimplementing them here, so the window cannot
//! drift into doing something the terminal would not.
//!
//! The client is never linked: Xodus is GPL-3.0-only, and the decrypted
//! executable has to stay a child of the process that opened it.
mod editor;
mod model;
mod progress;
mod state;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use ferestre_core::recipe::Recipe;
use ferestre_core::{account, catalog, gamepass, library, steam};
use model::{Action, Inputs, LibraryRow, Ownership};
use state::{Model, Section, AVATAR_PX, ICON_PX, PER_PAGE};

const APP_ID: &str = "io.github.icex.ferestre";

/// Everything a callback needs to redraw the window, in one place so it can
/// hold one clone instead of nine.
#[derive(Clone)]
struct Ui {
    model: Rc<RefCell<Model>>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    content: gtk::Box,
    account_button: gtk::Button,
    title: adw::WindowTitle,
    search: gtk::SearchEntry,
    spinner: gtk::Spinner,
    /// Set while a background task runs, so a second click cannot queue a
    /// duplicate sign-in or a duplicate library fetch.
    busy: Rc<RefCell<bool>>,
    /// Product ids whose name has been looked up, so a lookup that came back
    /// with nothing is not repeated on every redraw.
    asked_names: Rc<RefCell<std::collections::BTreeSet<String>>>,
    /// Set while the Game Pass listing is being fetched, so opening the section
    /// twice does not ask twice.
    fetching_gamepass: Rc<RefCell<bool>>,
    /// The bar along the bottom, and the download it is describing.
    download: DownloadBar,
    /// Titles this window started that have not exited, by product id, each
    /// mapped to the process group to signal. Deliberately not the `busy` flag:
    /// a game runs for hours and must not hold the window's other work.
    running: Rc<RefCell<BTreeMap<String, u32>>>,
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

/// Let a window run from a source checkout find its own icon.
///
/// An installed copy gets it from the icon theme, because the package puts it
/// there. A `cargo run` gets nothing, and the shell draws a letter tile -- for
/// a project whose icon is the thing its name is a joke about, that is a poor
/// first impression and it took someone reporting it to notice.
///
/// Adds the checkout's icon directory to the theme's search path when the
/// binary is sitting in a `target/` directory with one above it. Silent when
/// there is nothing there, which is every installed run.
fn add_source_icon_path(display: &gdk::Display) {
    let Some(exe) = std::env::current_exe().ok() else {
        return;
    };
    // target/debug/ferestre-gui -> the checkout root.
    let Some(root) = exe
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
    else {
        return;
    };
    let icons = root.join("packaging/icons");
    if icons.join("hicolor/scalable/apps").is_dir() {
        gtk::IconTheme::for_display(display).add_search_path(&icons);
    }
}

fn build_ui(app: &adw::Application) {
    let model = Rc::new(RefCell::new(Model::load()));

    if let Some(display) = gdk::Display::default() {
        add_source_icon_path(&display);
    }
    // Without this the shell has no icon to draw and falls back to a generated
    // letter tile. The name is the application id, which is what the packaging
    // installs the SVG as. Set here rather than in `main`, because it goes
    // through GTK and GTK is not initialised until the application activates.
    gtk::Window::set_default_icon_name(APP_ID);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Ferestre")
        .default_width(960)
        .default_height(700)
        .build();

    // --- sidebar ----------------------------------------------------------

    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    for section in Section::ALL {
        let row = adw::ActionRow::builder().title(section.title()).build();
        row.add_prefix(&gtk::Image::from_icon_name(section.icon()));
        sidebar.append(&row);
    }
    sidebar.select_row(sidebar.row_at_index(0).as_ref());

    let account_button = gtk::Button::builder()
        .css_classes(["flat"])
        .margin_start(6)
        .margin_end(6)
        .margin_bottom(6)
        .build();

    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_box.append(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&sidebar)
            .build(),
    );
    sidebar_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    sidebar_box.append(&account_button);

    let sidebar_view = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.set_title_widget(Some(&adw::WindowTitle::new("Ferestre", "")));
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(&sidebar_box));

    // --- content ----------------------------------------------------------

    let title = adw::WindowTitle::new(Section::Library.title(), "");
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search by name or product id")
        .width_chars(26)
        .build();
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Reload recipes and re-check the runtime"));
    let spinner = gtk::Spinner::new();

    let content_header = adw::HeaderBar::new();
    content_header.set_title_widget(Some(&title));
    content_header.pack_start(&search);
    content_header.pack_end(&refresh);
    content_header.pack_end(&spinner);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&scroll));

    // Below the content and outside it, because `render` empties the content
    // box: an inline bar would be destroyed and rebuilt four times a second,
    // taking the scroll position and the keyboard focus with it.
    let download = DownloadBar::new();

    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&content_header);
    content_view.add_bottom_bar(&download.root);
    content_view.set_content(Some(&toasts));

    let split = adw::NavigationSplitView::builder()
        .min_sidebar_width(210.0)
        .max_sidebar_width(260.0)
        .sidebar(
            &adw::NavigationPage::builder()
                .title("Ferestre")
                .child(&sidebar_view)
                .build(),
        )
        .content(
            &adw::NavigationPage::builder()
                .title("Library")
                .child(&content_view)
                .build(),
        )
        .build();
    window.set_content(Some(&split));

    let ui = Ui {
        model,
        window: window.clone(),
        toasts,
        content,
        account_button: account_button.clone(),
        title,
        search: search.clone(),
        spinner,
        busy: Rc::new(RefCell::new(false)),
        running: Rc::new(RefCell::new(BTreeMap::new())),
        asked_names: Rc::new(RefCell::new(std::collections::BTreeSet::new())),
        fetching_gamepass: Rc::new(RefCell::new(false)),
        download: download.clone(),
    };

    // Activation and selection both, because they are not the same event:
    // clicking selects, but Enter and an assistive technology activate, and a
    // sidebar that only answers the mouse is a sidebar half the people cannot
    // use.
    sidebar.connect_row_activated(|list, row| list.select_row(Some(row)));
    sidebar.connect_row_selected(glib::clone!(
        #[strong]
        ui,
        move |_, row| {
            let Some(row) = row else { return };
            let index = row.index().max(0) as usize;
            let Some(section) = Section::ALL.get(index).copied() else {
                return;
            };
            {
                let mut model = ui.model.borrow_mut();
                if model.section == section {
                    return;
                }
                model.section = section;
                model.page = 0;
            }
            render(&ui);
            refresh_updates(&ui);
        }
    ));

    search.connect_search_changed(glib::clone!(
        #[strong]
        ui,
        move |entry| {
            {
                let mut model = ui.model.borrow_mut();
                let text = entry.text().to_string();
                if model.query == text {
                    return;
                }
                model.query = text;
                model.page = 0;
            }
            render(&ui);
        }
    ));

    refresh.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| {
            ui.model.borrow_mut().reload();
            render(&ui);
            // Reload means "check again", and the Game Pass catalogue is a
            // thing that changes without anything here doing so.
            fetch_gamepass(&ui, true);
        }
    ));

    account_button.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| account_pressed(&ui)
    ));

    render(&ui);
    // The account is cheap and answers the first question the window raises --
    // whose library is this -- so it is asked without being told to. The
    // library is not: it signs in, pages through collections and can take
    // seconds, so it waits to be asked.
    if ui.model.borrow().runtime.is_none() {
        // A fresh AppImage has the small launcher and client, while the large
        // runtime follows its own release train. Make that first download an
        // ordinary visible operation instead of an error the person has to
        // diagnose before their first launch.
        spawn_cli_tracked(&ui, &["install-runtime"], "Downloading the runtime");
    } else {
        refresh_account(&ui);
    }

    window.present();
}

// --- rendering ------------------------------------------------------------

fn render(ui: &Ui) {
    while let Some(child) = ui.content.first_child() {
        ui.content.remove(&child);
    }

    let section = ui.model.borrow().section;
    ui.title.set_title(section.title());
    ui.search.set_visible(matches!(
        section,
        Section::Library | Section::GamePass | Section::Installed
    ));
    render_account_button(ui);

    let page = adw::PreferencesPage::new();
    ui.content.append(&page);

    if let Some(problem) = ui.model.borrow().problem.clone() {
        let group = adw::PreferencesGroup::builder().title("Not ready").build();
        let row = adw::ActionRow::builder().title(&problem).build();
        row.add_css_class("error");
        group.add(&row);
        page.add(&group);
    }

    match section {
        Section::Runtime => render_runtime(ui, &page),
        Section::Library => {
            let show_all = ui.model.borrow().show_unsupported;
            render_rows(
                ui,
                &page,
                move |row| show_all || !row.known_uninstallable(),
                "Nothing here yet.",
            )
        }
        Section::GamePass => {
            // Fetched the first time the section is opened rather than at
            // startup: it is a network round trip for a list most sessions
            // never look at.
            fetch_gamepass(ui, false);
            render_gamepass(ui, &page)
        }
        Section::Installed => render_rows(
            ui,
            &page,
            |row| row.installed,
            "Nothing is installed yet. Find a title in the library and install it.",
        ),
        Section::Updates => render_rows(
            ui,
            &page,
            |row| row.update == Some(true),
            "Everything installed is up to date.",
        ),
    }
}

fn render_runtime(ui: &Ui, page: &adw::PreferencesPage) {
    let model = ui.model.borrow();
    let (title, subtitle) = model::runtime_summary(model.runtime.as_ref());
    let group = adw::PreferencesGroup::builder()
        .title("Patched Proton")
        .build();
    let row = adw::ActionRow::builder()
        .title(&title)
        .subtitle(&subtitle)
        .subtitle_lines(3)
        .build();
    if model.runtime.is_none() {
        let install = gtk::Button::builder()
            .label("Install")
            .valign(gtk::Align::Center)
            .tooltip_text("Download the current patched Proton runtime")
            .build();
        install.connect_clicked(glib::clone!(
            #[strong]
            ui,
            move |_| spawn_cli_tracked(&ui, &["install-runtime"], "Downloading the runtime")
        ));
        row.add_suffix(&install);
    }
    group.add(&row);
    page.add(&group);

    if model.runtimes.len() > 1 {
        let versions = adw::PreferencesGroup::builder()
            .title("Installed versions")
            .description("New launches use the newest compatible version. A title-specific choice is kept locally.")
            .build();
        for runtime in &model.runtimes {
            versions.add(
                &adw::ActionRow::builder()
                    .title(runtime.label())
                    .subtitle(runtime.path.display().to_string())
                    .subtitle_lines(2)
                    .build(),
            );
        }
        page.add(&versions);
    }

    if let Some(runtime) = &model.runtime {
        let capabilities = adw::PreferencesGroup::builder()
            .title("Capabilities")
            .description("What this build provides. A recipe names the ones it needs.")
            .build();
        for capability in &runtime.provides {
            let row = adw::ActionRow::builder().title(capability).build();
            if let Some(symptom) = model.registry.as_ref().and_then(|r| r.symptom(capability)) {
                row.set_subtitle(symptom);
                row.set_subtitle_lines(2);
            }
            capabilities.add(&row);
        }
        if runtime.provides.is_empty() {
            capabilities.add(
                &adw::ActionRow::builder()
                    .title("None found")
                    .subtitle("The build publishes no capability list, and probing found nothing")
                    .build(),
            );
        }
        page.add(&capabilities);
    }
}

/// What one page of the list looks like, lifted out of the borrow so widgets
/// can be built without holding the model.
struct Drawn {
    rows: Vec<LibraryRow>,
    label: String,
    has_previous: bool,
    has_next: bool,
    /// Rows on this page whose art has not been fetched yet.
    want_icons: Vec<String>,
    /// Rows on this page still showing a product id because nothing has looked
    /// their name up.
    want_names: Vec<String>,
    /// How many rows ship in a package format this runtime cannot open, and how
    /// many the account's subscription tier does not include. Counted before
    /// the filter, so the numbers do not change depending on whether they are
    /// currently being shown.
    unsupported: usize,
    outside_tier: usize,
    /// Rows in the whole list still waiting for a name while a search is
    /// running. A search cannot match what has not been fetched, and saying
    /// "nothing matches" during that is telling somebody something untrue.
    pending_names: usize,
}

fn draw_list(
    model: &Model,
    running: &BTreeMap<String, u32>,
    downloading: Option<&str>,
    section: Section,
    keep: impl Fn(&LibraryRow) -> bool,
) -> Drawn {
    let running = |product_id: &str| running.contains_key(&product_id.to_ascii_uppercase());
    let installing =
        |product_id: &str| downloading.is_some_and(|id| id.eq_ignore_ascii_case(product_id));
    let installed = |r: &Recipe| model.is_installed(r);
    let installed_version = |r: &Recipe| model.installed_version(r);
    let installed_product = |id: &str| model.product_is_installed(id);
    let describes_itself = |id: &str| model.product_executable(id).is_some();
    let inputs = Inputs {
        recipes: &model.recipes,
        runtime: model.runtime.as_ref(),
        registry: model.registry.as_ref(),
        ownership: &model.ownership,
        catalog: &model.catalog,
        records: &model.records,
        // Filled by `refresh_updates` after every reload. See
        // `Inputs::available` and docs/ROADMAP.md.
        available: &model.available,
        installed: &installed,
        installed_version: &installed_version,
        installed_product: &installed_product,
        product_describes_itself: &describes_itself,
        installing: &installing,
        running: &running,
    };
    // The Game Pass section is a different list, not a filter over the same
    // one: its rows come from a public catalogue listing rather than from what
    // the account owns.
    let rows: Vec<LibraryRow> = match section {
        Section::GamePass => {
            let held: Vec<&str> = model.subscriptions.clone();
            model::gamepass(&inputs, &model.gamepass, &model.tiers, &held)
        }
        _ => model::library(&inputs),
    };
    let unsupported = rows.iter().filter(|row| row.unsupported).count();
    let outside_tier = rows.iter().filter(|row| row.outside_tier).count();
    let rows: Vec<LibraryRow> = rows.into_iter().filter(|row| keep(row)).collect();

    let matching = model::search(&rows, &model.query);
    let view = model::paginate(&matching, model.page, PER_PAGE);
    // With a query, the rows that are *not* on screen matter: search matches on
    // the name, and a row nothing has looked up is still showing its product
    // id, so "age of" cannot match it. That was a search that found nothing
    // until you happened to page past the row, and then worked ever after.
    let searching = !model.query.trim().is_empty();
    let unnamed_off_page: Vec<String> = if searching {
        rows.iter()
            .filter(|row| !row.is_named())
            .map(|row| row.product_id.clone())
            .collect()
    } else {
        Vec::new()
    };

    Drawn {
        pending_names: unnamed_off_page.len(),
        want_names: unnamed_off_page
            .iter()
            .cloned()
            .chain(view.rows.iter().flat_map(|row| {
                let mut wanted = Vec::new();
                if !row.is_named() {
                    wanted.push(row.product_id.clone());
                }
                // A bundle's answer is in another record. Until its children
                // are fetched the row can only say how many there are, so they
                // are asked for alongside the names on this page rather than
                // waiting for someone to open something.
                if let Some(product) = model.catalog.get(&row.product_id.to_ascii_uppercase()) {
                    wanted.extend(
                        product
                            .bundled_ids
                            .iter()
                            .filter(|id| !model.catalog.contains_key(&id.to_ascii_uppercase()))
                            .cloned(),
                    );
                }
                wanted
            }))
            .collect(),
        want_icons: view
            .rows
            .iter()
            .filter(|row| {
                row.image.is_some()
                    && !model
                        .icons
                        .contains_key(&row.product_id.to_ascii_uppercase())
            })
            .map(|row| row.product_id.clone())
            .collect(),
        label: view.label(),
        has_previous: view.has_previous(),
        has_next: view.has_next(),
        rows: view.rows.into_iter().cloned().collect(),
        unsupported,
        outside_tier,
    }
}

/// The Game Pass section: a header saying what this list is and what it is not,
/// then the titles.
///
/// The header is not decoration. A row here means the PC catalogue includes the
/// title, which is not the same as this account being able to install it, and
/// leaving that unsaid would make every licence refusal look like a bug in the
/// launcher.
fn render_gamepass(ui: &Ui, page: &adw::PreferencesPage) {
    let (held, listing, market) = {
        let model = ui.model.borrow();
        (
            model.subscriptions.clone(),
            model.gamepass.len(),
            model.market().0,
        )
    };

    let group = adw::PreferencesGroup::builder().build();
    let (title, subtitle) = match (listing, held.as_slice()) {
        (0, _) => (
            "The Game Pass catalogue has not been fetched yet".to_string(),
            "Reload to fetch it. It is a public listing -- no account needed.".to_string(),
        ),
        (n, []) => (
            format!("{n} titles are included with PC Game Pass in {market}"),
            "No active subscription was found on this account, so installing one \
             will be refused at the licence step."
                .to_string(),
        ),
        (n, subs) => (
            format!("{n} titles are included with PC Game Pass in {market}"),
            format!(
                "This account holds {}. Whether a tier covers a given title is decided \
                 when it is installed, not here.",
                subs.join(" and ")
            ),
        ),
    };
    let row = adw::ActionRow::builder()
        .title(&title)
        .subtitle(&subtitle)
        .subtitle_lines(3)
        .build();
    row.add_css_class("dim-label");
    group.add(&row);
    page.add(&group);

    let show_all = ui.model.borrow().show_unsupported;
    render_rows(
        ui,
        page,
        move |row| show_all || !row.known_uninstallable(),
        "Nothing here yet.",
    );
}

fn render_rows(
    ui: &Ui,
    page: &adw::PreferencesPage,
    keep: impl Fn(&LibraryRow) -> bool,
    empty_message: &str,
) {
    let section = ui.model.borrow().section;
    let downloading = ui
        .download
        .current
        .borrow()
        .as_ref()
        .map(|d| d.product_id.clone());
    let drawn = draw_list(
        &ui.model.borrow(),
        &ui.running.borrow(),
        downloading.as_deref(),
        section,
        keep,
    );

    let group = adw::PreferencesGroup::builder().build();
    // Deliberately not an early return: an empty list is exactly when the
    // reader most needs the switch below, because the filter may be the reason
    // it is empty.
    if drawn.rows.is_empty() {
        let query = ui.model.borrow().query.clone();
        let message = if query.trim().is_empty() {
            empty_message.to_string()
        } else if drawn.pending_names > 0 {
            format!(
                "Still looking up {} titles — searching finds the ones the catalog has \
                 answered about",
                drawn.pending_names
            )
        } else {
            format!("Nothing matches {query:?}")
        };
        let row = adw::ActionRow::builder().title(&message).build();
        row.add_css_class("dim-label");
        group.add(&row);
    } else {
        for row in &drawn.rows {
            group.add(&library_row(ui, row));
        }
    }
    page.add(&group);

    // Only in the library: the other sections list installed or updatable
    // titles, which are runnable by construction.
    let held_back = drawn.unsupported + drawn.outside_tier;
    if matches!(section, Section::Library | Section::GamePass) && held_back > 0 {
        // Two reasons a title is held back, and the switch names whichever
        // applies. Saying "95 titles" without saying why is the kind of filter
        // that makes people think a library is broken.
        let mut why = Vec::new();
        if drawn.outside_tier > 0 {
            why.push(format!(
                "{} are not in the Game Pass tier this account holds",
                drawn.outside_tier
            ));
        }
        if drawn.unsupported > 0 {
            why.push(format!(
                "{} ship as Appx or Msix packages, which this runtime cannot open",
                drawn.unsupported
            ));
        }
        let row = adw::SwitchRow::builder()
            .title("Show titles you cannot install")
            .subtitle(format!("{}.", why.join("; ")))
            .subtitle_lines(2)
            .active(ui.model.borrow().show_unsupported)
            .build();
        let ui = ui.clone();
        row.connect_active_notify(move |switch| {
            {
                let mut model = ui.model.borrow_mut();
                if model.show_unsupported == switch.is_active() {
                    return;
                }
                model.show_unsupported = switch.is_active();
                // The page the reader was on does not exist in the other list.
                model.page = 0;
            }
            render(&ui);
        });
        let group = adw::PreferencesGroup::builder().margin_top(18).build();
        group.add(&row);
        page.add(&group);
    }

    // Only when there is more than one page: a pager under a list that already
    // fits on screen is noise.
    if drawn.has_previous || drawn.has_next {
        let pager = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(12)
            .margin_bottom(18)
            .build();
        let previous = gtk::Button::from_icon_name("go-previous-symbolic");
        previous.set_sensitive(drawn.has_previous);
        let next = gtk::Button::from_icon_name("go-next-symbolic");
        next.set_sensitive(drawn.has_next);
        let counter = gtk::Label::builder()
            .label(&drawn.label)
            .css_classes(["dim-label"])
            .build();

        previous.connect_clicked(glib::clone!(
            #[strong]
            ui,
            move |_| {
                {
                    let mut model = ui.model.borrow_mut();
                    model.page = model.page.saturating_sub(1);
                }
                render(&ui);
            }
        ));
        next.connect_clicked(glib::clone!(
            #[strong]
            ui,
            move |_| {
                ui.model.borrow_mut().page += 1;
                render(&ui);
            }
        ));

        pager.append(&previous);
        pager.append(&counter);
        pager.append(&next);
        ui.content.append(&pager);
    }

    // Names before art: a row reading "9NBLGGH18846" is unusable, a row with no
    // picture is merely plain. Both fetch themselves rather than waiting to be
    // asked, because "press this button to find out what your games are called"
    // is not a thing anyone should have to know.
    if !drawn.want_names.is_empty() {
        fetch_names(ui, drawn.want_names);
    }
    if !drawn.want_icons.is_empty() {
        fetch_icons(ui, drawn.want_icons);
    }
}

fn library_row(ui: &Ui, row: &LibraryRow) -> adw::ActionRow {
    // A title is a name, not markup, and the flag has to be set before the text
    // is: left as markup, "Minecraft: Java & Bedrock Edition for PC" fails to
    // parse and the row draws with no title at all. The same goes for any
    // subtitle that names a title, which the bundle rows do.
    let action_row = adw::ActionRow::builder()
        .subtitle_lines(2)
        .tooltip_text(&row.product_id)
        .build();
    action_row.set_use_markup(false);
    action_row.set_title(&row.name);
    action_row.set_subtitle(&row.subtitle);

    let icon = gtk::Image::builder()
        .pixel_size(40)
        .icon_name("applications-games-symbolic")
        .margin_top(6)
        .margin_bottom(6)
        .build();
    if let Some(path) = ui
        .model
        .borrow()
        .icons
        .get(&row.product_id.to_ascii_uppercase())
    {
        if let Ok(texture) = gdk::Texture::from_filename(path) {
            icon.set_paintable(Some(&texture));
        }
    }
    action_row.add_prefix(&icon);

    if let Some((text, css)) = row.badge {
        let badge = gtk::Label::builder()
            .label(text)
            .css_classes([css, "caption"])
            .valign(gtk::Align::Center)
            .width_chars(12)
            .xalign(1.0)
            .build();
        action_row.add_suffix(&badge);
    }

    let store = gtk::Button::builder()
        .icon_name("web-browser-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text("Open this title's Microsoft Store page")
        .build();
    let store_url = format!("{STORE_PAGE}{}", row.product_id);
    store.connect_clicked(move |button| open_link(button, &store_url));
    action_row.add_suffix(&store);

    // Anything on disk can be removed, recipe or not -- and the rows that most
    // need it are the ones with no recipe, because a failed install leaves
    // exactly that and there was previously no way to say so.
    if row.installed {
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Remove this title from disk")
            .build();
        let product_id = row.product_id.clone();
        let name = row.name.clone();
        remove.connect_clicked(glib::clone!(
            #[strong]
            ui,
            move |_| confirm_remove(&ui, &product_id, &name)
        ));
        action_row.add_suffix(&remove);
    }

    // A shortcut needs something that will launch. A recipe is one; a title on
    // disk is another, because `run` writes a recipe from the package manifest
    // on its way past.
    if row.has_recipe || row.installed {
        let to_steam = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Add to Steam as a non-Steam shortcut")
            .build();
        let product_id = row.product_id.clone();
        let name = row.name.clone();
        to_steam.connect_clicked(glib::clone!(
            #[strong]
            ui,
            move |_| add_to_steam(&ui, &product_id, &name)
        ));
        action_row.add_suffix(&to_steam);

        let edit = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Edit how this title launches")
            .build();
        let product_id = row.product_id.clone();
        let name = row.name.clone();
        edit.connect_clicked(glib::clone!(
            #[strong]
            ui,
            move |_| open_editor(&ui, &product_id, &name)
        ));
        action_row.add_suffix(&edit);
    }

    // A greyed-out "Play" is a button that says a title might work if you found
    // the right thing to click. It never will: the reason is in the subtitle
    // and on the row, and the Store link beside it is the thing to press.
    if let Action::Blocked(reason) = &row.action {
        action_row.set_tooltip_text(Some(reason));
        return action_row;
    }

    let button = gtk::Button::builder()
        .label(row.action.label())
        .valign(gtk::Align::Center)
        .css_classes(["suggested-action"])
        .build();
    button.set_sensitive(row.action.is_enabled());

    match &row.action {
        Action::Blocked(reason) => button.set_tooltip_text(Some(reason)),
        Action::Installing => {
            button.set_tooltip_text(Some("Downloading — the bar at the bottom has the detail"));
            button.remove_css_class("suggested-action");
        }
        Action::Adopt => {
            button.set_tooltip_text(Some("Describe how this title launches, and try it"));
            button.remove_css_class("suggested-action");
            let product_id = row.product_id.clone();
            let name = row.name.clone();
            button.connect_clicked(glib::clone!(
                #[strong]
                ui,
                move |_| open_editor(&ui, &product_id, &name)
            ));
        }
        Action::Stop => {
            button.set_tooltip_text(Some("Ask the title to close"));
            button.remove_css_class("suggested-action");
            button.add_css_class("destructive-action");
            let product_id = row.product_id.clone();
            let name = row.name.clone();
            button.connect_clicked(glib::clone!(
                #[strong]
                ui,
                move |_| stop_title(&ui, &product_id, &name)
            ));
        }
        action => {
            let args: Vec<String> = action
                .command(&row.product_id)
                .expect("a runnable action has a command")
                .iter()
                .map(|s| s.to_string())
                .collect();
            button.set_tooltip_text(Some(&format!("ferestre {}", args.join(" "))));
            // Launching a game is fire-and-forget: it runs for hours and the
            // window has nothing to update when it ends. Installing is not --
            // it changes what every row says about itself.
            let waits = matches!(row.action, Action::Install | Action::Update);
            let plays = matches!(row.action, Action::Play);
            let updating = matches!(row.action, Action::Update);
            // A bundle installs its child, and the dialog names the child: a
            // 23 GB download starting under a different title's name would be
            // the launcher doing something it never explained.
            let (product_id, title_name) = match &row.install_as {
                Some((id, name)) => (id.clone(), name.clone()),
                None => (row.product_id.clone(), row.name.clone()),
            };
            button.connect_clicked(glib::clone!(
                #[strong]
                ui,
                move |_| {
                    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                    if waits {
                        confirm_install(&ui, &product_id, &title_name, updating);
                    } else if plays {
                        spawn_title(&ui, &product_id, &title_name);
                    } else {
                        spawn_cli(&ui, &borrowed);
                    }
                }
            ));
        }
    }
    action_row.add_suffix(&button);
    action_row.set_activatable_widget(Some(&button));
    action_row
}

fn render_account_button(ui: &Ui) {
    let model = ui.model.borrow();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let avatar = adw::Avatar::new(28, model.account.as_ref().map(|a| a.label()), true);
    if let Some(path) = &model.avatar {
        if let Ok(texture) = gdk::Texture::from_filename(path) {
            avatar.set_custom_image(Some(&texture));
        }
    }
    content.append(&avatar);

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
    labels.append(
        &gtk::Label::builder()
            .label(match &model.account {
                Some(account) => account.label(),
                None => "Not signed in",
            })
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    labels.append(
        &gtk::Label::builder()
            .label(if model.ownership.is_known() {
                "Library loaded"
            } else if model.account.is_some() {
                "Load library"
            } else {
                "Sign in"
            })
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    content.append(&labels);

    ui.account_button.set_child(Some(&content));
    ui.account_button
        .set_tooltip_text(Some(match &model.account {
            Some(_) => "This account, and what to do with it",
            None => "Sign in through the Xodus client",
        }));
}

/// A title's page on the web Store, by product id.
const STORE_PAGE: &str = "https://apps.microsoft.com/detail/";

/// Xbox's own page for the signed-in account.
const XBOX_PROFILE: &str = "https://account.xbox.com/en-us/profile";
/// Where a title is bought, for the titles this cannot install because the
/// account does not own them.
const MICROSOFT_STORE: &str = "https://apps.microsoft.com/games";

/// The menu behind the account button, once somebody is signed in.
///
/// A button that did one unlabelled thing was the wrong shape for it: signing
/// out had nowhere to live, and "reload what this account owns" is not what a
/// person expects a name and a picture to do.
fn account_menu(ui: &Ui) {
    let popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(true)
        .build();
    let items = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();

    let entry = |label: &str, tooltip: &str| {
        let button = gtk::Button::builder()
            .label(label)
            .tooltip_text(tooltip)
            .css_classes(["flat"])
            .build();
        button.set_child(Some(
            &gtk::Label::builder().label(label).xalign(0.0).build(),
        ));
        button
    };

    let reload = entry(
        "Reload library",
        "Ask the service again what this account owns",
    );
    reload.connect_clicked(glib::clone!(
        #[strong]
        ui,
        #[strong]
        popover,
        move |_| {
            popover.popdown();
            refresh_library(&ui);
        }
    ));
    items.append(&reload);

    let profile = entry("My profile", XBOX_PROFILE);
    profile.connect_clicked(glib::clone!(
        #[strong]
        popover,
        move |button| {
            popover.popdown();
            open_link(button, XBOX_PROFILE);
        }
    ));
    items.append(&profile);

    let store = entry("Microsoft Store", MICROSOFT_STORE);
    store.connect_clicked(glib::clone!(
        #[strong]
        popover,
        move |button| {
            popover.popdown();
            open_link(button, MICROSOFT_STORE);
        }
    ));
    items.append(&store);

    items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let out = entry("Sign out", "Forget this machine's tokens");
    out.add_css_class("destructive-action");
    out.connect_clicked(glib::clone!(
        #[strong]
        ui,
        #[strong]
        popover,
        move |_| {
            popover.popdown();
            confirm_sign_out(&ui);
        }
    ));
    items.append(&out);

    popover.set_child(Some(&items));
    popover.set_parent(&ui.account_button);
    popover.popup();
    // The popover is parented to a widget that outlives it, so it has to be
    // unparented or it leaks and the next one stacks on top of it.
    popover.connect_closed(|popover| popover.unparent());
}

fn open_link(widget: &impl IsA<gtk::Widget>, url: &str) {
    let launcher = gtk::UriLauncher::new(url);
    launcher.launch(
        widget.as_ref().root().and_downcast_ref::<gtk::Window>(),
        gio::Cancellable::NONE,
        |_| {},
    );
}

/// Signing out is asked about, because what it costs is not obvious from the
/// words: nothing will launch afterwards until somebody signs in again. A
/// launch needs a licence, and a licence needs a token.
fn confirm_sign_out(ui: &Ui) {
    let dialog = adw::AlertDialog::new(
        Some("Sign out?"),
        Some(
            "This machine's tokens are removed. Titles already downloaded stay on disk, \
             but none of them will start until you sign in again -- launching needs a \
             licence, and a licence needs a signed-in account.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("out", "Sign out");
    dialog.set_response_appearance("out", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");

    let window = ui.window.clone();
    let ui = ui.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "out" {
            sign_out(&ui);
        }
    });
    dialog.present(Some(&window));
}

/// Remove the tokens, through the client that owns them.
///
/// Not by deleting a file: the client keeps them in a keychain-backed store
/// whose location is its business, and reaching into it from here would break
/// the first time it changed.
fn sign_out(ui: &Ui) {
    let Some(client) = ui.model.borrow().xodus_cli() else {
        ui.toasts
            .add_toast(adw::Toast::new("The Xodus client was not found"));
        return;
    };
    if !begin(ui) {
        return;
    }
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            std::process::Command::new(&client)
                .arg("logout")
                .stdin(std::process::Stdio::null())
                .status()
                .map_err(|e| format!("{}: {e}", client.display()))
        })
        .await;
        finish(&ui);
        match result {
            Ok(Ok(status)) if status.success() => {
                {
                    // The listing goes with the tokens. Leaving it on screen
                    // would show a library belonging to nobody, with buttons
                    // that all fail at the licence step.
                    let mut model = ui.model.borrow_mut();
                    model.account = None;
                    model.avatar = None;
                    model.ownership = Ownership::Unknown;
                    model.subscriptions.clear();
                    if let Some(paths) = model.paths.as_ref() {
                        let _ = std::fs::remove_file(library::cache_path(paths.state_dir()));
                    }
                }
                render(&ui);
                ui.toasts.add_toast(adw::Toast::new("Signed out"));
            }
            Ok(Ok(status)) => ui
                .toasts
                .add_toast(adw::Toast::new(&format!("Sign out exited {status}"))),
            Ok(Err(e)) => ui.toasts.add_toast(adw::Toast::new(&e)),
            Err(_) => ui
                .toasts
                .add_toast(adw::Toast::new("Could not run the client")),
        }
    });
}

// --- actions ---------------------------------------------------------------

fn cli_binary() -> PathBuf {
    let exe = std::env::current_exe().ok();
    model::cli_binary(exe.as_deref().and_then(Path::parent), &|p| p.is_file())
}

/// Run `ferestre` and leave it running. Output goes where the CLI already sends it
/// -- a per-title log file -- rather than into a widget nobody would read while
/// a game is starting.
/// Start a title and keep track of it, so the row can say it is running.
///
/// Its own process group: `ferestre run` is a chain -- launcher, client, Proton,
/// then the game -- and signalling only the process this window started would
/// leave the game running with nothing tracking it.
fn spawn_title(ui: &Ui, product_id: &str, name: &str) {
    use std::os::unix::process::CommandExt;

    let program = cli_binary();
    let mut command = std::process::Command::new(&program);
    command.arg("run").arg(product_id);
    // Safety: setsid in the child between fork and exec. Async-signal-safe, and
    // the only thing this closure does.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            ui.toasts
                .add_toast(adw::Toast::new(&format!("{}: {e}", program.display())));
            return;
        }
    };

    let key = product_id.to_ascii_uppercase();
    // setsid makes the child its own group leader, so the group id is its pid.
    ui.running.borrow_mut().insert(key.clone(), child.id());
    render(ui);
    ui.toasts
        .add_toast(adw::Toast::new(&format!("Starting {name}")));

    // Wait off the main thread, and deliberately without the `busy` flag: a
    // game runs for hours and must not hold up the rest of the window.
    let ui = ui.clone();
    let name = name.to_string();
    glib::spawn_future_local(async move {
        let mut child = child;
        let status = gio::spawn_blocking(move || child.wait()).await;
        ui.running.borrow_mut().remove(&key);
        render(&ui);
        match status {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => ui
                .toasts
                .add_toast(adw::Toast::new(&format!("{name} exited {status}"))),
            _ => ui
                .toasts
                .add_toast(adw::Toast::new(&format!("{name}: could not be waited on"))),
        }
    });
}

/// Ask a running title to close.
///
/// SIGTERM to the whole group, not SIGKILL: a game killed outright loses
/// whatever it had not written, and saves are most of why these titles are run
/// here at all. A title that ignores it keeps running and the row keeps saying
/// so, which is honest -- the window does not pretend to have stopped it.
fn stop_title(ui: &Ui, product_id: &str, name: &str) {
    let key = product_id.to_ascii_uppercase();
    let Some(pgid) = ui.running.borrow().get(&key).copied() else {
        return;
    };
    // Safety: a signal to a process group this window created.
    let sent = unsafe { libc::kill(-(pgid as i32), libc::SIGTERM) };
    if sent != 0 {
        ui.toasts.add_toast(adw::Toast::new(&format!(
            "Could not signal {name}: {}",
            std::io::Error::last_os_error()
        )));
    }

    // And Wine's own way, because the signal above does not reach the game.
    // wineserver detaches, so the title is reparented out of the group this
    // window created and survives it -- measured, with the row saying stopped
    // while Minecraft was still running. `wineserver -k` ends the prefix.
    let server = {
        let model = ui.model.borrow();
        model
            .recipe_for(product_id)
            .and_then(|recipe| model.wineserver(recipe))
    };
    if let Some((server, prefix)) = server {
        if let Ok(child) = std::process::Command::new(server)
            .env("WINEPREFIX", prefix)
            .arg("-k")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            // Reaped, because stopping a title is something people do more than
            // once and an unwaited child is a zombie each time.
            glib::spawn_future_local(async move {
                let mut child = child;
                let _ = gio::spawn_blocking(move || child.wait()).await;
            });
        }
    }
    ui.toasts
        .add_toast(adw::Toast::new(&format!("Asked {name} to close")));
}

fn spawn_cli(ui: &Ui, args: &[&str]) {
    let program = cli_binary();
    match std::process::Command::new(&program).args(args).spawn() {
        Ok(_) => ui.toasts.add_toast(adw::Toast::new(&format!(
            "Started: ferestre {}",
            args.join(" ")
        ))),
        Err(e) => ui
            .toasts
            .add_toast(adw::Toast::new(&format!("{}: {e}", program.display()))),
    }
}

/// Run `ferestre` and wait for it, then reload.
///
/// For work that changes what the window shows -- an install, an update, the
/// runtime -- as opposed to launching a game, which runs for hours and must not
/// hold anything. Waiting is the point: an install that finishes and leaves the
/// row saying "not recorded" looks like it failed.
/// The bar along the bottom of the window while something is downloading.
///
/// Its own type because it outlives every redraw: the page is emptied and
/// rebuilt whenever anything changes, and a progress bar that goes with it
/// would restart its animation, lose the scroll position, and flicker four
/// times a second for however long a 45 GB download takes.
#[derive(Clone)]
struct DownloadBar {
    root: gtk::Box,
    title: gtk::Label,
    detail: gtk::Label,
    bar: gtk::ProgressBar,
    /// Written by the thread reading the client's output, read by the tick.
    /// A mutex rather than a channel because only the newest number matters --
    /// a queue of stale byte counts would be drained and thrown away.
    seen: Arc<Mutex<Option<(u64, u64)>>>,
    /// What is being downloaded, or nothing.
    current: Rc<RefCell<Option<progress::Download>>>,
}

impl DownloadBar {
    fn new() -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(18)
            .margin_end(18)
            .margin_top(10)
            .margin_bottom(12)
            .visible(false)
            .build();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        title.add_css_class("heading");
        let bar = gtk::ProgressBar::new();
        let detail = gtk::Label::builder().xalign(0.0).build();
        detail.add_css_class("caption");
        detail.add_css_class("dim-label");
        root.append(&title);
        root.append(&bar);
        root.append(&detail);
        DownloadBar {
            root,
            title,
            detail,
            bar,
            seen: Arc::new(Mutex::new(None)),
            current: Rc::new(RefCell::new(None)),
        }
    }

    fn start(&self, product_id: &str, name: &str, what: &str) {
        let download = progress::Download::new(product_id, name, what, std::time::Instant::now());
        *self.seen.lock().unwrap() = None;
        self.title.set_label(&download.heading());
        *self.current.borrow_mut() = Some(download);
        self.detail.set_label("Starting");
        self.bar.set_fraction(0.0);
        self.root.set_visible(true);
    }

    fn stop(&self) {
        *self.current.borrow_mut() = None;
        self.root.set_visible(false);
    }

    /// Redraw from whatever the reader has seen. Called on a timer rather than
    /// per line: the client emits four updates a second and a window does not
    /// need to lay out text that often.
    fn tick(&self) {
        let mut current = self.current.borrow_mut();
        let Some(download) = current.as_mut() else {
            return;
        };
        let now = std::time::Instant::now();
        if let Some((done, total)) = *self.seen.lock().unwrap() {
            download.observe(done, total, now);
        }
        match download.fraction() {
            Some(fraction) => self.bar.set_fraction(fraction),
            // No total yet. An indeterminate bar says "working" without
            // claiming a position, which is the truth at that moment.
            None => self.bar.pulse(),
        }
        self.detail.set_label(&download.detail(now));
    }
}

/// Run `ferestre install`, drawing its progress.
///
/// Unlike [`spawn_cli_tracked`] this reads the child's output instead of
/// inheriting it, because that output is the only thing that knows how far a
/// download has got. The client draws a terminal progress bar which hides
/// itself when stdout is not a terminal -- exactly this case -- so it is asked
/// for machine-readable progress instead, and prints one JSON line per quarter
/// second. A client too old to know about that simply prints nothing this can
/// read, and the bar stays indeterminate rather than breaking.
fn spawn_install(ui: &Ui, product_id: &str, name: &str, dir: Option<&str>, what: &str) {
    if !begin(ui) {
        ui.toasts
            .add_toast(adw::Toast::new("Something is already running"));
        return;
    }
    let program = cli_binary();
    let mut args = vec!["install".to_string(), product_id.to_string()];
    if let Some(dir) = dir.map(str::trim).filter(|d| !d.is_empty()) {
        args.push(dir.to_string());
    }

    ui.download.start(product_id, name, what);
    let seen = Arc::clone(&ui.download.seen);

    // One timer for the life of the download, stopped when it ends. Attached
    // here rather than at startup so an idle window does no work at all.
    let ticker = ui.clone();
    let tick = glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        if ticker.download.current.borrow().is_none() {
            return glib::ControlFlow::Break;
        }
        ticker.download.tick();
        glib::ControlFlow::Continue
    });

    let ui = ui.clone();
    let what = what.to_string();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            let mut child = std::process::Command::new(&program)
                .args(&args)
                .env("XODUS_PROGRESS", "json")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| format!("{}: {e}", program.display()))?;

            if let Some(out) = child.stdout.take() {
                use std::io::BufRead;
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    match progress::parse_line(&line) {
                        Some(update) => *seen.lock().unwrap() = Some(update),
                        // Everything else on that stream is the client talking
                        // to a terminal that is not there. Passing it through
                        // keeps `ferestre install` debuggable from a console
                        // that started the window.
                        None => println!("{line}"),
                    }
                }
            }
            child
                .wait()
                .map_err(|e| format!("{}: {e}", program.display()))
        })
        .await;

        tick.remove();
        ui.download.stop();
        finish(&ui);
        match result {
            Ok(Ok(status)) if status.success() => {
                ui.model.borrow_mut().reload();
                render(&ui);
                ui.toasts
                    .add_toast(adw::Toast::new(&format!("{what} finished")));
            }
            Ok(Ok(status)) => {
                ui.model.borrow_mut().reload();
                render(&ui);
                ui.toasts
                    .add_toast(adw::Toast::new(&format!("{what} exited {status}")));
            }
            Ok(Err(e)) => ui.toasts.add_toast(adw::Toast::new(&e)),
            Err(_) => ui
                .toasts
                .add_toast(adw::Toast::new("The install could not be started")),
        }
    });
}

fn spawn_cli_tracked(ui: &Ui, args: &[&str], what: &str) {
    if !begin(ui) {
        ui.toasts
            .add_toast(adw::Toast::new("Something is already running"));
        return;
    }
    let program = cli_binary();
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let runtime_install = owned == ["install-runtime"];
    let tick = if runtime_install {
        // A runtime archive is large and GitHub cannot provide a stable byte
        // total before the redirect. Keep progress visibly moving throughout
        // discovery, download and unpacking.
        ui.download
            .start("runtime", "Patched runtime", "Installing");
        let ticker = ui.clone();
        Some(glib::timeout_add_local(
            std::time::Duration::from_millis(250),
            move || {
                if ticker.download.current.borrow().is_none() {
                    return glib::ControlFlow::Break;
                }
                ticker.download.tick();
                glib::ControlFlow::Continue
            },
        ))
    } else {
        None
    };
    ui.toasts
        .add_toast(adw::Toast::new(&format!("{what} — this can take a while")));

    let ui = ui.clone();
    let what = what.to_string();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            std::process::Command::new(&program)
                .args(&owned)
                .stdin(std::process::Stdio::null())
                .status()
                .map_err(|e| format!("{}: {e}", program.display()))
        })
        .await;
        if let Some(tick) = tick {
            tick.remove();
            ui.download.stop();
        }
        finish(&ui);
        match result {
            Ok(Ok(status)) if status.success() => {
                // The record was written by the child, so the window has to
                // re-read it before it can say anything different.
                ui.model.borrow_mut().reload();
                render(&ui);
                ui.toasts
                    .add_toast(adw::Toast::new(&format!("{what} finished")));
            }
            Ok(Ok(status)) => {
                ui.model.borrow_mut().reload();
                render(&ui);
                ui.toasts
                    .add_toast(adw::Toast::new(&format!("{what} exited {status}")))
            }
            Ok(Err(e)) => ui.toasts.add_toast(adw::Toast::new(&e)),
            Err(_) => ui
                .toasts
                .add_toast(adw::Toast::new(&format!("{what}: the process crashed"))),
        }
    });
}

/// Ask before downloading, and say where it is going.
///
/// A download here is between a few hundred megabytes and 160 GB, so starting
/// one on a single click -- with no size, no destination, and no way back --
/// is not a thing to do to somebody's evening or their disk.
fn confirm_install(ui: &Ui, product_id: &str, name: &str, updating: bool) {
    let (default_dir, size, free) = {
        let model = ui.model.borrow();
        let dir = model.install_destination(product_id);
        let product = model.catalog.get(&product_id.to_ascii_uppercase());
        let size = product.map(|p| p.size_label()).filter(|s| !s.is_empty());
        let free = dir.as_deref().and_then(free_space_label);
        (dir, size, free)
    };
    let Some(default_dir) = default_dir else {
        ui.toasts
            .add_toast(adw::Toast::new("Cannot work out where to put it"));
        return;
    };

    let heading = if updating {
        format!("Update {name}?")
    } else {
        format!("Install {name}?")
    };
    let body = match (&size, &free) {
        (Some(size), Some(free)) => format!("{size} to download. {free}"),
        (Some(size), None) => format!("{size} to download."),
        (None, Some(free)) => format!("The catalog does not give a size. {free}"),
        (None, None) => "The catalog does not give a size.".to_string(),
    };

    let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("install", if updating { "Update" } else { "Install" });
    dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("install"));
    dialog.set_close_response("cancel");

    // The destination is editable, because "somewhere on your smallest disk" is
    // not a decision to make on someone's behalf for 160 GB.
    let where_to = adw::EntryRow::builder()
        .title("Install to")
        .text(default_dir.to_string_lossy().as_ref())
        .build();
    let group = adw::PreferencesGroup::new();
    group.add(&where_to);
    dialog.set_extra_child(Some(&group));

    let window = ui.window.clone();
    let ui = ui.clone();
    let product_id = product_id.to_string();
    let name = name.to_string();
    dialog.connect_response(None, move |_, response| {
        if response != "install" {
            return;
        }
        let dir = where_to.text().to_string();
        let what = if updating { "Updating" } else { "Installing" };
        spawn_install(&ui, &product_id, &name, Some(dir.trim()), what);
    });
    dialog.present(Some(&window));
}

/// Ask before removing a title, and say exactly what will go.
///
/// The path and the size are both in the question, because "remove" is a
/// different amount of work to undo depending on whether it is 4 MB of a failed
/// download or 45 GB of a game -- and because a launcher deleting a directory
/// the person cannot see named is a launcher nobody should trust.
fn confirm_remove(ui: &Ui, product_id: &str, name: &str) {
    let dir = {
        let model = ui.model.borrow();
        model
            .records
            .get(&product_id.to_ascii_uppercase())
            .map(|record| record.dir.clone())
    };
    let Some(dir) = dir else {
        ui.toasts.add_toast(adw::Toast::new(
            "Nothing is recorded on disk for this title",
        ));
        return;
    };

    let size = ferestre_core::install::tree_size(&dir);
    let prefix = ui
        .model
        .borrow()
        .recipe_for(product_id)
        .and_then(|recipe| {
            ui.model
                .borrow()
                .paths
                .as_ref()
                .and_then(|paths| paths.prefix_dir(recipe).ok())
        })
        .filter(|p| p.is_dir());

    let body = format!(
        "{} will be deleted, freeing {}.\n\nThe title stays in your library and can be \
         installed again.",
        dir.display(),
        ferestre_core::human_bytes(size)
    );
    let dialog = adw::AlertDialog::new(Some(&format!("Remove {name}?")), Some(&body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Remove");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");

    // The Wine prefix is the title's C: drive, so it is where saved games are.
    // Off by default, and it says why: removing it silently along with the
    // files throws away the one thing somebody would want back, and leaving it
    // unmentioned is how a `stardew-valley-proton` sits there afterwards with
    // nothing explaining what it is.
    let also_prefix = prefix.as_ref().map(|path| {
        let check = gtk::CheckButton::builder()
            .label(format!(
                "Also delete saved games and settings ({}, in {})",
                ferestre_core::human_bytes(ferestre_core::install::tree_size(path)),
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
            .active(false)
            .margin_top(6)
            .build();
        dialog.set_extra_child(Some(&check));
        check
    });

    let window = ui.window.clone();
    let ui = ui.clone();
    let product_id = product_id.to_string();
    let name = name.to_string();
    dialog.connect_response(None, move |_, response| {
        if response != "remove" {
            return;
        }
        let mut args = vec!["uninstall", &product_id, "--yes"];
        if also_prefix
            .as_ref()
            .is_some_and(gtk::prelude::CheckButtonExt::is_active)
        {
            args.push("--prefix");
        }
        spawn_cli_tracked(&ui, &args, &format!("Removing {name}"));
    });
    dialog.present(Some(&window));
}

/// How much room is left where a title would go, in the words a person uses.
fn free_space_label(dir: &Path) -> Option<String> {
    // The directory itself may not exist yet; ask about the nearest ancestor
    // that does, which is the filesystem it will land on.
    let mut at = dir;
    while !at.is_dir() {
        at = at.parent()?;
    }
    let path = std::ffi::CString::new(at.as_os_str().as_encoded_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // Safety: a valid C string and a stack buffer of the right type.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let free = stat.f_bavail as u64 * stat.f_frsize as u64;
    Some(format!("{:.0} GB free there.", free as f64 / 1e9))
}

fn open_editor(ui: &Ui, product_id: &str, name: &str) {
    let (recipe, titles_dir) = {
        let model = ui.model.borrow();
        let mut recipe = model
            .recipe_for(product_id)
            .cloned()
            .unwrap_or_else(|| Recipe::blank(product_id, name));
        // Fill the executable in from the package's own manifest rather than
        // asking someone to find an .exe in a tree of thousands. Only when the
        // field is empty: a recipe that names something else names it because
        // the manifest's entry point was a launcher shim, and overwriting that
        // would undo the very thing that made the title work.
        if recipe.launch.executable.trim().is_empty() {
            if let Some(found) = model.detected_executable(&recipe) {
                recipe.launch.executable = found;
            }
        }
        (recipe, model.user_titles_dir())
    };
    let Some(titles_dir) = titles_dir else {
        ui.toasts
            .add_toast(adw::Toast::new("Nowhere to save an edit: HOME is not set"));
        return;
    };

    let window = ui.window.clone();
    let ui = ui.clone();
    editor::present(&window, recipe, titles_dir, move |path| {
        ui.model.borrow_mut().reload();
        render(&ui);
        ui.toasts.add_toast(adw::Toast::new(&format!(
            "Saved {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )));
    });
}

fn add_to_steam(ui: &Ui, product_id: &str, name: &str) {
    // Steam rewrites shortcuts.vdf from memory when it exits, so an edit made
    // underneath a running Steam is simply lost. Refusing is the only honest
    // behaviour; writing it and saying nothing would look like a bug in Steam.
    if steam::is_running() {
        ui.toasts.add_toast(adw::Toast::new(
            "Close Steam first — it overwrites its shortcuts file when it exits",
        ));
        return;
    }

    let steam_dir = ui
        .model
        .borrow()
        .paths
        .as_ref()
        .and_then(|p| p.steam_dir().map(Path::to_owned));
    let Some(steam_dir) = steam_dir else {
        ui.toasts
            .add_toast(adw::Toast::new("No Steam installation found"));
        return;
    };
    let files = steam::shortcut_files(&steam_dir);
    if files.is_empty() {
        ui.toasts.add_toast(adw::Toast::new(
            "No Steam account on this machine has a shortcuts file",
        ));
        return;
    }

    let program = cli_binary();
    let start_dir = program.parent().unwrap_or(Path::new("/")).to_path_buf();
    let shortcut = steam::Shortcut::new(name, &program, &start_dir, format!("run {product_id}"));

    // Every account, because guessing which one a person uses and getting it
    // wrong looks exactly like the feature not working.
    let mut added = 0;
    for file in &files {
        let result = steam::load(file).and_then(|mut document| {
            steam::upsert(&mut document, &shortcut)?;
            steam::save(file, &document)
        });
        match result {
            Ok(()) => added += 1,
            Err(e) => ui
                .toasts
                .add_toast(adw::Toast::new(&format!("{}: {e}", file.display()))),
        }
    }
    if added > 0 {
        ui.toasts.add_toast(adw::Toast::new(&format!(
            "Added \"{name}\" to Steam. Restart Steam to see it."
        )));
    }
}

fn account_pressed(ui: &Ui) {
    if ui.model.borrow().account.is_some() {
        account_menu(ui);
    } else {
        sign_in(ui);
    }
}

// --- background work -------------------------------------------------------

/// Guard a background task so two clicks cannot start two of them.
fn begin(ui: &Ui) -> bool {
    if *ui.busy.borrow() {
        return false;
    }
    *ui.busy.borrow_mut() = true;
    ui.spinner.start();
    true
}

fn finish(ui: &Ui) {
    *ui.busy.borrow_mut() = false;
    ui.spinner.stop();
}

fn sign_in(ui: &Ui) {
    let Some(client) = ui.model.borrow().xodus_cli() else {
        ui.toasts.add_toast(adw::Toast::new(
            "The Xodus client was not found. `ferestre doctor` says where it is looked for.",
        ));
        return;
    };
    if !begin(ui) {
        return;
    }
    ui.toasts
        .add_toast(adw::Toast::new("Signing in — the client opens a browser"));

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            account::login_command(&client)
                .stdin(std::process::Stdio::null())
                .status()
                .map_err(|e| format!("{}: {e}", client.display()))
        })
        .await;
        finish(&ui);
        match result {
            Ok(Ok(status)) if status.success() => refresh_account(&ui),
            Ok(Ok(status)) => ui
                .toasts
                .add_toast(adw::Toast::new(&format!("Sign-in exited {status}"))),
            Ok(Err(e)) => ui.toasts.add_toast(adw::Toast::new(&e)),
            Err(_) => ui.toasts.add_toast(adw::Toast::new("The client crashed")),
        }
    });
}

fn refresh_account(ui: &Ui) {
    let (client, cache_dir) = {
        let model = ui.model.borrow();
        (model.xodus_cli(), model.cache_dir().map(Path::to_owned))
    };
    let (Some(client), Some(cache_dir)) = (client, cache_dir) else {
        return;
    };
    if !begin(ui) {
        return;
    }

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            let output = account::command(&client).output().ok()?;
            if !output.status.success() {
                return None;
            }
            let account = account::parse(&String::from_utf8_lossy(&output.stdout)).ok()?;
            // Best effort: an account with no picture is still an account.
            let avatar = account::avatar(&cache_dir, &account, AVATAR_PX).ok();
            Some((account, avatar))
        })
        .await;
        finish(&ui);
        if let Ok(Some((account, avatar))) = result {
            {
                let mut model = ui.model.borrow_mut();
                model.account = Some(account);
                model.avatar = avatar;
            }
            render_account_button(&ui);
        }
    });
}

fn refresh_updates(ui: &Ui) {
    let (client, records) = {
        let model = ui.model.borrow();
        (
            model.xodus_cli(),
            model.records.values().cloned().collect::<Vec<_>>(),
        )
    };
    let Some(client) = client else { return };
    if records.is_empty() || !begin(ui) {
        return;
    }
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let checked = gio::spawn_blocking(move || {
            let mut available = BTreeMap::new();
            for record in records {
                let Some(content_id) = record.content_ids.first() else {
                    continue;
                };
                let output = std::process::Command::new(&client)
                    .args(["update", content_id, "--json"])
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()?;
                if !output.status.success() {
                    continue;
                }
                let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
                if let Some(version) = value["version"]
                    .as_str()
                    .filter(|version| !version.is_empty())
                {
                    available.insert(record.product_id.to_ascii_uppercase(), version.to_string());
                }
            }
            Some(available)
        })
        .await;
        finish(&ui);
        if let Ok(Some(available)) = checked {
            ui.model.borrow_mut().available = available;
            render(&ui);
        }
    });
}

fn refresh_library(ui: &Ui) {
    let (client, cache) = {
        let model = ui.model.borrow();
        (model.xodus_cli(), model.catalog_cache())
    };
    let (Some(client), Some(cache)) = (client, cache) else {
        ui.toasts
            .add_toast(adw::Toast::new("The Xodus client was not found"));
        return;
    };
    if !begin(ui) {
        return;
    }
    let (market, language) = ui.model.borrow().market();
    let recipe_ids: Vec<String> = ui
        .model
        .borrow()
        .recipes
        .iter()
        .map(|r| r.title.product_id.clone())
        .collect();
    ui.toasts
        .add_toast(adw::Toast::new("Asking what this account owns"));

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            let output = ferestre_core::library::library_command(&client, &Default::default())
                .stderr(std::process::Stdio::null())
                .output()
                .map_err(|e| format!("{}: {e}", client.display()))?;
            if !output.status.success() {
                return Err(format!("{} exited {}", client.display(), output.status));
            }
            let entries = ferestre_core::library::parse(&String::from_utf8_lossy(&output.stdout))
                .map_err(|e| e.to_string())?;

            // Names and art for everything that gets a row: what is owned, and
            // what has a recipe but is not owned.
            let mut ids: Vec<String> = entries
                .iter()
                .filter(|e| e.is_title())
                .map(|e| e.product_id.clone())
                .collect();
            for id in recipe_ids {
                if !ids.iter().any(|known| known.eq_ignore_ascii_case(&id)) {
                    ids.push(id);
                }
            }
            let (products, failures) = catalog::resolve(&cache, &ids, &market, &language);
            Ok((entries, products, failures))
        })
        .await;
        finish(&ui);

        match result {
            Ok(Ok((entries, products, failures))) => {
                let owned = entries.iter().filter(|e| e.is_title()).count();
                {
                    // Written before it is drawn, so the next launch opens on a
                    // library rather than on a button.
                    if let Some(state_dir) = ui
                        .model
                        .borrow()
                        .paths
                        .as_ref()
                        .map(|p| p.state_dir().to_owned())
                    {
                        let _ = ferestre_core::library::save_cache(&state_dir, &entries);
                    }
                    let mut model = ui.model.borrow_mut();
                    model.ownership = Ownership::Known(entries);
                    for product in products {
                        model
                            .catalog
                            .insert(product.product_id.to_ascii_uppercase(), product);
                    }
                    model.page = 0;
                }
                render(&ui);
                let message = if failures.is_empty() {
                    format!("This account owns {owned} games and apps")
                } else {
                    format!(
                        "{owned} games and apps; {} catalog lookups failed, so some rows show ids",
                        failures.len()
                    )
                };
                ui.toasts.add_toast(adw::Toast::new(&message));
            }
            Ok(Err(e)) => ui
                .toasts
                .add_toast(adw::Toast::new(&format!("Library: {e}"))),
            Err(_) => ui
                .toasts
                .add_toast(adw::Toast::new("Library: the client crashed")),
        }
    });
}

/// Look up names for the rows currently on screen.
///
/// A page at a time, like the art, and each id is attempted once: a product the
/// catalog does not answer for would otherwise be re-requested on every redraw,
/// which is a request loop rather than a retry.
/// Fetch the PC Game Pass listing, unless there is already one and `force` is
/// not set.
///
/// Anonymous: this asks a public catalogue what PC Game Pass includes, which
/// needs no account and grants nothing. What the account may install is decided
/// by the licence request at install time.
fn fetch_gamepass(ui: &Ui, force: bool) {
    let (paths, market, language, have) = {
        let model = ui.model.borrow();
        let (market, language) = model.market();
        (
            model.paths.as_ref().map(|p| p.state_dir().to_path_buf()),
            market,
            language,
            // Both halves, or a cached listing means the tier map is never
            // fetched: the listing survives a restart and the tier map does
            // not, so every session after the first showed 524 titles with no
            // idea which of them this account can install.
            !model.gamepass.is_empty() && !model.tiers.included.is_empty(),
        )
    };
    if have && !force {
        return;
    }
    // Not the `busy` flag: this is a background list, and holding the window's
    // sign-in and library buttons hostage to it would be out of proportion.
    if !ui.fetching_gamepass.replace(true) {
        let ui = ui.clone();
        glib::spawn_future_local(async move {
            let fetched = gio::spawn_blocking({
                let market = market.clone();
                move || {
                    let ids = gamepass::fetch(&market, &language)?;
                    // The tier map is a second request and a failure of it is
                    // not a failure of the listing: without it the section
                    // still works, it just cannot warn.
                    let tiers = gamepass::fetch_tiers(&market, &language).unwrap_or_default();
                    Ok::<_, anyhow::Error>((ids, tiers))
                }
            })
            .await;
            ui.fetching_gamepass.replace(false);
            let Ok(Ok((ids, tiers))) = fetched else {
                // Silent on purpose when there is already a listing: a failed
                // refresh of a public list is not worth a toast over the top of
                // whatever someone is doing.
                if !have {
                    ui.toasts
                        .add_toast(adw::Toast::new("Could not fetch the Game Pass catalogue"));
                }
                return;
            };
            if let Some(dir) = paths {
                let _ = gamepass::save_cache(&dir, &market, &ids, &tiers);
            }
            {
                let mut model = ui.model.borrow_mut();
                model.gamepass = ids;
                if !tiers.included.is_empty() {
                    model.tiers = tiers;
                }
            }
            if ui.model.borrow().section == Section::GamePass {
                render(&ui);
            }
        });
    }
}

fn fetch_names(ui: &Ui, product_ids: Vec<String>) {
    let (cache, market, wanted) = {
        let model = ui.model.borrow();
        let mut asked = ui.asked_names.borrow_mut();
        let wanted: Vec<String> = product_ids
            .into_iter()
            .filter(|id| asked.insert(id.to_ascii_uppercase()))
            .collect();
        (model.catalog_cache(), model.market(), wanted)
    };
    let Some(cache) = cache else { return };
    if wanted.is_empty() {
        return;
    }

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let (market, language) = market;
        // In chunks, so filling in five hundred names shows rows as they land
        // rather than after every request has finished. Each chunk is several
        // batched lookups, so this is not a request per await.
        for chunk in wanted.chunks(catalog::BATCH * 5) {
            let ids = chunk.to_vec();
            let (cache, market, language) = (cache.clone(), market.clone(), language.clone());
            let found = gio::spawn_blocking(move || {
                let (products, _failures) = catalog::resolve(&cache, &ids, &market, &language);
                products
            })
            .await;
            let Ok(found) = found else { return };
            if found.is_empty() {
                continue;
            }
            {
                let mut model = ui.model.borrow_mut();
                for product in found {
                    model
                        .catalog
                        .insert(product.product_id.to_ascii_uppercase(), product);
                }
            }
            render(&ui);
        }
    });
}

/// Fetch art for the rows currently on screen.
///
/// A page at a time, not the whole library: a hundred titles is a hundred
/// requests, and nobody is looking at page five.
fn fetch_icons(ui: &Ui, product_ids: Vec<String>) {
    let (cache, wanted) = {
        let model = ui.model.borrow();
        let wanted: Vec<(String, catalog::Product)> = product_ids
            .into_iter()
            .filter_map(|id| {
                let key = id.to_ascii_uppercase();
                model.catalog.get(&key).map(|p| (key, p.clone()))
            })
            .collect();
        (model.catalog_cache(), wanted)
    };
    let Some(cache) = cache else { return };
    if wanted.is_empty() {
        return;
    }

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let fetched = gio::spawn_blocking(move || {
            wanted
                .into_iter()
                .filter_map(|(key, product)| {
                    catalog::icon(&cache, &product, ICON_PX)
                        .ok()
                        .map(|path| (key, path))
                })
                .collect::<BTreeMap<String, PathBuf>>()
        })
        .await;
        let Ok(fetched) = fetched else { return };
        if fetched.is_empty() {
            return;
        }
        ui.model.borrow_mut().icons.extend(fetched);
        render(&ui);
    });
}

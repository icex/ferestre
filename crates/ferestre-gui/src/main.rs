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
mod state;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ferestre_core::recipe::Recipe;
use ferestre_core::{account, catalog, steam};
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
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let model = Rc::new(RefCell::new(Model::load()));

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

    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&content_header);
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
    refresh_account(&ui);

    window.present();
}

// --- rendering ------------------------------------------------------------

fn render(ui: &Ui) {
    while let Some(child) = ui.content.first_child() {
        ui.content.remove(&child);
    }

    let section = ui.model.borrow().section;
    ui.title.set_title(section.title());
    ui.search
        .set_visible(matches!(section, Section::Library | Section::Installed));
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
        Section::Library => render_rows(ui, &page, |_| true, "Nothing here yet."),
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
            .tooltip_text("ferestre install-runtime — builds a patched Proton, which takes a while")
            .build();
        install.connect_clicked(glib::clone!(
            #[strong]
            ui,
            move |_| spawn_cli_tracked(&ui, &["install-runtime"], "Building the runtime")
        ));
        row.add_suffix(&install);
    }
    group.add(&row);
    page.add(&group);

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
}

fn draw_list(model: &Model, keep: impl Fn(&LibraryRow) -> bool) -> Drawn {
    let installed = |r: &Recipe| model.is_installed(r);
    let installed_version = |r: &Recipe| model.installed_version(r);
    let rows: Vec<LibraryRow> = model::library(&Inputs {
        recipes: &model.recipes,
        runtime: model.runtime.as_ref(),
        registry: model.registry.as_ref(),
        ownership: &model.ownership,
        catalog: &model.catalog,
        records: &model.records,
        // Empty until something asks the update service what it is offering.
        // See `Inputs::available` and docs/ROADMAP.md.
        available: &model.available,
        installed: &installed,
        installed_version: &installed_version,
    })
    .into_iter()
    .filter(|row| keep(row))
    .collect();

    let matching = model::search(&rows, &model.query);
    let view = model::paginate(&matching, model.page, PER_PAGE);
    Drawn {
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
    }
}

fn render_rows(
    ui: &Ui,
    page: &adw::PreferencesPage,
    keep: impl Fn(&LibraryRow) -> bool,
    empty_message: &str,
) {
    let drawn = draw_list(&ui.model.borrow(), keep);

    let group = adw::PreferencesGroup::builder().build();
    if drawn.rows.is_empty() {
        let query = ui.model.borrow().query.clone();
        let message = if query.trim().is_empty() {
            empty_message.to_string()
        } else {
            format!("Nothing matches {query:?}")
        };
        let row = adw::ActionRow::builder().title(&message).build();
        row.add_css_class("dim-label");
        group.add(&row);
        page.add(&group);
        return;
    }

    for row in &drawn.rows {
        group.add(&library_row(ui, row));
    }
    page.add(&group);

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

    if !drawn.want_icons.is_empty() {
        fetch_icons(ui, drawn.want_icons);
    }
}

fn library_row(ui: &Ui, row: &LibraryRow) -> adw::ActionRow {
    let action_row = adw::ActionRow::builder()
        .title(&row.name)
        .subtitle(&row.subtitle)
        .subtitle_lines(2)
        .tooltip_text(&row.product_id)
        .build();

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

    // A shortcut needs something to point at, which means a recipe.
    if row.has_recipe {
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

    let button = gtk::Button::builder()
        .label(row.action.label())
        .valign(gtk::Align::Center)
        .css_classes(["suggested-action"])
        .build();
    button.set_sensitive(row.action.is_enabled());

    match &row.action {
        Action::Blocked(reason) => button.set_tooltip_text(Some(reason)),
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
            let what = match row.action {
                Action::Update => format!("Updating {}", row.name),
                _ => format!("Installing {}", row.name),
            };
            button.connect_clicked(glib::clone!(
                #[strong]
                ui,
                move |_| {
                    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                    if waits {
                        spawn_cli_tracked(&ui, &borrowed, &what);
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
            Some(_) => "Load what this account owns",
            None => "Sign in through the Xodus client",
        }));
}

// --- actions ---------------------------------------------------------------

fn cli_binary() -> PathBuf {
    let exe = std::env::current_exe().ok();
    model::cli_binary(exe.as_deref().and_then(Path::parent), &|p| p.is_file())
}

/// Run `ferestre` and leave it running. Output goes where the CLI already sends it
/// -- a per-title log file -- rather than into a widget nobody would read while
/// a game is starting.
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
fn spawn_cli_tracked(ui: &Ui, args: &[&str], what: &str) {
    if !begin(ui) {
        ui.toasts
            .add_toast(adw::Toast::new("Something is already running"));
        return;
    }
    let program = cli_binary();
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
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

fn open_editor(ui: &Ui, product_id: &str, name: &str) {
    let (recipe, titles_dir) = {
        let model = ui.model.borrow();
        let recipe = model
            .recipe_for(product_id)
            .cloned()
            .unwrap_or_else(|| Recipe::blank(product_id, name));
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
        refresh_library(ui);
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

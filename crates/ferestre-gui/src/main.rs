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
mod accounts_ui;
mod app_updates;
mod artwork;
mod cancel;
mod client_check;
mod controller;
mod deadline;
mod editor;
mod folder_picker;
mod handheld;
mod launch_state;
mod model;
mod packagekit;
mod presence_ui;
mod progress;
mod queue_ui;
mod state;
mod stop;
mod text_input;
mod window_activation;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use ferestre_core::recipe::Recipe;
use ferestre_core::{account, accounts, catalog, gamepass, library, steam};
use model::{Action, Inputs, LibraryRow, Ownership};
use state::{Model, Section, AVATAR_PX, PER_PAGE};

const APP_ID: &str = "io.github.icex.ferestre";

/// Only what Adwaita's own classes do not cover. Colours are Adwaita's named
/// ones, so dark mode and accent changes follow without a second stylesheet.
const STYLE: &str = "
@define-color window_bg_color #1b1d21;
@define-color window_fg_color #f4f4f5;
@define-color view_bg_color #1b1d21;
@define-color view_fg_color #f4f4f5;
@define-color card_bg_color #25282d;
@define-color card_fg_color #f4f4f5;
@define-color sidebar_bg_color #25282d;
@define-color sidebar_fg_color #f4f4f5;
@define-color headerbar_bg_color #1b1d21;
@define-color headerbar_fg_color #f4f4f5;
@define-color accent_bg_color #107c10;
@define-color accent_fg_color #ffffff;
@define-color accent_color #8bd77b;
window.ferestre-window, .ferestre-content, .ferestre-content > contents {
  background-color: #1b1d21;
  color: #f4f4f5;
}
.ferestre-sidebar, .ferestre-sidebar > contents, .ferestre-sidebar headerbar {
  background-color: #25282d;
  color: #f4f4f5;
}
.ferestre-content headerbar { background-color: #1b1d21; color: #f4f4f5; box-shadow: none; }
.ferestre-window label { color: inherit; }
.ferestre-window .dim-label { color: #b3b6bd; opacity: 1; }
.ferestre-window .navigation-sidebar { background-color: #25282d; color: #f4f4f5; }
.ferestre-window .navigation-sidebar row:selected { background-color: #393d45; color: #ffffff; }
.ferestre-window .navigation-sidebar row:hover { background-color: #30343b; }
.ferestre-window .navigation-sidebar row { font-size: 15px; font-weight: 600; }
.ferestre-window entry, .ferestre-window searchentry { background-color: #292c32; color: #f4f4f5; border: 1px solid #565b66; border-radius: 24px; }
.ferestre-window button { color: #f4f4f5; }
.ferestre-window button.suggested-action { background-color: #107c10; color: #ffffff; }
.game-tile { background-color: #25282d; border-radius: 7px; }
button.game-tile { padding: 0; border: 0; background-color: #25282d; color: #f4f4f5; box-shadow: none; border-radius: 7px; }
button.game-tile:hover { background-color: #30343b; }
button.game-tile:focus-visible { outline: 3px solid white; outline-offset: 4px; }
.game-tile picture { border-radius: 7px 7px 0 0; }
.game-tile .game-title { font-size: 14px; font-weight: 700; }
.game-tile .game-meta { font-size: 12px; color: #b3b6bd; }
.game-tile .game-running { color: #8bd77b; font-weight: 700; }
.game-launch-overlay { background-color: alpha(#080b0f, 0.72); border-radius: 7px; }
.game-launch-overlay .game-overlay-state { font-size: 18px; font-weight: 600; color: #f4f4f5; }
.game-launch-overlay .game-overlay-action { font-size: 20px; font-weight: 700; color: #ffffff; border: 1px solid alpha(#ffffff, 0.65); border-radius: 22px; padding: 7px 22px; }
.game-update-badge { background-color: #f2ca64; color: #171b20; border-radius: 5px; padding: 5px 7px; }
.game-update-badge label { color: #171b20; font-size: 11px; font-weight: 700; }
.game-update-badge image { color: #171b20; }
.game-more > button { background-color: alpha(#111317,0.85); border-radius: 5px; }
.sidebar-recents button { padding: 6px; background: transparent; box-shadow: none; border-radius: 6px; }
.sidebar-recents button:hover { background-color: #343840; }
.sidebar-footer button { background: transparent; box-shadow: none; }
.library-tabs button { background: transparent; border-radius: 0; box-shadow: none; border-bottom: 3px solid transparent; }
.library-tabs button.active { border-bottom-color: white; font-weight: bold; }
.filter-chip { border-radius: 18px; background-color: #30343b; }
.filter-chip.active { background-color: #e4e5e7; color: #1b1d21; }
.feature-card { border-radius: 7px; background-color: #25282d; }
.feature-caption { padding: 55px 18px 18px; color: white; background: linear-gradient(to bottom, transparent, alpha(black,0.95)); }
.gamepass-strip { background: linear-gradient(90deg, #15443c, #163b49); color: #f4f4f5; border: 1px solid #166550; border-radius: 7px; }

.access-chip {
  padding: 4px 10px;
  border-radius: 999px;
  color: @warning_color;
  background-color: alpha(@warning_color, 0.12);
}
row.library-row {
  padding-top: 8px;
  padding-bottom: 8px;
}
row.library-row label.title {
  font-weight: bold;
}
.role-badge {
  padding: 2px 8px;
  border-radius: 999px;
  font-size: smaller;
  font-weight: bold;
  color: @accent_color;
  background-color: alpha(@accent_color, 0.12);
}
.role-badge.player {
  color: @success_color;
  background-color: alpha(@success_color, 0.12);
}
.presence-dot {
  min-width: 6px;
  min-height: 6px;
  border-radius: 999px;
  border: 2px solid @sidebar_bg_color;
  background-color: alpha(@window_fg_color, 0.4);
}
.presence-dot.online {
  background-color: @success_color;
}
window.ferestre-window button, window.ferestre-window menubutton > button {
  min-height: 44px;
  min-width: 44px;
}
window.ferestre-window entry { min-height: 44px; }
window.ferestre-window button:focus-visible,
window.ferestre-window row:focus-visible,
window.ferestre-window flowboxchild:focus-visible {
  outline: 3px solid @accent_color;
  outline-offset: 3px;
}
.navigation-sidebar row { min-height: 52px; border-radius: 10px; margin: 3px 8px; }
.game-grid { background: transparent; }
.game-grid flowboxchild { padding: 0; border-radius: 16px; }
.game-card { border-radius: 16px; background: @card_bg_color; box-shadow: 0 2px 10px alpha(black,0.12); }
.game-card picture { border-radius: 16px 16px 0 0; }
.game-card .game-actions { margin: 0 14px 14px 14px; }
.library-hero { border-radius: 18px; padding: 28px; color: white; background: linear-gradient(120deg, #124b3a, #153b40 55%, #26343e); }
.controller-hints { padding: 10px 16px; font-size: 14px; background: #202328; }
window.controller-navigation button:focus, window.controller-navigation row:focus { outline: 4px solid #8bd77b; outline-offset: 3px; }
window.controller-navigation button.game-tile:focus { background-color: #343a40; }
.controller-navigation .game-grid flowboxchild:focus-visible { outline: none; }
.fullscreen-navigation { padding: 4px 20px 8px; }
.fullscreen-navigation button { background: transparent; border-radius: 8px; font-weight: 700; font-size: 16px; }
.fullscreen-navigation button.active { background: #393d45; border-bottom: 3px solid #8bd77b; }
window.fullscreen-browser button, window.fullscreen-browser menubutton > button { min-height: 48px; min-width: 48px; }
.fullscreen-browser .game-title { font-size: 16px; }
.fullscreen-browser .game-meta { font-size: 14px; }
.fullscreen-browser button:focus-visible, .fullscreen-browser button.game-tile:focus { outline: 4px solid #8bd77b; outline-offset: 3px; }
.fullscreen-browser .game-grid flowboxchild:focus-visible { outline: none; }

";

/// A toast that shows its text as written.
///
/// Since libadwaita 1.4 a toast's title is Pango markup unless told otherwise,
/// and game names and error text are not markup: "Command & Conquer" or a
/// "<DIR>" in an error drew an empty toast, so Play, Stop and Install went
/// unconfirmed. Every toast is made here, or with `use_markup(false)`.
fn toast(text: impl AsRef<str>) -> adw::Toast {
    adw::Toast::builder()
        .title(text.as_ref())
        .use_markup(false)
        .build()
}

/// A row whose title and subtitle are shown as written, for the same reason.
fn plain_row(title: &str, subtitle: Option<&str>) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_use_markup(false);
    row.set_title(title);
    if let Some(subtitle) = subtitle {
        row.set_subtitle(subtitle);
    }
    row
}

/// Everything a callback needs to redraw the window, in one place so it can
/// hold one clone instead of nine.
#[derive(Clone)]
struct Ui {
    model: Rc<RefCell<Model>>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    content: gtk::Box,
    scroll: gtk::ScrolledWindow,
    account_button: gtk::Button,
    title: adw::WindowTitle,
    search: gtk::SearchEntry,
    spinner: gtk::Spinner,
    /// Set while a background task runs, so a second click cannot queue a
    /// duplicate sign-in or a duplicate library fetch.
    busy: Rc<RefCell<Option<String>>>,
    wait_on_close: Rc<RefCell<bool>>,
    /// What the activity panel says about closing the window during a task
    /// that does not hold it open. Each task knows what a close costs it; one
    /// that says nothing gets a sentence that promises nothing.
    close_note: Rc<Cell<Option<&'static str>>>,
    activity: gtk::Box,
    activity_title: gtk::Label,
    activity_detail: gtk::Label,
    keep_open: gtk::Button,
    closing: Rc<RefCell<bool>>,
    close_dialog_open: Rc<RefCell<bool>>,
    /// Closing was confirmed with titles still running.
    close_while_running: Rc<Cell<bool>>,
    download_control: Rc<RefCell<Option<cancel::DownloadControl>>>,
    /// Product ids whose name has been looked up, so a lookup that came back
    /// with nothing is not repeated on every redraw.
    asked_names: Rc<RefCell<std::collections::BTreeSet<String>>>,
    artwork: Rc<RefCell<artwork::Queue>>,
    gamepass_art_requested: Rc<Cell<bool>>,
    /// Set while the Game Pass listing is being fetched, so opening the section
    /// twice does not ask twice.
    fetching_gamepass: Rc<RefCell<bool>>,
    fetching_names: Rc<Cell<bool>>,
    catalog_generation: Rc<Cell<u64>>,
    fetching_entitlements: Rc<RefCell<bool>>,
    /// Background checks that failed in a row, and when the next may run on
    /// its own. Offline, or signed out, a check every minute was a client
    /// process and a keyring every minute for nothing.
    entitlement_failures: Rc<Cell<u32>>,
    entitlements_retry_at: Rc<Cell<Option<std::time::Instant>>>,
    gamepass_failures: Rc<Cell<u32>>,
    gamepass_retry_at: Rc<Cell<Option<std::time::Instant>>>,
    /// Whether a failed Game Pass fetch has been reported since the last
    /// Refresh, so a redraw does not queue the same toast again.
    gamepass_reported: Rc<Cell<bool>>,
    /// Set while the update check runs. Its own flag rather than `busy`: it is
    /// background work, and it must never hold Install, Remove or Sign in.
    checking_updates: Rc<Cell<bool>>,
    /// An account refresh that could not start because another task held the
    /// window. Owed until it runs, like `entitlements_owed`.
    account_owed: Rc<Cell<bool>>,
    /// A forced ownership check that could not start because another task held
    /// the window. Owed until it runs: the next finish or timer tick starts it,
    /// so a Refresh pressed during a background check is not lost for a day.
    entitlements_owed: Rc<Cell<bool>>,
    /// The bar along the bottom, and the download it is describing.
    download: DownloadBar,
    /// Titles this window started that have not exited, by product id, each
    /// mapped to the process group to signal. Deliberately not the `busy` flag:
    /// a game runs for hours and must not hold the window's other work.
    running: Rc<RefCell<BTreeMap<String, stop::Launch>>>,
    running_servers: Rc<RefCell<BTreeMap<String, (PathBuf, PathBuf)>>>,
    /// Each running launch's stop channel, through which an image view asks
    /// a title in its namespaces to close (see [`stop::Channel`]).
    running_channels: Rc<RefCell<BTreeMap<String, std::sync::Arc<stop::Channel>>>>,
    /// Says the Xbox client cannot start, for as long as that is true.
    banner: adw::Banner,
    update_banner: adw::Banner,
    app_updates: Rc<RefCell<app_updates::State>>,
    /// Why the client cannot start, when the last check found it could not.
    client_blocked: Rc<RefCell<Option<client_check::Blocked>>>,
    /// A PackageKit install asked for while another task held the window, and
    /// what to carry on with after it. Started by the next finish.
    install_owed: Rc<Cell<Option<client_check::Owed>>>,
    /// Cancels the PackageKit install in progress, from the activity panel.
    package_cancel: Rc<RefCell<Option<packagekit::Cancel>>>,
    cancel_package: gtk::Button,
    /// Online status: the Player's switch, what Xbox shows, and the runners
    /// this window's launches report.
    presence: Rc<RefCell<presence_ui::State>>,
    controller_hints: gtk::Label,
    navigation_hint: gtk::Label,
    controller_family: Rc<Cell<controller::Family>>,
    controller_connected: Rc<Cell<bool>>,
    last_game_focus: Rc<RefCell<Option<String>>>,
    game_actions: Rc<RefCell<BTreeMap<String, String>>>,
    fullscreen_nav: gtk::Box,
    recent_games: gtk::Box,
    sidebar: gtk::ListBox,
    split: adw::NavigationSplitView,
    install_queue: Rc<RefCell<ferestre_core::transfers::Queue>>,
    queue_error: Rc<RefCell<Option<String>>>,
}

fn main() -> glib::ExitCode {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--resume-x11") {
        let pids: Result<Vec<u32>, _> = args[2..].iter().map(|s| s.parse()).collect();
        return if pids.is_ok_and(|pids| !pids.is_empty() && window_activation::x11_resume(&pids)) {
            glib::ExitCode::SUCCESS
        } else {
            glib::ExitCode::FAILURE
        };
    }
    let application_id = std::env::var("FERESTRE_APPLICATION_ID")
        .ok()
        .filter(|id| gio::Application::id_is_valid(id))
        .unwrap_or_else(|| APP_ID.into());
    let app = adw::Application::builder()
        .application_id(&application_id)
        .build();
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
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    let model = Rc::new(RefCell::new(Model::load()));

    if let Some(display) = gdk::Display::default() {
        add_source_icon_path(&display);
        let css = gtk::CssProvider::new();
        css.load_from_data(STYLE);
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    // Without this the shell has no icon to draw and falls back to a generated
    // letter tile. The name is the application id, which is what the packaging
    // installs the SVG as. Set here rather than in `main`, because it goes
    // through GTK and GTK is not initialised until the application activates.
    gtk::Window::set_default_icon_name(APP_ID);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Ferestre")
        .default_width(1280)
        .default_height(800)
        .css_classes(["ferestre-window"])
        .build();

    // --- sidebar ----------------------------------------------------------

    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    for section in Section::NAV {
        let row = adw::ActionRow::builder().title(section.title()).build();
        row.set_widget_name(&format!("navigation-{}", section.title()));
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

    account_button.set_widget_name("navigation-account");
    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    sidebar_box.set_margin_top(18);
    sidebar_box.append(&account_button);
    sidebar_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    sidebar_box.append(
        &gtk::ScrolledWindow::builder()
            .vexpand(false)
            .min_content_height(190)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&sidebar)
            .build(),
    );

    let navigation_hint = gtk::Label::builder()
        .label("↑ / ↓ Choose a section   → Games")
        .wrap(true)
        .margin_start(12)
        .margin_end(12)
        .css_classes(["controller-hints"])
        .visible(false)
        .build();
    sidebar_box.append(&navigation_hint);

    let recent_games = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(12)
        .margin_end(12)
        .css_classes(["sidebar-recents"])
        .build();
    let recent_scroll = gtk::ScrolledWindow::builder()
        .child(&recent_games)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    sidebar_box.append(&recent_scroll);
    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(12)
        .css_classes(["sidebar-footer"])
        .build();
    let downloads_link = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("folder-download-symbolic")
                .label("Install Queue")
                .build(),
        )
        .halign(gtk::Align::Fill)
        .build();
    let settings_link = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("emblem-system-symbolic")
                .label("Settings")
                .build(),
        )
        .halign(gtk::Align::Fill)
        .build();
    downloads_link.set_widget_name("navigation-downloads");
    settings_link.set_widget_name("navigation-settings");
    let fullscreen_link = gtk::Button::with_label("Fullscreen");
    fullscreen_link.set_widget_name("navigation-fullscreen");
    let help_link = gtk::Button::with_label("Controller help");
    help_link.set_widget_name("navigation-help");
    footer.append(&downloads_link);
    footer.append(&settings_link);
    footer.append(&fullscreen_link);
    footer.append(&help_link);
    sidebar_box.append(&footer);
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_css_class("ferestre-sidebar");
    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.set_show_start_title_buttons(false);
    sidebar_header.set_show_end_title_buttons(false);
    sidebar_view.set_content(Some(&sidebar_box));

    // --- content ----------------------------------------------------------

    let title = adw::WindowTitle::new("Ferestre", "");
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search for games and more")
        .width_chars(12)
        .max_width_chars(32)
        .hexpand(true)
        .height_request(44)
        .build();
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Refresh library, subscriptions and catalogue"));
    // Older GTK versions give Spinner a presentation role and omit its tooltip
    // from the accessibility tree. Expose the activity consistently there too.
    let spinner = gtk::Spinner::builder()
        .accessible_role(gtk::AccessibleRole::ProgressBar)
        .build();

    let content_header = adw::HeaderBar::new();
    content_header.set_title_widget(Some(&search));
    content_header.set_decoration_layout(Some(":minimize,maximize,close"));
    let back = gtk::Button::from_icon_name("go-previous-symbolic");
    back.set_tooltip_text(Some("Back to Home"));
    content_header.pack_start(&back);
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

    let activity = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(18)
        .margin_end(18)
        .margin_top(10)
        .margin_bottom(10)
        .visible(false)
        .build();
    let activity_title = gtk::Label::builder().xalign(0.0).wrap(true).build();
    activity_title.add_css_class("heading");
    let activity_detail = gtk::Label::builder().xalign(0.0).wrap(true).build();
    activity_detail.add_css_class("dim-label");
    let keep_open = gtk::Button::builder()
        .label("Keep Window Open")
        .halign(gtk::Align::Start)
        .visible(false)
        .build();
    let cancel_package = gtk::Button::builder()
        .label("Cancel Installation")
        .halign(gtk::Align::Start)
        .visible(false)
        .build();
    activity.append(&activity_title);
    activity.append(&activity_detail);
    activity.append(&keep_open);
    activity.append(&cancel_package);

    // Under the header and outside the content, like the activity panel: it
    // has to survive every redraw, and it is about the whole window.
    let banner = adw::Banner::builder()
        .use_markup(false)
        .revealed(false)
        .visible(false)
        .build();

    let update_banner = adw::Banner::builder()
        .button_label("Update")
        .use_markup(false)
        .revealed(false)
        .visible(false)
        .build();

    let content_view = adw::ToolbarView::new();
    content_view.add_css_class("ferestre-content");
    content_view.add_top_bar(&content_header);
    content_view.add_top_bar(&banner);
    content_view.add_top_bar(&update_banner);
    content_view.add_bottom_bar(&activity);
    content_view.add_bottom_bar(&download.root);
    let controller_hints = gtk::Label::builder()
        .label("D-pad Move   ·   A Select   ·   B Back   ·   Y Search   ·   View Navigation   ·   Menu Options")
        .wrap(true)
        .css_classes(["caption", "dim-label", "controller-hints"])
        .visible(false)
        .build();
    content_view.add_bottom_bar(&controller_hints);
    content_view.set_content(Some(&toasts));

    let split = adw::NavigationSplitView::builder()
        .min_sidebar_width(250.0)
        .max_sidebar_width(270.0)
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
    let breakpoint = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 860sp").expect("valid breakpoint"),
    );
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    breakpoint.add_setter(&title, "visible", Some(&false.to_value()));
    window.add_breakpoint(breakpoint);
    split.set_show_content(true);
    let navigation = gtk::Button::from_icon_name("sidebar-show-symbolic");
    navigation.set_tooltip_text(Some("Show navigation"));
    navigation.update_property(&[gtk::accessible::Property::Label("Show navigation")]);
    split
        .bind_property("collapsed", &navigation, "visible")
        .sync_create()
        .build();
    let split_back = split.clone();
    navigation.connect_clicked(move |_| split_back.set_show_content(false));
    content_header.pack_start(&navigation);
    scroll.set_kinetic_scrolling(true);
    window.set_size_request(360, 480);
    window.set_content(Some(&split));

    let queue = model
        .borrow()
        .paths
        .as_ref()
        .map(|p| ferestre_core::transfers::Queue::load(p.state_dir()))
        .unwrap_or_else(|| Err(anyhow::anyhow!("State directory unavailable")));
    let (install_queue, queue_error) = match queue {
        Ok(queue) => (queue, None),
        Err(error) => (Default::default(), Some(error.to_string())),
    };
    let ui = Ui {
        model,
        controller_hints,
        navigation_hint,
        controller_family: Rc::new(Cell::new(controller::Family::default())),
        controller_connected: Rc::new(Cell::new(false)),
        last_game_focus: Rc::new(RefCell::new(None)),
        game_actions: Rc::new(RefCell::new(BTreeMap::new())),
        fullscreen_nav: gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .halign(gtk::Align::Center)
            .css_classes(["fullscreen-navigation"])
            .build(),
        recent_games,
        sidebar: sidebar.clone(),
        split: split.clone(),
        install_queue: Rc::new(RefCell::new(install_queue)),
        queue_error: Rc::new(RefCell::new(queue_error)),
        window: window.clone(),
        toasts,
        content,
        scroll,
        account_button: account_button.clone(),
        title,
        search: search.clone(),
        spinner,
        busy: Rc::new(RefCell::new(None)),
        wait_on_close: Rc::new(RefCell::new(false)),
        close_note: Rc::new(Cell::new(None)),
        activity,
        activity_title,
        activity_detail,
        keep_open: keep_open.clone(),
        closing: Rc::new(RefCell::new(false)),
        close_dialog_open: Rc::new(RefCell::new(false)),
        close_while_running: Rc::new(Cell::new(false)),
        download_control: Rc::new(RefCell::new(None)),
        running: Rc::new(RefCell::new(BTreeMap::new())),
        running_servers: Rc::new(RefCell::new(BTreeMap::new())),
        running_channels: Rc::new(RefCell::new(BTreeMap::new())),
        asked_names: Rc::new(RefCell::new(std::collections::BTreeSet::new())),
        artwork: Rc::new(RefCell::new(artwork::Queue::default())),
        gamepass_art_requested: Rc::new(Cell::new(false)),
        fetching_gamepass: Rc::new(RefCell::new(false)),
        fetching_names: Rc::new(Cell::new(false)),
        catalog_generation: Rc::new(Cell::new(0)),
        fetching_entitlements: Rc::new(RefCell::new(false)),
        entitlements_owed: Rc::new(Cell::new(false)),
        entitlement_failures: Rc::new(Cell::new(0)),
        entitlements_retry_at: Rc::new(Cell::new(None)),
        gamepass_failures: Rc::new(Cell::new(0)),
        gamepass_retry_at: Rc::new(Cell::new(None)),
        gamepass_reported: Rc::new(Cell::new(false)),
        checking_updates: Rc::new(Cell::new(false)),
        account_owed: Rc::new(Cell::new(false)),
        download: download.clone(),
        banner: banner.clone(),
        update_banner: update_banner.clone(),
        app_updates: Rc::new(RefCell::new(app_updates::State::capture())),
        client_blocked: Rc::new(RefCell::new(None)),
        install_owed: Rc::new(Cell::new(None)),
        package_cancel: Rc::new(RefCell::new(None)),
        cancel_package: cancel_package.clone(),
        presence: Rc::new(RefCell::new(Default::default())),
    };

    let fullscreen_window = window.clone();
    fullscreen_link.connect_clicked(move |_| {
        if fullscreen_window.is_fullscreen() {
            fullscreen_window.unfullscreen();
        } else {
            fullscreen_window.fullscreen();
        }
    });
    let help_ui = ui.clone();
    help_link.connect_clicked(move |_| controller::show_help(&help_ui));
    downloads_link.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| select_section(&ui, Section::Downloads)
    ));
    settings_link.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| select_section(&ui, Section::Runtime)
    ));
    back.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| select_section(&ui, Section::Home)
    ));
    ui.download.pause.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| {
            let paused = ui
                .download_control
                .borrow()
                .as_ref()
                .is_some_and(|c| c.paused());
            queue_ui::pause(&ui, !paused);
        }
    ));
    ui.download.cancel.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |button| {
            if let Some(control) = ui.download_control.borrow().as_ref() {
                control.stop();
            }
            button.set_sensitive(false);
            show_activity(&ui);
        }
    ));
    window.connect_close_request(glib::clone!(
        #[strong]
        ui,
        move |_| request_close(&ui)
    ));
    keep_open.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| {
            *ui.closing.borrow_mut() = false;
            show_activity(&ui);
        }
    ));
    cancel_package.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |button| {
            if let Some(cancel) = ui.package_cancel.borrow().as_ref() {
                cancel.cancel();
            }
            button.set_sensitive(false);
            show_activity(&ui);
        }
    ));

    // Activation and selection both, because they are not the same event:
    // clicking selects, but Enter and an assistive technology activate, and a
    // sidebar that only answers the mouse is a sidebar half the people cannot
    // use.
    sidebar.connect_row_activated(glib::clone!(
        #[strong]
        ui,
        move |_, row| {
            if let Some(section) = Section::NAV.get(row.index() as usize) {
                select_section(&ui, *section);
            }
        }
    ));
    sidebar.connect_row_selected(glib::clone!(
        #[strong]
        ui,
        move |_, row| {
            let Some(row) = row else { return };
            let index = row.index().max(0) as usize;
            let Some(section) = Section::NAV.get(index).copied() else {
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
            ui.split.set_show_content(true);
            ui.scroll.vadjustment().set_value(0.0);
            render(&ui);
            if section == Section::GamePass {
                refresh_entitlements(&ui, false);
            }
            refresh_updates(&ui, false);
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
            ui.scroll.vadjustment().set_value(0.0);
            render(&ui);
        }
    ));

    refresh.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| {
            ui.model.borrow_mut().reload();
            ui.catalog_generation
                .set(ui.catalog_generation.get().wrapping_add(1));
            ui.asked_names.borrow_mut().clear();
            artwork::refresh(&ui);
            ui.gamepass_reported.set(false);
            render(&ui);
            // Reload means "check again", and the Game Pass catalogue is a
            // thing that changes without anything here doing so.
            fetch_gamepass(&ui, true);
            refresh_entitlements(&ui, true);
            refresh_updates(&ui, true);
            client_check::check(&ui, None, false);
            app_updates::refresh(&ui, true);
        }
    ));

    account_button.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| account_pressed(&ui)
    ));
    banner.connect_button_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| client_check::banner_pressed(&ui)
    ));
    update_banner.connect_button_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| app_updates::update(&ui)
    ));

    // Coming back from a terminal or a software centre is when the library
    // most likely appeared.
    window.connect_is_active_notify(glib::clone!(
        #[strong]
        ui,
        move |window| {
            if window.is_active() {
                client_check::check_if_blocked(&ui);
            }
        }
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

    handheld::install(&ui, &content_view, &content_header, &back);
    window.present();
    controller::install(&ui);
    app_updates::refresh(&ui, false);
    let update_checks = ui.clone();
    glib::timeout_add_seconds_local(60, move || {
        app_updates::refresh(&update_checks, false);
        glib::ControlFlow::Continue
    });
    client_check::check(&ui, None, false);
    refresh_entitlements(&ui, false);
    fetch_gamepass(&ui, false);
    // Tier upgrades happen outside this window. The listing is reused for a
    // day (LIBRARY_MAX_AGE), so this picks one up once the saved snapshot
    // expires, or sooner when Refresh was pressed while something else was
    // running. It never takes the download's busy flag or touches its process.
    glib::timeout_add_seconds_local(
        60,
        glib::clone!(
            #[weak]
            window,
            #[strong]
            ui,
            #[upgrade_or]
            glib::ControlFlow::Break,
            move || {
                if !window.is_visible() {
                    return glib::ControlFlow::Break;
                }
                refresh_entitlements(&ui, false);
                refresh_updates(&ui, false);
                // Xbox is asked every five minutes, not every minute.
                if let Some(probe) = presence_ui::due(&ui) {
                    presence_ui::refresh(&ui, probe);
                }
                glib::ControlFlow::Continue
            }
        ),
    );
}

// --- rendering ------------------------------------------------------------

fn select_section(ui: &Ui, section: Section) {
    let nav = if matches!(
        section,
        Section::Installed | Section::Updates | Section::Downloads
    ) {
        Section::Library
    } else {
        section
    };
    if let Some(index) = Section::NAV.iter().position(|s| *s == nav) {
        if let Some(row) = ui.sidebar.row_at_index(index as i32) {
            ui.sidebar.select_row(Some(&row));
        }
    } else {
        // Settings is outside the primary navigation. Clear the old selection
        // so returning to the same primary row also emits selection changes.
        ui.sidebar.unselect_all();
    }
    if ui.model.borrow().section != section {
        {
            let mut model = ui.model.borrow_mut();
            model.section = section;
            model.page = 0;
        }
        ui.scroll.vadjustment().set_value(0.0);
        render(ui);
    }
    ui.split.set_show_content(true);
}

fn named_widget(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        if let Some(found) = named_widget(&widget, name) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

fn render(ui: &Ui) {
    app_updates::render(ui);
    handheld::render_navigation(ui);
    ui.game_actions.borrow_mut().clear();
    // A clicked button keeps the focus, and every title action redraws the
    // row it sits on. Destroying the focused widget makes GTK hand the focus to
    // the first widget left in the rebuilt list after the next paint, and the
    // viewport scrolls to the top to show it. Let go of the focus first, so the
    // library stays where the person was looking. (Named trait calls: RootExt
    // has a `focus` and `set_focus` of its own.)
    let window = &ui.window;
    let focus_name = GtkWindowExt::focus(window)
        .filter(|w| w.is_ancestor(&ui.content))
        .map(|w| w.widget_name().to_string())
        .filter(|name| {
            name.starts_with("game-") || name.starts_with("queue-") || name.starts_with("recent-")
        });
    if GtkWindowExt::focus(window).is_some_and(|focus| focus.is_ancestor(&ui.content)) {
        GtkWindowExt::set_focus(window, None::<&gtk::Widget>);
    }
    while let Some(child) = ui.content.first_child() {
        ui.content.remove(&child);
    }

    let section = ui.model.borrow().section;
    if section == Section::GamePass {
        ui.gamepass_art_requested.set(true);
    }
    ui.title.set_title(section.title());
    ui.search.set_visible(matches!(
        section,
        Section::Home | Section::Library | Section::GamePass | Section::Installed
    ));
    render_account_button(ui);
    render_recents(ui);

    // PreferencesPage owns a second ScrolledWindow. Recreating it for a
    // title action resets that inner viewport to zero. Keep one persistent
    // outer viewport and put the preference groups in ordinary content.
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(if ui.window.is_fullscreen() { 16 } else { 24 })
        .margin_top(if ui.window.is_fullscreen() { 16 } else { 24 })
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    page.set_accessible_role(gtk::AccessibleRole::Region);
    page.update_property(&[gtk::accessible::Property::Label(&format!(
        "{} page",
        section.title()
    ))]);
    let clamp = adw::Clamp::builder()
        .maximum_size(1200)
        .tightening_threshold(1000)
        .child(&page)
        .build();
    ui.content.append(&clamp);

    if let Some(problem) = ui.model.borrow().problem.clone() {
        let group = adw::PreferencesGroup::builder().title("Not ready").build();
        // An error names files and quotes their text, which is not markup.
        let row = plain_row(&problem, None);
        row.set_title_lines(4);
        row.add_css_class("error");
        if ui.model.borrow().roles_error.is_some() {
            let reset = gtk::Button::builder()
                .label("Reset")
                .valign(gtk::Align::Center)
                .tooltip_text(
                    "Set the unreadable account roles file aside and choose accounts again",
                )
                .build();
            reset.connect_clicked(glib::clone!(
                #[strong]
                ui,
                move |_| reset_account_roles(&ui)
            ));
            row.add_suffix(&reset);
        }
        group.add(&row);
        page.append(&group);
    }

    if !ui.window.is_fullscreen()
        && matches!(
            section,
            Section::Library | Section::Installed | Section::Updates | Section::Downloads
        )
    {
        render_library_tabs(ui, &page);
    }
    match section {
        Section::Runtime => render_runtime(ui, &page),
        Section::Downloads => queue_ui::render_queue(ui, &page),
        Section::Home | Section::Library => {
            let show_all = ui.model.borrow().show_unsupported;
            render_rows(
                ui,
                &page,
                move |row| show_all || !row.known_uninstallable(),
                "Nothing here yet.",
            );
        }
        Section::GamePass => {
            // A missing catalogue is fetched when this section opens too.
            fetch_gamepass(ui, false);
            render_gamepass(ui, &page)
        }
        Section::Installed => {
            render_rows(
                ui,
                &page,
                |row| row.installed,
                "Nothing is installed yet. Find a title in the library and install it.",
            );
        }
        Section::Updates => {
            render_rows(
                ui,
                &page,
                |row| row.update == Some(true),
                "Everything installed is up to date.",
            );
        }
    }
    preload_gamepass_art(ui);
    if let Some(name) = focus_name {
        let window = ui.window.clone();
        glib::idle_add_local_once(move || {
            if GtkWindowExt::focus(&window).is_none() {
                if let Some(widget) = named_widget(window.upcast_ref(), &name) {
                    widget.grab_focus();
                }
            }
        });
    }
}

/// The Installed list's "Add All to Steam", for every installed title on
/// every page, whatever the search shows.
fn add_all_to_steam_button(ui: &Ui, titles: Vec<(String, String)>) -> gtk::Button {
    let button = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("Add All to Steam")
                .build(),
        )
        .tooltip_text("Add a Steam shortcut for every installed game")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let ui = ui.clone();
    button.connect_clicked(move |_| confirm_add_all_to_steam(&ui, titles.clone()));
    button
}

/// Set an unreadable `account-roles.json` aside and start over without roles.
fn reset_account_roles(ui: &Ui) {
    let config = ui
        .model
        .borrow()
        .paths
        .as_ref()
        .map(|p| p.config_dir().to_path_buf());
    let Some(config) = config else { return };
    match state::set_aside_roles(&config) {
        Ok(aside) => {
            ui.model.borrow_mut().reload();
            render(ui);
            ui.toasts.add_toast(toast(format!(
                "Account roles were reset. The old file was kept as {}.",
                aside.display()
            )));
            refresh_account(ui);
        }
        Err(error) => ui
            .toasts
            .add_toast(toast(format!("Could not reset the account roles: {error}"))),
    }
}

fn render_runtime(ui: &Ui, page: &gtk::Box) {
    let packaged = std::env::var("FERESTRE_PACKAGE_VERSION").ok();
    let release = adw::PreferencesGroup::new();
    let release_row = plain_row(
        "Ferestre release",
        Some(ferestre_core::package_version(packaged.as_deref())),
    );
    let check = gtk::Button::builder()
        .label("Check for Updates")
        .valign(gtk::Align::Center)
        .build();
    check.connect_clicked(glib::clone!(
        #[strong]
        ui,
        move |_| app_updates::refresh(&ui, true)
    ));
    release_row.add_suffix(&check);
    let quit = gtk::Button::builder()
        .label("Quit Ferestre")
        .valign(gtk::Align::Center)
        .build();
    let quit_window = ui.window.clone();
    quit.connect_clicked(move |_| quit_window.close());
    release_row.add_suffix(&quit);
    release.add(&release_row);
    page.append(&release);

    let model = ui.model.borrow();
    let (title, subtitle) = model::runtime_summary(model.runtime.as_ref());
    let group = adw::PreferencesGroup::builder()
        .title("Patched Proton")
        .build();
    let row = plain_row(&title, Some(&subtitle));
    row.set_subtitle_lines(3);
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
    page.append(&group);

    if model.runtimes.len() > 1 {
        let versions = adw::PreferencesGroup::builder()
            .title("Installed versions")
            .description("New launches use the newest compatible version. A title-specific choice is kept locally.")
            .build();
        for runtime in &model.runtimes {
            let row = plain_row(&runtime.label(), Some(&runtime.path.display().to_string()));
            row.set_subtitle_lines(2);
            versions.add(&row);
        }
        page.append(&versions);
    }

    if let Some(runtime) = &model.runtime {
        let capabilities = adw::PreferencesGroup::builder()
            .title("Capabilities")
            .description("What this build provides. A recipe names the ones it needs.")
            .build();
        for capability in &runtime.provides {
            let row = plain_row(capability, None);
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
        page.append(&capabilities);
    }
}

/// What one page of the list looks like, lifted out of the borrow so widgets
/// can be built without holding the model.
struct Drawn {
    page: usize,
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
    /// Rows the list holds after the filter and before a search narrows it:
    /// what a heading counting this list should say.
    listed: usize,
    /// Those rows' product ids and names, every page of them: what an action
    /// on the whole list applies to.
    kept: Vec<(String, String)>,
    /// Rows in the whole list still waiting for a name while a search is
    /// running. A search cannot match what has not been fetched, and saying
    /// "nothing matches" during that is telling somebody something untrue.
    pending_names: usize,
}

/// Which names have been asked for, and whether a lookup is still running.
/// A name asked for and not found is not "still looking": the catalog does not
/// describe every product, and those rows would otherwise read as pending for
/// as long as the window was open.
struct NameLookups<'a> {
    asked: &'a std::collections::BTreeSet<String>,
    fetching: bool,
}

impl NameLookups<'_> {
    fn pending(&self, product_id: &str) -> bool {
        self.fetching || !self.asked.contains(&product_id.to_ascii_uppercase())
    }
}

fn draw_list(
    model: &Model,
    running: &BTreeMap<String, stop::Launch>,
    downloading: Option<&str>,
    names: NameLookups,
    section: Section,
    keep: impl Fn(&LibraryRow) -> bool,
) -> Drawn {
    let launches = running;
    let running = |product_id: &str| launches.contains_key(&product_id.to_ascii_uppercase());
    let installing = |product_id: &str| {
        downloading.is_some_and(|id| {
            id.eq_ignore_ascii_case(product_id)
                || model
                    .catalog
                    .get(&product_id.to_ascii_uppercase())
                    .is_some_and(|product| {
                        product
                            .bundled_ids
                            .iter()
                            .any(|child| child.eq_ignore_ascii_case(id))
                    })
        })
    };
    let installed = |r: &Recipe| model.is_installed(r);
    let incomplete = |id: &str| model.incomplete(id);
    let selected_runtime = |id: &str| {
        model
            .paths
            .as_ref()
            .and_then(|paths| ferestre_core::runtime::requested(paths, id).ok().flatten())
    };
    let installed_version = |r: &Recipe| model.installed_version(r);
    let installed_product = |id: &str| model.product_is_installed(id);
    let describes_itself = |id: &str| model.product_executable(id).is_some();
    let game_dir = |r: &Recipe| model.game_dir(r);
    let content_parent = |id: &str| {
        model
            .product_dir(id)
            .and_then(|dir| ferestre_core::related::parent(&dir))
    };
    let inputs = Inputs {
        recipes: &model.recipes,
        runtimes: &model.runtimes,
        selected_runtime: &selected_runtime,
        incomplete: &incomplete,
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
        content_parent: &content_parent,
        installing: &installing,
        running: &running,
        game_dir: &game_dir,
    };
    // The Game Pass section is a different list, not a filter over the same
    // one: its rows come from a public catalogue listing rather than from what
    // the account owns.
    let mut rows: Vec<LibraryRow> = match section {
        Section::GamePass => {
            let held = if model.entitlements_checked {
                model.subscriptions.clone()
            } else {
                Vec::new()
            };
            model::gamepass(
                &inputs,
                &model.gamepass,
                // Empty is "not known", not "nothing is merchandised".
                Some(&model.gamepass_merchandised).filter(|m| !m.is_empty()),
                &model.tiers,
                &held,
            )
        }
        _ => model::library(&inputs),
    };
    // Bundle rows install a child product. They still need to show the active
    // download on their own row, regardless of which model branch built it.
    for row in &mut rows {
        // An installed title says when the selected Store account cannot play
        // it, before Play fails at the licence step.
        let store = model
            .account
            .as_ref()
            .map_or("this Store account", |a| a.label());
        let gap = model::subscription_gap(
            &row.product_id,
            &model.ownership,
            &model.catalog,
            &model.gamepass,
            &model.tiers,
            &model.subscriptions,
        );
        let installed_here = installed_product(&row.product_id);
        let source =
            if installed_here && model.refused.contains(&row.product_id.to_ascii_uppercase()) {
                format!("Not available to {store}: licence refused")
            } else if installed_here && gap == Some(model::SubscriptionGap::NoSubscription) {
                format!("Not available to {store}: needs Game Pass")
            } else if installed_here && gap == Some(model::SubscriptionGap::OutsideTier) {
                format!("Not available to {store}: outside its Game Pass tier")
            } else {
                model::access_source(
                    &row.product_id,
                    &model.ownership,
                    &model.catalog,
                    &model.gamepass,
                    &model.tiers,
                    &model.subscriptions,
                )
                .to_string()
            };
        // The chip beside Play says this; the subtitle need not say it twice.
        let (source, unavailable) = if source.starts_with("Not available to ") {
            (String::new(), Some(source))
        } else {
            (source, None)
        };
        row.unavailable = unavailable;
        let details = row
            .subtitle
            .split("  ·  ")
            .filter(|part| *part != "Included with PC Game Pass")
            .collect::<Vec<_>>()
            .join("  ·  ");
        row.subtitle = if source.is_empty() {
            details
        } else if details.starts_with("Version ") {
            match details.split_once("  ·  ") {
                Some((version, rest)) => format!("{version}  ·  {source}  ·  {rest}"),
                None => format!("{details}  ·  {source}"),
            }
        } else if details.is_empty() {
            source
        } else {
            format!("{source}  ·  {details}")
        };
        if installing(&row.product_id) {
            row.action = Action::Installing;
            row.outside_tier = false;
            row.unsupported = false;
        }
        // Every kind of row, not only the ones with a recipe: a running title
        // that still offered Play was started twice in one prefix, and the
        // first copy could no longer be stopped from here.
        let child_running = row.install_as.as_ref().is_some_and(|(id, _)| running(id));
        if running(&row.product_id) || child_running {
            row.action = Action::Stop;
            let launch = launches
                .get(&row.product_id.to_ascii_uppercase())
                .or_else(|| {
                    row.install_as
                        .as_ref()
                        .and_then(|(id, _)| launches.get(&id.to_ascii_uppercase()))
                })
                .unwrap();
            let state = if launch.asked.is_some() {
                "stopping"
            } else if launch.ready {
                "running"
            } else {
                "starting"
            };
            let details = row
                .subtitle
                .split("  ·  ")
                .filter(|s| !matches!(*s, "running" | "starting" | "stopping"))
                .collect::<Vec<_>>()
                .join("  ·  ");
            row.subtitle = if details.is_empty() {
                state.into()
            } else {
                format!("{details}  ·  {state}")
            };
        }
    }
    let unsupported = rows.iter().filter(|row| row.unsupported).count();
    let outside_tier = rows.iter().filter(|row| row.outside_tier).count();
    // Resolve the whole list before filtering: unnamed off-page titles must
    // be searchable, and learning their names must not depend on paging.
    let unnamed_off_page: Vec<String> = rows
        .iter()
        .filter(|row| {
            (!row.is_named()
                || !model
                    .catalog
                    .contains_key(&row.product_id.to_ascii_uppercase()))
                && names.pending(&row.product_id)
        })
        .map(|row| row.product_id.clone())
        .collect();
    // Revalidation must include named, cached rows, including off-page ones.
    let metadata_ids: Vec<String> = rows.iter().map(|row| row.product_id.clone()).collect();
    let rows: Vec<LibraryRow> = rows.into_iter().filter(|row| keep(row)).collect();
    let listed = rows.len();
    let kept = rows
        .iter()
        .map(|row| (row.product_id.clone(), row.name.clone()))
        .collect();

    let matching = model::search(&rows, &model.query);
    let view = model::paginate(&matching, model.page, PER_PAGE);
    // With a query, the rows that are *not* on screen matter: search matches on
    // the name, and a row nothing has looked up is still showing its product
    // id, so "age of" cannot match it. That was a search that found nothing
    // until you happened to page past the row, and then worked ever after.
    Drawn {
        page: view.page,
        listed,
        kept,
        pending_names: unnamed_off_page.len(),
        want_names: metadata_ids
            .into_iter()
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
                    wanted.extend(product.bundled_ids.iter().cloned());
                }
                wanted
            }))
            .collect(),
        want_icons: view
            .rows
            .iter()
            .filter(|row| {
                !model
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
fn render_gamepass(ui: &Ui, page: &gtk::Box) {
    let (held, fetched, market, store, checked) = {
        let model = ui.model.borrow();
        (
            model.subscriptions.clone(),
            !model.gamepass.is_empty(),
            model.market().0,
            model.account.as_ref().map(|a| a.label().to_string()),
            model.entitlements_checked,
        )
    };

    // Drawn first, filled in after the list: the count is what the list below
    // actually holds -- games only, in this market, that this account's tier
    // and this runtime can install -- not the size of the feed it came from.
    let group = adw::PreferencesGroup::builder().build();
    let banner = adw::ActionRow::builder().subtitle_lines(3).build();
    banner.add_css_class("gamepass-strip");
    banner.set_use_markup(false);
    banner.add_prefix(&gtk::Image::from_icon_name("starred-symbolic"));
    group.add(&banner);
    page.append(&group);

    let show_all = ui.model.borrow().show_unsupported;
    let listed = render_rows(
        ui,
        page,
        move |row| show_all || !row.known_uninstallable(),
        "Nothing here yet.",
    );

    let games = |n: usize| format!("{n} {}", if n == 1 { "game" } else { "games" });
    let in_market = format!("{} in PC Game Pass for the {market} Store", games(listed));
    let (title, subtitle) = match (fetched, store, checked, held.as_slice()) {
        (false, ..) => (
            "The Game Pass catalogue has not been fetched yet".to_string(),
            "Reload to fetch it. It is a public listing -- no account needed.".to_string(),
        ),
        (true, None, ..) => (
            in_market,
            "Sign in to see which of these your Store account can play.".to_string(),
        ),
        (true, Some(store), false, _) => (
            in_market,
            format!("Load the library to see which of these {store} can play."),
        ),
        (true, Some(store), true, []) => (
            in_market,
            format!(
                "{store} has no Game Pass subscription, so it cannot play these. Choose an \
                 account with Game Pass as the Store account in Accounts."
            ),
        ),
        (true, Some(store), true, subs) => (
            if show_all {
                in_market
            } else {
                format!(
                    "{} {store} can install with {}",
                    games(listed),
                    subs.join(" and ")
                )
            },
            format!(
                "From the PC Game Pass catalogue for the {market} Store. Add-ons are not \
                 counted, and titles outside the tier or in packages this runtime cannot \
                 open are left out unless you choose to show them."
            ),
        ),
    };
    banner.set_title(&title);
    banner.set_subtitle(&subtitle);
}

/// The saved measurement for a title, if it still matches the install the
/// model knows about now: same directory, same record, same root and manifest.
fn current_disk_usage(model: &Model, key: &str) -> Option<u64> {
    let cache_dir = model.cache_dir()?;
    let record = model.records.get(key);
    let dir = record.map(|r| r.dir.clone()).or_else(|| {
        model
            .recipe_for(key)
            .and_then(|recipe| model.game_dir(recipe))
    })?;
    ferestre_core::install::cached_disk_usage(cache_dir, key, &dir, record).map(|(bytes, _)| bytes)
}

/// Reuse persistent disk measurements before drawing. Only missing/stale or
/// changed installs need a walk, and that runs off the GTK thread. A walk that
/// a reload overtook repopulates the model only for installs whose saved
/// fingerprint still matches what the reloaded model points at.
fn fetch_disk_usage(ui: &Ui, rows: &[LibraryRow]) {
    let (generation, cache_dir, wanted) = {
        let mut model = ui.model.borrow_mut();
        let cache_dir = model.cache_dir().map(Path::to_path_buf);
        let wanted: Vec<_> = rows
            .iter()
            .filter(|row| row.installed)
            .filter_map(|row| {
                let key = row.product_id.to_ascii_uppercase();
                if model.disk_usage.contains_key(&key) {
                    return None;
                }
                let dir = model.records.get(&key).map(|r| r.dir.clone()).or_else(|| {
                    model
                        .recipe_for(&row.product_id)
                        .and_then(|recipe| model.game_dir(recipe))
                });
                let record = model.records.get(&key).cloned();
                if let (Some(cache_dir), Some(dir)) = (&cache_dir, &dir) {
                    if let Some((bytes, fresh)) = ferestre_core::install::cached_disk_usage(
                        cache_dir,
                        &key,
                        dir,
                        record.as_ref(),
                    ) {
                        model.disk_usage.insert(key.clone(), Some(bytes));
                        if fresh {
                            return None;
                        }
                    }
                }
                // A scan from before a reload is still walking this title.
                // Show the saved total meanwhile, and let that scan answer.
                if model.sizes_pending.contains(&key) {
                    return None;
                }
                model.sizes_pending.insert(key.clone());
                Some((key, dir, record))
            })
            .collect();
        (model.size_generation, cache_dir, wanted)
    };
    if wanted.is_empty() {
        return;
    }
    // Pending now outlives a reload, so a walk that panicked must not leave its
    // titles marked as being scanned for the rest of the session.
    let keys: Vec<String> = wanted.iter().map(|(key, _, _)| key.clone()).collect();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let scanned = gio::spawn_blocking(move || {
            wanted
                .into_iter()
                .map(|(key, dir, record)| {
                    let bytes = dir.and_then(|dir| match &cache_dir {
                        Some(cache_dir) => ferestre_core::install::measure_disk_usage(
                            cache_dir,
                            &key,
                            &dir,
                            record.as_ref(),
                        ),
                        None => ferestre_core::install::disk_usage(&dir),
                    });
                    (key, bytes)
                })
                .collect::<Vec<_>>()
        })
        .await;
        if let Ok(scanned) = scanned {
            let mut model = ui.model.borrow_mut();
            let current = model.size_generation == generation;
            for (key, bytes) in scanned {
                model.sizes_pending.remove(&key);
                let bytes = if current {
                    bytes
                } else {
                    // The window reloaded while this walked. The result still
                    // counts if the install it measured is the one there now,
                    // which is what the saved fingerprint checks; otherwise the
                    // render below starts a scan of what is there now.
                    match current_disk_usage(&model, &key) {
                        Some(bytes) => Some(bytes),
                        None => continue,
                    }
                };
                // Keep an expired measurement if the new walk failed.
                if bytes.is_some() || !model.disk_usage.contains_key(&key) {
                    model.disk_usage.insert(key, bytes);
                }
            }
            drop(model);
            render(&ui);
        } else {
            let mut model = ui.model.borrow_mut();
            for key in &keys {
                model.sizes_pending.remove(key);
            }
        }
    });
}

/// Draws the list and returns how many rows it holds before a search.
fn render_rows(
    ui: &Ui,
    page: &gtk::Box,
    keep: impl Fn(&LibraryRow) -> bool,
    empty_message: &str,
) -> usize {
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
        NameLookups {
            asked: &ui.asked_names.borrow(),
            fetching: ui.fetching_names.get(),
        },
        section,
        keep,
    );
    let listed = drawn.listed;
    artwork::visible(ui, drawn.rows.iter().map(|row| row.product_id.clone()));
    // Persist the clamped page. Otherwise Previous decrements a stale page
    // number and keeps showing the last page after the list shrinks.
    ui.model.borrow_mut().page = drawn.page;
    fetch_disk_usage(ui, &drawn.rows);

    let group = adw::PreferencesGroup::builder().build();
    if section == Section::Installed && !drawn.kept.is_empty() {
        group.set_title(&match listed {
            1 => "1 game".to_string(),
            n => format!("{n} games"),
        });
        group.set_header_suffix(Some(&add_all_to_steam_button(ui, drawn.kept.clone())));
    }
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
        // The query is whatever was typed, "&" included.
        let row = plain_row(&message, None);
        row.add_css_class("dim-label");
        group.add(&row);
    } else {
        if section == Section::Updates {
            for row in &drawn.rows {
                group.add(&library_row(ui, row));
            }
        }
    }
    page.append(&group);
    if !drawn.rows.is_empty() && section != Section::Updates {
        if matches!(section, Section::Home | Section::GamePass)
            && ui.model.borrow().query.is_empty()
            && drawn.page == 0
        {
            render_featured(ui, page, &drawn.rows);
        }
        let heading = gtk::Label::builder()
            .label(if section == Section::Home {
                "All games".into()
            } else {
                format!("{} games", drawn.listed)
            })
            .xalign(0.0)
            .css_classes(["title-2"])
            .build();
        if !(ui.window.is_fullscreen() && matches!(section, Section::GamePass | Section::Installed))
        {
            page.append(&heading);
        }
        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(5)
            .row_spacing(22)
            .column_spacing(18)
            .css_classes(["game-grid"])
            .build();
        for row in &drawn.rows {
            grid.insert(&game_card(ui, row), -1);
        }
        grid.connect_child_activated(|_, child| {
            if let Some(card) = child.child() {
                fn primary(root: &gtk::Widget) -> Option<gtk::Widget> {
                    if root.widget_name().starts_with("game-")
                        && root.widget_name().ends_with("-primary")
                    {
                        return Some(root.clone());
                    }
                    let mut next = root.first_child();
                    while let Some(widget) = next {
                        if let Some(found) = primary(&widget) {
                            return Some(found);
                        }
                        next = widget.next_sibling();
                    }
                    None
                }
                if let Some(button) = primary(&card) {
                    button.activate();
                }
            }
        });
        page.append(&grid);
    }

    if drawn.pending_names > 0 && !drawn.rows.is_empty() {
        let progress = gtk::Label::builder()
            .label(format!("Looking up {} title names…", drawn.pending_names))
            .css_classes(["dim-label"])
            .build();
        page.append(&progress);
    }

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
        page.append(&group);
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
        previous.set_tooltip_text(Some("Previous page"));
        previous.update_property(&[gtk::accessible::Property::Label("Previous page")]);
        let next = gtk::Button::from_icon_name("go-next-symbolic");
        next.set_sensitive(drawn.has_next);
        next.set_tooltip_text(Some("Next page"));
        next.update_property(&[gtk::accessible::Property::Label("Next page")]);
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
                ui.scroll.vadjustment().set_value(0.0);
                render(&ui);
            }
        ));
        next.connect_clicked(glib::clone!(
            #[strong]
            ui,
            move |_| {
                ui.model.borrow_mut().page += 1;
                ui.scroll.vadjustment().set_value(0.0);
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
    listed
}

fn choose_runtime(ui: &Ui, product_id: &str, title: &str) {
    let (paths, runtimes, current) = {
        let model = ui.model.borrow();
        let Some(paths) = model.paths.clone() else {
            return;
        };
        (
            paths.clone(),
            model.runtimes.clone(),
            ferestre_core::runtime::selected(paths.config_dir(), product_id),
        )
    };
    let dialog = gtk::Window::builder()
        .application(ui.window.application().as_ref().unwrap())
        .transient_for(&ui.window)
        .modal(true)
        .title(format!("Runtime for {title}"))
        .default_width(520)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let automatic = gtk::Button::with_label("Automatic (newest compatible)");
    let automatic_ui = ui.clone();
    let automatic_dialog = dialog.clone();
    let automatic_paths = paths.clone();
    let automatic_product = product_id.to_string();
    automatic.connect_clicked(move |_| {
        if let Err(error) = ferestre_core::runtime::set_selected(
            automatic_paths.config_dir(),
            &automatic_product,
            None,
        ) {
            automatic_ui
                .toasts
                .add_toast(toast(format!("Could not save runtime choice: {error}")));
        } else {
            automatic_dialog.close();
            // The row is judged against the runtime its launch will use.
            render(&automatic_ui);
            automatic_ui
                .toasts
                .add_toast(toast("Runtime selection reset to automatic"));
        }
    });
    content.append(&automatic);
    for runtime in runtimes {
        let selected = current.as_deref() == Some(runtime.path.as_path());
        let label = if selected {
            format!("{} (selected)", runtime.label())
        } else {
            runtime.label()
        };
        let choice = gtk::Button::with_label(&label);
        let selected_ui = ui.clone();
        let selected_dialog = dialog.clone();
        let selected_paths = paths.clone();
        let selected_product = product_id.to_string();
        let path = runtime.path.clone();
        choice.connect_clicked(move |_| {
            if let Err(error) = ferestre_core::runtime::set_selected(
                selected_paths.config_dir(),
                &selected_product,
                Some(&path),
            ) {
                selected_ui
                    .toasts
                    .add_toast(toast(format!("Could not save runtime choice: {error}")));
            } else {
                selected_dialog.close();
                render(&selected_ui);
                selected_ui
                    .toasts
                    .add_toast(toast("Runtime selected for this title"));
            }
        });
        content.append(&choice);
    }
    let cancel = gtk::Button::with_label("Cancel");
    let cancel_dialog = dialog.clone();
    cancel.connect_clicked(move |_| cancel_dialog.close());
    content.append(&cancel);
    dialog.set_child(Some(&content));
    dialog.present();
}

fn render_library_tabs(ui: &Ui, page: &gtk::Box) {
    page.append(
        &gtk::Label::builder()
            .label("My Library")
            .xalign(0.0)
            .css_classes(["title-1"])
            .build(),
    );
    let tabs = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(22)
        .css_classes(["library-tabs"])
        .build();
    for (label, section) in [
        ("My games", Section::Library),
        ("Installed", Section::Installed),
        ("Updates", Section::Updates),
        ("Install queue", Section::Downloads),
    ] {
        let button = gtk::Button::with_label(label);
        button.set_widget_name(&format!("section-{}", section.title()));
        if ui.model.borrow().section == section {
            button.add_css_class("active");
        }
        let ui = ui.clone();
        button.connect_clicked(move |_| select_section(&ui, section));
        tabs.append(&button);
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&tabs)
        .build();
    page.append(&scroll);
}
fn render_recents(ui: &Ui) {
    while let Some(child) = ui.recent_games.first_child() {
        ui.recent_games.remove(&child);
    }
    let model = ui.model.borrow();
    let mut ids = std::collections::BTreeSet::new();
    ids.extend(model.records.keys().cloned());
    ids.extend(model.installed_products.keys().cloned());
    for recipe in &model.recipes {
        if model.is_installed(recipe) {
            ids.insert(recipe.title.product_id.to_ascii_uppercase());
        }
    }
    let mut titles: Vec<_> = ids
        .into_iter()
        .filter(|id| {
            model.product_is_installed(id)
                || model.recipe_for(id).is_some_and(|r| model.is_installed(r))
        })
        .map(|id| {
            let played = model
                .paths
                .as_ref()
                .and_then(|p| {
                    std::fs::metadata(p.state_dir().join("launches").join(format!("{id}.stderr")))
                        .ok()
                })
                .and_then(|m| m.modified().ok());
            let installed = model.records.get(&id).and_then(|r| r.installed_at.clone());
            (played, installed, id)
        })
        .collect();
    titles.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    ui.recent_games.append(
        &gtk::Label::builder()
            .label("Most Recent")
            .xalign(0.0)
            .margin_top(12)
            .margin_bottom(6)
            .css_classes(["caption-heading", "dim-label"])
            .build(),
    );
    if titles.is_empty() {
        ui.recent_games.append(
            &gtk::Label::builder()
                .label("Your installed games appear here")
                .wrap(true)
                .xalign(0.0)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
    }
    for (played, _, id) in titles.into_iter().take(5) {
        let name = model
            .catalog
            .get(&id)
            .map(|p| p.name.clone())
            .or_else(|| model.recipe_for(&id).map(|r| r.title.name.clone()))
            .unwrap_or(id.clone());
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let image = gtk::Image::builder()
            .icon_name("applications-games-symbolic")
            .pixel_size(48)
            .width_request(48)
            .height_request(56)
            .build();
        if let Some(texture) = model
            .icons
            .get(&id)
            .and_then(|p| gdk::Texture::from_filename(p).ok())
        {
            image.set_paintable(Some(&texture));
        }
        content.append(&image);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
        text.append(
            &gtk::Label::builder()
                .label(&name)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(18)
                .css_classes(["heading"])
                .build(),
        );
        text.append(
            &gtk::Label::builder()
                .label(if played.is_some() {
                    "Recently played"
                } else {
                    "Installed"
                })
                .xalign(0.0)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
        content.append(&text);
        let button = gtk::Button::builder()
            .child(&content)
            .tooltip_text(format!("Play {name}"))
            .build();
        button.set_widget_name(&format!("recent-{id}"));
        let recent_ui = ui.clone();
        button.connect_clicked(move |_| spawn_title(&recent_ui, &id, &name));
        ui.recent_games.append(&button);
    }
}
fn render_featured(ui: &Ui, page: &gtk::Box, rows: &[LibraryRow]) {
    // Fullscreen browsing prioritizes complete rows of game covers on a
    // handheld display instead of spending its first viewport on promotions.
    if ui.window.is_fullscreen() {
        return;
    }
    let strip = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .height_request(48)
        .margin_bottom(12)
        .css_classes(["gamepass-strip"])
        .build();
    strip.append(
        &gtk::Label::builder()
            .label("Discover PC Game Pass")
            .xalign(0.0)
            .hexpand(true)
            .margin_start(18)
            .css_classes(["heading"])
            .build(),
    );
    let count = ui.model.borrow().gamepass.len();
    if count > 0 {
        strip.append(
            &gtk::Label::builder()
                .label(format!("{count} games"))
                .margin_end(18)
                .css_classes(["heading"])
                .build(),
        );
    }
    page.append(&strip);
    let features = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    for row in rows.iter().take(3) {
        if row.action == Action::Stop {
            features.append(&game_card(ui, row));
            continue;
        }
        let overlay = gtk::Overlay::builder()
            .width_request(420)
            .height_request(260)
            .css_classes(["feature-card"])
            .build();
        let art = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Cover)
            .build();
        let path = ui
            .model
            .borrow()
            .catalog_cache()
            .and_then(|cache| cache.hero_path(&row.product_id));
        if let Some(texture) = path.and_then(|p| gdk::Texture::from_filename(p).ok()) {
            art.set_paintable(Some(&texture));
        }
        overlay.set_child(Some(&art));
        let caption = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .valign(gtk::Align::End)
            .css_classes(["feature-caption"])
            .build();
        caption.append(
            &gtk::Label::builder()
                .label(&row.name)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(32)
                .css_classes(["title-2"])
                .build(),
        );
        caption.append(
            &gtk::Label::builder()
                .label(if row.installed {
                    "Jump back in"
                } else {
                    "Discover your next game"
                })
                .xalign(0.0)
                .css_classes(["body"])
                .build(),
        );
        overlay.add_overlay(&caption);
        let (_, actions) = library_components(ui, row);
        if let Some(button) = actions
            .last_child()
            .and_then(|w| w.downcast::<gtk::Button>().ok())
        {
            actions.remove(&button);
            button.set_widget_name(&format!("feature-{}-primary", row.product_id));
            button.set_child(Some(&overlay));
            button.remove_css_class("suggested-action");
            button.remove_css_class("destructive-action");
            button.add_css_class("game-tile");
            features.append(&button);
        } else {
            features.append(&overlay);
        }
    }
    let carousel = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .height_request(284)
        .child(&features)
        .build();
    page.append(&carousel);
    let links = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(4)
        .row_spacing(10)
        .column_spacing(14)
        .build();
    for (label, section) in [
        ("All games", Section::Library),
        ("Installed games", Section::Installed),
        ("Updates", Section::Updates),
        ("Install queue", Section::Downloads),
    ] {
        let button = gtk::Button::with_label(label);
        let ui = ui.clone();
        button.connect_clicked(move |_| select_section(&ui, section));
        links.insert(&button, -1);
    }
    page.append(&links);
}

fn library_row(ui: &Ui, row: &LibraryRow) -> adw::ActionRow {
    let (view, actions) = library_components(ui, row);
    view.add_suffix(&actions);
    view
}
/// A high-contrast indeterminate ring, independent of the desktop spinner theme.
fn game_launch_spinner(label: &str) -> gtk::DrawingArea {
    let ring = gtk::DrawingArea::builder()
        .content_width(56)
        .content_height(56)
        .halign(gtk::Align::Center)
        .accessible_role(gtk::AccessibleRole::ProgressBar)
        .build();
    ring.update_property(&[gtk::accessible::Property::Label(label)]);
    let started = std::time::Instant::now();
    ring.set_draw_func(move |widget, context, width, height| {
        let center = (f64::from(width) / 2., f64::from(height) / 2.);
        let radius = (f64::from(width.min(height)) / 2. - 5.).max(0.);
        let angle = if widget.settings().is_gtk_enable_animations() {
            started.elapsed().as_secs_f64() * std::f64::consts::TAU
        } else {
            0.
        };
        context.set_line_width(4.);
        context.set_line_cap(gtk::cairo::LineCap::Round);
        context.set_source_rgba(1., 1., 1., 0.22);
        context.arc(center.0, center.1, radius, 0., std::f64::consts::TAU);
        let _ = context.stroke();
        context.set_source_rgb(139. / 255., 215. / 255., 123. / 255.);
        context.arc(
            center.0,
            center.1,
            radius,
            angle,
            angle + std::f64::consts::TAU * 0.72,
        );
        let _ = context.stroke();
    });
    ring.add_tick_callback(|widget, _| {
        if widget.settings().is_gtk_enable_animations() {
            widget.queue_draw();
        }
        glib::ControlFlow::Continue
    });
    ring
}

fn game_card(ui: &Ui, row: &LibraryRow) -> adw::Clamp {
    let (description, actions) = library_components(ui, row);
    let primary = actions
        .last_child()
        .and_then(|w| w.downcast::<gtk::Button>().ok());
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    body.update_property(&[gtk::accessible::Property::Description(&format!(
        "{} · {}",
        row.name,
        description.subtitle().unwrap_or_default()
    ))]);
    let key = row.product_id.to_ascii_uppercase();
    let texture = ui
        .model
        .borrow()
        .icons
        .get(&key)
        .and_then(|p| gdk::Texture::from_filename(p).ok());
    let art = gtk::Picture::builder()
        .can_shrink(true)
        .height_request(if ui.window.is_fullscreen() { 300 } else { 280 })
        .content_fit(gtk::ContentFit::Cover)
        .build();
    if let Some(texture) = texture.filter(|t| t.width() >= 300) {
        art.set_paintable(Some(&texture));
    }
    let cover = gtk::Overlay::new();
    cover.set_child(Some(&art));
    if row.has_update() {
        // Keep the badge on the cover, below the separate top-right menu.
        // Both controls must remain readable on the narrow handheld cards.
        let badge = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .css_classes(["game-update-badge"])
            .can_target(false)
            .build();
        let icon = gtk::Image::from_icon_name("software-update-available-symbolic");
        icon.set_pixel_size(14);
        badge.append(&icon);
        badge.append(&gtk::Label::new(Some("Update available")));
        cover.add_overlay(&badge);
    }
    body.append(&cover);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(5)
        .margin_top(10)
        .margin_bottom(12)
        .margin_start(10)
        .margin_end(10)
        .build();
    let title = gtk::Label::builder()
        .label(&row.name)
        .xalign(0.0)
        .wrap(true)
        .lines(2)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .width_chars(18)
        .max_width_chars(20)
        .height_request(36)
        .css_classes(["game-title"])
        .build();
    text.append(&title);
    let publisher = ui
        .model
        .borrow()
        .catalog
        .get(&key)
        .and_then(|p| p.publisher.clone())
        .unwrap_or_else(|| "Xbox PC".into());
    text.append(
        &gtk::Label::builder()
            .label(publisher)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(20)
            .css_classes(["game-meta"])
            .build(),
    );
    let launching =
        row.action == Action::Stop && row.subtitle.split("  ·  ").any(|s| s == "starting");
    let state = if row.action == Action::Stop {
        if launching {
            "Starting…"
        } else if row.subtitle.split("  ·  ").any(|s| s == "stopping") {
            "Stopping…"
        } else {
            "● Running"
        }
    } else if row.action == Action::Play {
        "✓ Installed"
    } else {
        row.action.label()
    };
    let size = description
        .subtitle()
        .filter(|_| row.action != Action::Stop)
        .and_then(|text| {
            text.split("  ·  ")
                .find(|part| part.contains(" download") || part.contains("on disk"))
                .map(str::to_owned)
        });
    let state = size.map_or_else(|| state.to_string(), |size| format!("{state} · {size}"));
    let state = if row.action == Action::Stop {
        state
    } else if let Some(reason) = &row.unavailable {
        reason.clone()
    } else if row.subtitle.contains("known not to run") {
        "Known not to run".into()
    } else {
        state
    };
    let status = gtk::Label::builder()
        .label(state)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(20)
        .css_classes(["game-meta"])
        .tooltip_text(description.subtitle().unwrap_or_default())
        .build();
    // Keep the full details available to assistive technology while the
    // visible caption fits the card. More > Game details exposes them to touch.
    status.update_property(&[gtk::accessible::Property::Label(
        description.subtitle().as_deref().unwrap_or_default(),
    )]);
    if row.action == Action::Stop && !launching {
        status.add_css_class("game-running");
    }
    text.append(&status);
    body.append(&text);
    let surface = gtk::Overlay::new();
    surface.set_child(Some(&body));
    if row.action == Action::Stop {
        let stopping = row.subtitle.split("  ·  ").any(|s| s == "stopping");
        let state = if launching {
            "Starting…"
        } else if stopping {
            "Stopping…"
        } else {
            "Running"
        };
        let layer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .css_classes(["game-launch-overlay"])
            .build();
        layer.set_accessible_role(gtk::AccessibleRole::Group);
        layer.update_property(&[gtk::accessible::Property::Label(if launching {
            "Starting overlay"
        } else {
            "Running overlay"
        })]);
        let center = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .vexpand(true)
            .hexpand(true)
            .build();
        if launching || stopping {
            center.append(&game_launch_spinner(if launching {
                "Starting game"
            } else {
                "Stopping game"
            }));
        } else {
            let icon = gtk::Image::from_icon_name("media-playback-stop-symbolic");
            icon.set_pixel_size(48);
            center.append(&icon);
        }
        center.append(
            &gtk::Label::builder()
                .label(state)
                .css_classes(["game-overlay-state"])
                .build(),
        );
        // These are separate controls, never buttons nested inside a button.
        let mut control = actions.first_child();
        while let Some(widget) = control {
            control = widget.next_sibling();
            if widget.is::<gtk::Button>() {
                actions.remove(&widget);
                widget.set_size_request(144, 48);
                widget.add_css_class("pill");
                widget.reset_relation(gtk::AccessibleRelation::LabelledBy);
                if let Some(button) = widget.downcast_ref::<gtk::Button>() {
                    widget.update_property(&[gtk::accessible::Property::Label(&format!(
                        "{} · {}",
                        row.name,
                        button.label().unwrap_or_default()
                    ))]);
                }
                center.append(&widget);
            }
        }
        layer.append(&center);
        surface.add_overlay(&layer);
    }
    let overlay = gtk::Overlay::new();
    if row.action == Action::Stop {
        surface.add_css_class("game-tile");
        surface.set_accessible_role(gtk::AccessibleRole::Group);
        surface.update_property(&[gtk::accessible::Property::Label(&format!(
            "{} · Game card",
            row.name
        ))]);
        overlay.set_child(Some(&surface));
    } else if let Some(primary) = primary {
        actions.remove(&primary);
        primary.set_child(Some(&surface));
        primary.set_tooltip_text(description.tooltip_text().as_deref());
        primary.reset_relation(gtk::AccessibleRelation::LabelledBy);
        primary.update_property(&[gtk::accessible::Property::Label(&format!(
            "{} · {}",
            row.name,
            row.action.label()
        ))]);
        primary.remove_css_class("suggested-action");
        primary.remove_css_class("destructive-action");
        primary.add_css_class("game-tile");
        primary.update_property(&[gtk::accessible::Property::Description(&format!(
            "{} · {}",
            row.action.label(),
            description.subtitle().unwrap_or_default()
        ))]);
        overlay.set_child(Some(&primary));
    } else {
        surface.add_css_class("game-tile");
        overlay.set_child(Some(&surface));
    }
    if let Some(more) = actions.first_child() {
        actions.remove(&more);
        more.set_halign(gtk::Align::End);
        more.set_valign(gtk::Align::Start);
        more.set_margin_top(6);
        more.set_margin_end(6);
        more.add_css_class("game-more");
        overlay.add_overlay(&more);
    }
    // Clamp both the natural size and final width: FlowBox otherwise chooses
    // one giant column from a texture's original pixel dimensions.
    adw::Clamp::builder()
        .maximum_size(196)
        .tightening_threshold(196)
        .child(&overlay)
        .build()
}

fn library_components(ui: &Ui, row: &LibraryRow) -> (adw::ActionRow, gtk::Box) {
    ui.game_actions
        .borrow_mut()
        .insert(row.product_id.clone(), row.action.label().into());
    // A title is a name, not markup, and the flag has to be set before the text
    // is: left as markup, "Minecraft: Java & Bedrock Edition for PC" fails to
    // parse and the row draws with no title at all. The same goes for any
    // subtitle that names a title, which the bundle rows do.
    let action_row = adw::ActionRow::builder()
        .title_lines(1)
        .subtitle_lines(2)
        .css_classes(["library-row"])
        .tooltip_text(&row.product_id)
        .build();
    action_row.set_use_markup(false);
    action_row.set_title(&row.name);

    let icon = gtk::Image::builder()
        .pixel_size(48)
        .width_request(48)
        .height_request(48)
        .valign(gtk::Align::Center)
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

    let size_text = {
        let model = ui.model.borrow();
        let key = row.product_id.to_ascii_uppercase();
        let download_id = row
            .install_as
            .as_ref()
            .map(|(id, _)| id)
            .unwrap_or(&row.product_id);
        let download = model
            .catalog
            .get(&download_id.to_ascii_uppercase())
            .and_then(|p| p.download_bytes);
        model::game_size_label(row.installed, model.disk_usage.get(&key).copied(), download)
    };
    // One title line, one details line. Keep status and sizes beside the
    // version instead of competing with the title and buttons for row width.
    let (version, description) = if row.subtitle.starts_with("Version ") {
        match row.subtitle.split_once("  ·  ") {
            Some((version, rest)) => (Some(version), rest),
            None => (Some(row.subtitle.as_str()), ""),
        }
    } else {
        (None, row.subtitle.as_str())
    };
    let mut details = Vec::new();
    if let Some(version) = version {
        details.push(format!("v{}", version.trim_start_matches("Version ")));
    }
    details.push(size_text);
    if let Some((badge, _)) = row.badge {
        details.push(badge.to_string());
    }
    if !description.is_empty() {
        details.push(description.to_string());
    }
    let details = details.join("  ·  ");
    action_row.set_subtitle(&details);
    // Which runtime Play would use: the per-title choice changes it, and a
    // row that hid that made "Choose runtime" look as if it did nothing.
    let runtime_note = ui.model.borrow().runtime_note(&row.product_id);
    action_row.set_tooltip_text(Some(&match &runtime_note {
        Some(note) => format!("{}\n{}\n{}\n{}", row.name, details, note, row.product_id),
        None => format!("{}\n{}\n{}", row.name, details, row.product_id),
    }));

    // Said where the Play button is, not only in a dimmed subtitle the eye
    // skips: pressing Play would end at the licence step.
    if let Some(reason) = &row.unavailable {
        let why = reason
            .split_once(": ")
            .map_or("Not available", |(_, why)| why);
        let mut why = why.chars();
        let why: String = why
            .next()
            .map(|c| c.to_uppercase().chain(why).collect())
            .unwrap_or_default();
        let chip = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(6)
                .valign(gtk::Align::Center)
                .css_classes(["access-chip"])
                .tooltip_text(format!(
                    "{reason}. Choose an account that owns it or has Game Pass as the Store account in Accounts."
                ))
                .build();
        chip.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
        chip.append(
            &gtk::Label::builder()
                .label(&why)
                .css_classes(["caption-heading"])
                .build(),
        );
        action_row.add_suffix(&chip);
    }

    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .valign(gtk::Align::Center)
        .margin_start(8)
        .build();
    actions.add_css_class("game-actions");

    // Secondary actions live behind one menu button: five icon buttons on
    // every row left the title three characters wide.
    let menu = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    let popover = gtk::Popover::builder().child(&menu).build();
    let more = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text("More actions")
        .popover(&popover)
        .build();
    more.set_widget_name(&format!("game-{}-more", row.product_id));
    more.update_property(&[gtk::accessible::Property::Label(&format!(
        "More actions for {}",
        row.name
    ))]);
    let item = |icon: &str, label: &str, run: Box<dyn Fn()>| {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        content.append(&gtk::Image::from_icon_name(icon));
        content.append(&gtk::Label::builder().label(label).xalign(0.0).build());
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        let popover = popover.clone();
        button.connect_clicked(move |_| {
            popover.popdown();
            run();
        });
        menu.append(&button);
    };
    let detail_ui = ui.clone();
    let detail_name = row.name.clone();
    let detail_text = match &runtime_note {
        Some(runtime) => format!("{details}\n\n{runtime}"),
        None => details.clone(),
    };
    item(
        "dialog-information-symbolic",
        "Game details",
        Box::new(move || {
            let dialog = adw::AlertDialog::new(Some(&detail_name), Some(&detail_text));
            dialog.set_body_use_markup(false);
            dialog.add_response("close", "Close");
            dialog.set_close_response("close");
            dialog.present(Some(&detail_ui.window));
        }),
    );
    let store_url = format!("{STORE_PAGE}{}", row.product_id);
    let anchor = action_row.clone();
    item(
        "web-browser-symbolic",
        "Open Store page",
        Box::new(move || open_link(&anchor, &store_url)),
    );
    // A shortcut needs something that will launch. A recipe is one; a title on
    // disk is another, because `run` writes a recipe from the package manifest
    // on its way past.
    if (row.has_recipe || row.installed) && row.action != Action::Content {
        for (icon, label, run) in [
            (
                "applications-engineering-symbolic",
                "Choose runtime",
                choose_runtime as fn(&Ui, &str, &str),
            ),
            ("list-add-symbolic", "Add to Steam", add_to_steam),
            (
                "document-edit-symbolic",
                "Edit launch settings",
                open_editor,
            ),
        ] {
            let (ui, product_id, name) = (ui.clone(), row.product_id.clone(), row.name.clone());
            item(icon, label, Box::new(move || run(&ui, &product_id, &name)));
        }
    }
    if row.installed && row.action != Action::Stop && row.action != Action::Installing {
        let (ui, product_id, name) = (ui.clone(), row.product_id.clone(), row.name.clone());
        item(
            "go-jump-symbolic",
            "Move location",
            Box::new(move || choose_move(&ui, &product_id, &name)),
        );
    }
    // Anything on disk can be removed, recipe or not -- and the rows that most
    // need it are the ones with no recipe, or a download that stopped, because
    // a failed install leaves exactly that. Not while it runs: its files, and
    // with the box ticked its saves, would go from under it.
    let on_disk = row.installed || row.action == Action::Resume;
    if on_disk && row.action != Action::Stop {
        let (ui, product_id, name) = (ui.clone(), row.product_id.clone(), row.name.clone());
        item(
            "user-trash-symbolic",
            "Remove from disk",
            Box::new(move || confirm_remove(&ui, &product_id, &name)),
        );
    }
    actions.append(&more);

    // A greyed-out "Play" is a button that says a title might work if you found
    // the right thing to click. It never will: the reason is in the subtitle
    // and on the row, and the Store link beside it is the thing to press.
    if let Action::Blocked(reason) = &row.action {
        action_row.set_tooltip_text(Some(&format!("{}\n{}\n{}", row.name, details, reason)));
        return (action_row, actions);
    }

    let button = gtk::Button::builder()
        .label(row.action.label())
        .width_request(88)
        .valign(gtk::Align::Center)
        .css_classes(["suggested-action"])
        .build();
    button.set_widget_name(&format!("game-{}-primary", row.product_id));
    let downloading = ui.download.current.borrow().is_some();
    button.set_sensitive(
        if matches!(
            row.action,
            Action::Install | Action::Resume | Action::Update
        ) {
            row.action.is_enabled()
        } else {
            row.action.is_enabled_during_download(downloading)
        },
    );
    if row.unavailable.is_some() {
        button.remove_css_class("suggested-action");
    }

    match &row.action {
        Action::Blocked(reason) => button.set_tooltip_text(Some(reason)),
        Action::Installing => {
            button.set_tooltip_text(Some("Downloading — the bar at the bottom has the detail"));
            button.remove_css_class("suggested-action");
        }
        Action::Content => {
            button.set_tooltip_text(Some(
                "Installed add-on. Launch its base game to use this content.",
            ));
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
            // A bundle row runs its child, under the child's id.
            let running_id = match &row.install_as {
                Some((child, _))
                    if !ui
                        .running
                        .borrow()
                        .contains_key(&row.product_id.to_ascii_uppercase()) =>
                {
                    child.clone()
                }
                _ => row.product_id.clone(),
            };
            let asked = ui
                .running
                .borrow()
                .get(&running_id.to_ascii_uppercase())
                .is_some_and(|launch| launch.asked.is_some());
            let ready = ui
                .running
                .borrow()
                .get(&running_id.to_ascii_uppercase())
                .is_some_and(|launch| launch.ready);
            if ready {
                let resume = gtk::Button::with_label("Resume");
                resume.set_widget_name(&format!("game-{}-primary", row.product_id));
                resume.update_property(&[gtk::accessible::Property::Label(&format!(
                    "{} · Resume",
                    row.name
                ))]);
                resume.add_css_class("suggested-action");
                resume.set_tooltip_text(Some("Return to the running game's window"));
                let (resume_ui, id, name) = (ui.clone(), running_id.clone(), row.name.clone());
                resume.connect_clicked(move |_| window_activation::resume(&resume_ui, &id, &name));
                actions.append(&resume);
                ui.game_actions
                    .borrow_mut()
                    .insert(row.product_id.clone(), "Resume".into());
                button.set_widget_name(&format!("game-{}-stop", row.product_id));
            }
            button.set_label(if asked { "Force Stop" } else { "Stop" });
            button.set_tooltip_text(Some("Choose Stop to confirm before closing the game"));
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "{} · Stop",
                row.name
            ))]);
            button.remove_css_class("suggested-action");
            button.add_css_class("destructive-action");
            let product_id = running_id;
            let name = row.name.clone();
            button.connect_clicked(glib::clone!(
                #[strong]
                ui,
                move |_| confirm_stop_title(&ui, &product_id, &name)
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
            let waits = matches!(
                row.action,
                Action::Install | Action::Resume | Action::Update
            );
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
    actions.append(&button);
    action_row.set_activatable_widget(Some(&button));
    (action_row, actions)
}

fn render_account_button(ui: &Ui) {
    let model = ui.model.borrow();
    let badge = if model.account.is_some() {
        ui.presence.borrow().badge()
    } else {
        ferestre_core::presence::Badge::HIDDEN
    };
    let status = badge.label();
    let dot = presence_ui::dot(&badge);
    // The avatar is the Store account's. With a separate Player the dot goes
    // on the Player's line instead, so it is never on the wrong face.
    let split = model.roles.as_ref().is_some_and(|r| r.split());
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let avatar = adw::Avatar::new(28, model.account.as_ref().map(|a| a.label()), true);
    if let Some(path) = &model.avatar {
        if let Ok(texture) = gdk::Texture::from_filename(path) {
            avatar.set_custom_image(Some(&texture));
        }
    }
    match &dot {
        Some(dot) if !split => {
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&avatar));
            overlay.add_overlay(dot);
            content.append(&overlay);
        }
        _ => content.append(&avatar),
    }

    let (primary, secondary) = if let Some(roles) = &model.roles {
        let player = model
            .profiles
            .as_ref()
            .and_then(|p| p.profiles.iter().find(|p| p.id == roles.player))
            .map(|p| {
                format!(
                    "{}{}",
                    p.label,
                    if p.signed_in { "" } else { " (signed out)" }
                )
            })
            .unwrap_or_else(|| "not checked".into());
        (
            format!(
                "Store: {}",
                model
                    .account
                    .as_ref()
                    .map(|a| a.label())
                    .unwrap_or("signed out")
            ),
            format!("Player: {player}"),
        )
    } else {
        (
            model
                .account
                .as_ref()
                .map(|a| a.label())
                .unwrap_or("Not signed in")
                .to_string(),
            if model.ownership.is_known() {
                "Library loaded"
            } else if model.account.is_some() {
                "Load library"
            } else {
                "Sign in"
            }
            .into(),
        )
    };
    // Added after what is there, which stays as it was: "Player: <name>".
    let secondary = match &status {
        Some(status) => format!("{secondary} · {status}"),
        None => secondary,
    };
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
    labels.append(
        &gtk::Label::builder()
            .label(&primary)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    let secondary = gtk::Label::builder()
        .label(&secondary)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["caption", "dim-label"])
        .build();
    match dot {
        Some(dot) if split => {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            dot.set_valign(gtk::Align::Center);
            line.append(&dot);
            line.append(&secondary);
            labels.append(&line);
        }
        _ => labels.append(&secondary),
    }
    content.append(&labels);

    ui.account_button.set_child(Some(&content));
    let about = match &model.account {
        Some(_) => "This account, and what to do with it",
        None => "Sign in through the Xodus client",
    };
    ui.account_button.set_tooltip_text(Some(&match &status {
        Some(status) => format!("{about}\nStatus: {status}"),
        None => about.to_string(),
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
        if let Some(label) = button.child().and_downcast::<gtk::Label>() {
            label.set_xalign(0.0);
        }
        button
    };

    presence_ui::menu_section(ui, &items, &popover);

    let accounts = entry(
        "Accounts",
        "Choose the Store owner and Xbox player separately",
    );
    accounts.connect_clicked(glib::clone!(
        #[strong]
        ui,
        #[strong]
        popover,
        move |_| {
            popover.popdown();
            accounts_ui::open(&ui);
        }
    ));
    items.append(&accounts);

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

    let out = entry(
        "Sign out",
        "Remove the Store account's sign-in from this computer",
    );
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
        ui.toasts.add_toast(toast("The Xodus client was not found"));
        return;
    };
    if !begin(ui, "Signing out") {
        report_busy(ui);
        return;
    }
    let roles = ui.model.borrow().roles.clone();
    // The account being signed out, whose game runners must say goodbye to
    // Xbox while it still has a token: the Store account, which is the Player
    // too unless they are split.
    let leaving = roles
        .as_ref()
        .map(|r| r.store.clone())
        .or_else(|| presence_ui::player(ui));
    // Until the client has been started, closing the window must wait: the
    // runners are still saying goodbye, and a window gone before the client
    // starts would leave the person told they were signed out while the
    // client still holds the tokens.
    *ui.wait_on_close.borrow_mut() = true;
    show_activity(ui);
    // Before the client runs, not after: a window closed while it signs out
    // never reaches the code below, and the next one would open showing the
    // old account and its library from these files. If sign-out then fails,
    // the next account refresh writes them again.
    if let Some(state) = ui.model.borrow().private_state_dir() {
        state::forget_saved_account(&state);
    }
    if let Some(cache) = ui.model.borrow().account_cache_dir() {
        state::forget_saved_avatar(&cache);
    }
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        presence_ui::stop_for(&ui, leaving.as_deref()).await;
        let mut logout = scope_store(account::logout_command(&client), roles.as_ref());
        let failed = move |e: std::io::Error| format!("{}: {e}", client.display());
        let started = deadline::start(&mut logout).map_err(&failed);
        if started.is_ok() {
            // The saved account is already gone, and the client is its own
            // process, which a closed window does not stop.
            *ui.wait_on_close.borrow_mut() = false;
            note_close(
                &ui,
                "You can close the window. You will still be signed out.",
            );
        }
        let result = gio::spawn_blocking(move || {
            deadline::wait(&mut started?, deadline::CLIENT_QUESTION).map_err(failed)
        })
        .await;
        finish(&ui);
        match result {
            Ok(Ok(status)) if status.success() => {
                ui.model.borrow_mut().forget_account();
                render(&ui);
                ui.toasts.add_toast(toast("Signed out"));
            }
            Ok(Ok(status)) => ui
                .toasts
                .add_toast(toast(format!("Sign out exited {status}"))),
            Ok(Err(e)) => ui.toasts.add_toast(toast(&e)),
            Err(_) => ui.toasts.add_toast(toast("Could not run the client")),
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
/// A GUI launch may have no terminal. Surface missing media dependencies in
/// the window as well as in the CLI diagnostic, without delaying the launch.
fn warn_about_media(ui: &Ui, product_id: &str) {
    let runtime = {
        let model = ui.model.borrow();
        model.paths.as_ref().and_then(|paths| {
            let recipe = model.recipe_for(product_id)?;
            model
                .launch_runtime(recipe)
                .ok()
                // The Steam install too: Proton searches its FFmpeg 4 first.
                .map(|runtime| {
                    (
                        runtime.path.clone(),
                        paths.steam_dir().map(Path::to_path_buf),
                    )
                })
        })
    };
    let Some((runtime, steam)) = runtime else {
        return;
    };
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let checked = gio::spawn_blocking(move || {
            ferestre_core::media::missing_libav_dependencies(&runtime, steam.as_deref(), None)
        })
        .await;
        if let Ok(Ok(missing)) = checked {
            if !missing.is_empty() {
                let message = format!("Videos may be black or silent: missing {}. Fedora: install compat-ffmpeg4 from RPM Fusion. Run ferestre doctor for details.", missing.join(", "));
                ui.toasts.add_toast(
                    adw::Toast::builder()
                        .title(&message)
                        .use_markup(false)
                        .timeout(15)
                        .build(),
                );
            }
        }
    });
}

fn spawn_title(ui: &Ui, product_id: &str, name: &str) {
    let key = product_id.to_ascii_uppercase();
    // A second `ferestre run` in the same prefix would replace the first in
    // the window's records, and the first could no longer be stopped.
    if ui.running.borrow().contains_key(&key) {
        ui.toasts
            .add_toast(toast(format!("{name} is already running")));
        return;
    }
    let (roles, server, state_dir) = {
        let model = ui.model.borrow();
        if let Some(roles) = &model.roles {
            let result = model
                .profiles
                .as_ref()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Account identities are still loading. Open Accounts to check them."
                    )
                })
                .and_then(|profiles| roles.selected(profiles));
            if let Err(error) = result {
                ui.toasts.add_toast(
                    adw::Toast::builder()
                        .title(error.to_string())
                        .use_markup(false)
                        .priority(adw::ToastPriority::High)
                        .timeout(15)
                        .build(),
                );
                return;
            }
            if roles.split() {
                let recipe = model.recipe_for(product_id);
                let shares = recipe
                    .and_then(|recipe| model.launch_runtime(recipe).ok())
                    .is_some_and(|runtime| runtime.provides(accounts::CAPABILITY));
                if let Some(refusal) = model::account_sharing_refusal(recipe.is_some(), shares) {
                    ui.toasts.add_toast(
                        adw::Toast::builder()
                            .title(refusal)
                            .use_markup(false)
                            .priority(adw::ToastPriority::High)
                            .timeout(15)
                            .build(),
                    );
                    return;
                }
            }
        }
        (
            model.roles.clone(),
            model
                .recipe_for(product_id)
                .and_then(|r| model.wineserver(r)),
            model.paths.as_ref().map(|p| p.state_dir().to_path_buf()),
        )
    };
    warn_about_media(ui, product_id);
    use std::os::unix::process::CommandExt;

    let program = cli_binary();
    let mut command = std::process::Command::new(&program);
    command.arg("run").arg(product_id);
    let launch_tag = launch_state::tag();
    command.env(launch_state::TAG, &launch_tag);
    if let Some(roles) = &roles {
        command.env(
            "FERESTRE_ACCOUNT_ROLES",
            serde_json::to_string(roles).unwrap(),
        );
    } else {
        command.env_remove("FERESTRE_ACCOUNT_ROLES");
    }
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

    // The reason a launch failed is on the launcher's stderr ("not entitled to
    // this content", "Player account is signed out"), not in its exit status.
    // It goes to a file of its own rather than a pipe into this window: the
    // title outlives the window, and a pipe whose reader has gone made the
    // launcher's own output fail, losing the rest of the launch log. The tail
    // is read back when the launch ends.
    let output = state_dir
        .as_deref()
        .and_then(|dir| state::launch_output(dir, &key).ok());
    match &output {
        Some((_, file)) => match file.try_clone() {
            Ok(file) => command.stderr(file),
            Err(_) => command.stderr(std::process::Stdio::inherit()),
        },
        None => command.stderr(std::process::Stdio::inherit()),
    };
    let output = output.map(|(path, _)| path);
    // Without one, Stop still asks what it can reach from here, and ends the
    // rest.
    let channel = stop::Channel::attach(&mut command).ok();
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            ui.toasts
                .add_toast(toast(format!("{}: {e}", program.display())));
            return;
        }
    };

    // The launch's end of the channel goes with the command: only the launch
    // holds it from here on.
    drop(command);

    // setsid makes the child its own group leader, so the group id is its pid.
    let pgid = child.id();
    ui.running.borrow_mut().insert(
        key.clone(),
        stop::Launch {
            tag: launch_tag.clone(),
            ..stop::Launch::new(pgid, std::time::Instant::now())
        },
    );
    if let Some(server) = server {
        ui.running_servers.borrow_mut().insert(key.clone(), server);
    }
    match channel {
        Some(channel) => {
            ui.running_channels
                .borrow_mut()
                .insert(key.clone(), std::sync::Arc::new(channel));
        }
        None => {
            ui.running_channels.borrow_mut().remove(&key);
        }
    }
    presence_ui::launch_started(ui, &key, pgid);
    // Set once the launch has exited: the follower reads what is left, then
    // stops.
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let executable = ui
        .model
        .borrow()
        .recipe_for(product_id)
        .map(|r| r.launch.executable.clone());
    if let Some(executable) = executable {
        launch_state::watch(ui, &key, pgid, launch_tag, executable, finished.clone());
    }
    if let Some(path) = output.clone() {
        // The launch's runner reports on its stderr, which goes to the
        // launch's output file; follow it while the launch runs.
        let (sender, mut events) = futures_channel::mpsc::unbounded();
        let following = std::sync::Arc::clone(&finished);
        std::thread::spawn(move || {
            state::follow_lines(&path, &following, |line| {
                if let Some(event) = ferestre_core::presence::parse_relay(line) {
                    let _ = sender.unbounded_send(event);
                }
            });
        });
        glib::spawn_future_local(glib::clone!(
            #[strong]
            ui,
            #[strong]
            key,
            async move {
                use futures_util::StreamExt;
                while let Some(event) = events.next().await {
                    presence_ui::launch_event(&ui, &key, pgid, event);
                }
            }
        ));
    }
    render(ui);
    ui.toasts.add_toast(toast(format!("Starting {name}")));

    // Wait off the main thread, and deliberately without the `busy` flag: a
    // game runs for hours and must not hold up the rest of the window.
    let ui = ui.clone();
    let name = name.to_string();
    glib::spawn_future_local(async move {
        let mut child = child;
        let status = gio::spawn_blocking(move || {
            let status = child.wait();
            finished.store(true, std::sync::atomic::Ordering::Release);
            let reason = output
                .map(|path| state::tail_lines(&path, 64))
                .and_then(|lines| model::launch_failure(&lines));
            (status, reason)
        })
        .await;
        let stopped = ui
            .running
            .borrow()
            .get(&key)
            .is_some_and(|launch| launch.pgid == pgid && launch.asked.is_some());
        if ui
            .running
            .borrow()
            .get(&key)
            .is_some_and(|launch| launch.pgid == pgid)
        {
            ui.running.borrow_mut().remove(&key);
            ui.running_servers.borrow_mut().remove(&key);
            ui.running_channels.borrow_mut().remove(&key);
            presence_ui::launch_ended(&ui, &key);
        }
        render(&ui);
        match status {
            Ok((Ok(status), _)) if status.success() => {}
            // Asked to stop, and it did: the exit status says how it was
            // ended, not that anything went wrong.
            Ok((Ok(_), _)) if stopped => {
                ui.toasts.add_toast(toast(format!("{name} closed")));
            }
            Ok((Ok(status), reason)) => {
                if reason.as_deref().is_some_and(model::is_licence_refusal) {
                    ui.model.borrow_mut().refused.insert(key.clone());
                    render(&ui);
                }
                let code = status
                    .code()
                    .map_or_else(|| status.to_string(), |c| format!("with code {c}"));
                ui.toasts.add_toast(
                    adw::Toast::builder()
                        .title(match reason {
                            Some(reason) => format!("{name} stopped: {reason}"),
                            None => format!("{name} exited {code}"),
                        })
                        // The reason is a program's text, not markup: "<DIR>"
                        // in it made the toast fail to draw at all.
                        .use_markup(false)
                        .priority(adw::ToastPriority::High)
                        .timeout(20)
                        .build(),
                )
            }
            _ => ui
                .toasts
                .add_toast(toast(format!("{name}: could not be waited on"))),
        }
    });
}

/// Confirm the exact launch, so an old dialog cannot stop a replacement game.
fn confirm_stop_title(ui: &Ui, product_id: &str, name: &str) {
    let key = product_id.to_ascii_uppercase();
    let Some(launch) = ui.running.borrow().get(&key).cloned() else {
        return;
    };
    if ui.window.visible_dialog().is_some()
        || stop::press(&launch, std::time::Instant::now()) == stop::Press::Ignore
    {
        return;
    }
    let force = launch.asked.is_some();
    let dialog = adw::AlertDialog::new(
        Some(&format!(
            "{} {name}?",
            if force { "Force stop" } else { "Stop" }
        )),
        Some(if force {
            "The game has not closed yet. Force stopping can lose unsaved progress."
        } else {
            "Unsaved progress may be lost. Ferestre will ask the game to close before ending it."
        }),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("stop", if force { "Force Stop" } else { "Stop Game" });
    dialog.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let window = ui.window.clone();
    let ui = ui.clone();
    let name = name.to_string();
    dialog.connect_response(None, move |_, response| {
        if response != "stop" {
            return;
        }
        let action = stop::confirmed(ui.running.borrow().get(&key), &launch);
        match action {
            Some(stop::Press::Ask) => ask_title_to_close(&ui, &key, launch.pgid, &name),
            Some(stop::Press::End) => end_title(&ui, &key, launch.pgid, &name),
            _ => (),
        }
    });
    dialog.present(Some(&window));
}

/// The wineserver and prefix of a running title. Worked out again when the
/// launch had none to record: a title with no recipe gets one from `run` on
/// its way past, so by now there is one to read.
fn running_server(ui: &Ui, key: &str) -> Option<(PathBuf, PathBuf)> {
    if let Some(server) = ui.running_servers.borrow().get(key).cloned() {
        return Some(server);
    }
    if ui.model.borrow().recipe_for(key).is_none() {
        ui.model.borrow_mut().reload();
    }
    let server = {
        let model = ui.model.borrow();
        model.recipe_for(key).and_then(|r| model.wineserver(r))
    };
    if let Some(server) = &server {
        ui.running_servers
            .borrow_mut()
            .insert(key.to_string(), server.clone());
    }
    server
}

fn ask_title_to_close(ui: &Ui, key: &str, pgid: u32, name: &str) {
    let Some(server) = running_server(ui, key) else {
        // Nothing to ask through: no runtime or prefix is known for it.
        end_title(ui, key, pgid, name);
        return;
    };
    let asked_at = std::time::Instant::now();
    if let Some(launch) = ui.running.borrow_mut().get_mut(key) {
        launch.asked = Some(asked_at);
    }
    render(ui);
    let channel = ui.running_channels.borrow().get(key).cloned();
    let ui = ui.clone();
    let key = key.to_string();
    let name = name.to_string();
    glib::spawn_future_local(async move {
        let (wineserver, prefix) = server;
        let asked = gio::spawn_blocking(move || {
            stop::ask_to_close(&wineserver, &prefix, channel.as_deref())
        })
        .await
        .unwrap_or(false);
        if !still_asking(&ui, &key, pgid, asked_at) {
            return;
        }
        if !asked {
            // No window to ask: it is still starting, or has none.
            end_title(&ui, &key, pgid, &name);
            return;
        }
        ui.toasts.add_toast(toast(format!(
            "Asked {name} to close. Press Stop again to end it now."
        )));
        let remaining = stop::CLOSE_WAIT.saturating_sub(asked_at.elapsed());
        glib::timeout_add_local_once(remaining, move || {
            if still_asking(&ui, &key, pgid, asked_at) {
                end_title(&ui, &key, pgid, &name);
            }
        });
    });
}

/// Whether the launch asked to close at `asked_at` is still running and still
/// waiting on that request: not exited, not relaunched.
fn still_asking(ui: &Ui, key: &str, pgid: u32, asked_at: std::time::Instant) -> bool {
    ui.running
        .borrow()
        .get(key)
        .is_some_and(|launch| launch.pgid == pgid && launch.asked == Some(asked_at))
}

fn end_title(ui: &Ui, key: &str, pgid: u32, name: &str) {
    // Marked as stopped on purpose, so its exit is not reported as a failure.
    if let Some(launch) = ui.running.borrow_mut().get_mut(key) {
        launch.asked.get_or_insert_with(std::time::Instant::now);
    }
    let server = running_server(ui, key);
    if let Some(child) = stop::end(pgid, server.as_ref()) {
        // Reaped, because stopping a title is something people do more than
        // once and an unwaited child is a zombie each time.
        glib::spawn_future_local(async move {
            let mut child = child;
            let _ = gio::spawn_blocking(move || child.wait()).await;
        });
    }
    ui.toasts.add_toast(toast(format!("Ending {name}")));
}

fn spawn_cli(ui: &Ui, args: &[&str]) {
    let program = cli_binary();
    match std::process::Command::new(&program).args(args).spawn() {
        Ok(_) => ui
            .toasts
            .add_toast(toast(format!("Started: ferestre {}", args.join(" ")))),
        Err(e) => ui
            .toasts
            .add_toast(toast(format!("{}: {e}", program.display()))),
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
    cancel: gtk::Button,
    pause: gtk::Button,
    snapshot: Arc<Mutex<Option<progress::Snapshot>>>,
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
        let cancel = gtk::Button::builder()
            .label("Cancel")
            .halign(gtk::Align::End)
            .build();
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        controls.set_halign(gtk::Align::End);
        let pause = gtk::Button::with_label("Pause");
        pause.set_visible(false);
        controls.append(&pause);
        controls.append(&cancel);
        root.append(&controls);
        DownloadBar {
            pause,
            snapshot: Arc::new(Mutex::new(None)),
            root,
            title,
            detail,
            bar,
            cancel,
            seen: Arc::new(Mutex::new(None)),
            current: Rc::new(RefCell::new(None)),
        }
    }

    fn start(&self, product_id: &str, name: &str, what: &str) {
        let download = progress::Download::new(product_id, name, what, std::time::Instant::now());
        *self.seen.lock().unwrap() = None;
        *self.snapshot.lock().unwrap() = None;
        self.pause.set_visible(false);
        self.pause.set_label("Pause");
        self.title.set_label(&download.heading());
        *self.current.borrow_mut() = Some(download);
        self.cancel.set_sensitive(true);
        self.cancel.set_label(if what == "Moving" {
            "Cancel Move"
        } else {
            "Stop Download"
        });
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
        let snapshot = self.snapshot.lock().unwrap().clone();
        self.pause.set_visible(
            snapshot.as_ref().is_some_and(|s| s.can_pause)
                && matches!(download.what.as_str(), "Installing" | "Updating"),
        );
        let detail = if let Some(snapshot) = snapshot {
            self.pause
                .set_label(if snapshot.paused { "Resume" } else { "Pause" });
            let status = if snapshot.paused {
                "Paused — progress is kept".into()
            } else {
                download.detail(now)
            };
            match (snapshot.network_bytes, snapshot.reused_bytes) {
                (Some(network), Some(reused)) => format!(
                    "{status}  ·  {} downloaded  ·  {} reused",
                    progress::bytes(network),
                    progress::bytes(reused)
                ),
                _ => status,
            }
        } else {
            download.detail(now)
        };
        self.detail.set_label(&detail);
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
fn spawn_install(ui: &Ui, product_id: &str, name: &str, dir: Option<&Path>, what: &str) {
    queue_ui::enqueue(ui, product_id, name, dir, what);
}

fn spawn_transfer(
    ui: &Ui,
    product_id: &str,
    name: &str,
    dir: Option<&Path>,
    what: &str,
    command_name: &str,
) {
    if !begin(ui, &format!("{what} {name}")) {
        report_busy(ui);
        return;
    }
    let control = cancel::DownloadControl::default();
    *ui.download_control.borrow_mut() = Some(control.clone());
    show_activity(ui);
    let program = cli_binary();
    let mut args: Vec<std::ffi::OsString> = vec![command_name.into(), product_id.into()];
    if let Some(dir) = dir {
        args.push(dir.as_os_str().to_owned());
    }

    ui.download.start(product_id, name, what);
    render(ui);
    let seen = Arc::clone(&ui.download.seen);
    let snapshot = Arc::clone(&ui.download.snapshot);
    let failure = Arc::new(Mutex::new(None::<ferestre_core::diagnostics::Diagnostic>));
    let failure_read = failure.clone();

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
    let name = name.to_string();
    let moving = command_name == "move";
    let product_id = product_id.to_string();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            let mut command = std::process::Command::new(&program);
            command
                .args(&args)
                .env("XODUS_PROGRESS", "json")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut child = control
                .spawn(&mut command)
                .map_err(|e| format!("{}: {e}", program.display()))?;

            let errors = child.stderr.take().map(|stderr| {
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(stderr)
                        .lines()
                        .map_while(Result::ok)
                    {
                        if let Some(diagnostic) =
                            ferestre_core::diagnostics::Diagnostic::parse(&line)
                        {
                            *failure_read.lock().unwrap() = Some(diagnostic);
                        }
                        eprintln!("{line}");
                    }
                })
            });
            if let Some(out) = child.stdout.take() {
                use std::io::BufRead;
                let mut reader = progress::Reader::default();
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    match reader.read(&line) {
                        Some(progress::Update::Structured(update)) => {
                            *seen.lock().unwrap() =
                                Some((update.completed_bytes, update.total_bytes.unwrap_or(0)));
                            *snapshot.lock().unwrap() = Some(update);
                        }
                        Some(progress::Update::Legacy(done, total)) => {
                            *seen.lock().unwrap() = Some((done, total))
                        }
                        None => println!("{line}"),
                    }
                }
            }

            let status = child
                .wait()
                .map_err(|e| format!("{}: {e}", program.display()));
            if let Some(errors) = errors {
                let _ = errors.join();
            }
            status
        })
        .await;

        tick.remove();
        ui.download.stop();
        let stopped = ui
            .download_control
            .borrow()
            .as_ref()
            .is_some_and(|c| c.requested());
        if !moving {
            queue_ui::completed(
                &ui,
                &product_id,
                matches!(&result,Ok(Ok(status)) if status.success()),
                stopped,
            );
        }
        finish(&ui);
        match result {
            Ok(Ok(status)) if status.success() => {
                ui.model.borrow_mut().reload();
                ui.toasts
                    .add_toast(toast(format!("Finished {} {name}", what.to_lowercase())));
            }
            _ if stopped => {
                ui.model.borrow_mut().reload();
                ui.toasts.add_toast(toast(if moving {
                    "Move stopped. Original installation kept."
                } else {
                    "Download stopped. Downloaded files kept for resuming."
                }));
            }
            Ok(Ok(status)) => {
                ui.model.borrow_mut().reload();
                ui.toasts.add_toast(toast(
                    failure
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|d| d.message())
                        .unwrap_or_else(|| {
                            format!(
                                "{what} {name} failed ({status}). Open Downloads → Diagnostics."
                            )
                        }),
                ));
            }
            Ok(Err(e)) => ui.toasts.add_toast(toast(&e)),
            Err(_) => ui
                .toasts
                .add_toast(toast("The install could not be started")),
        }
        render(&ui);
    });
}

fn spawn_cli_tracked(ui: &Ui, args: &[&str], what: &str) {
    if !begin(ui, what) {
        report_busy(ui);
        return;
    }
    *ui.wait_on_close.borrow_mut() = args.first() == Some(&"uninstall");
    let program = cli_binary();
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let runtime_install = owned.first().map(String::as_str) == Some("install-runtime");
    let control = runtime_install.then(cancel::DownloadControl::default);
    *ui.download_control.borrow_mut() = control.clone();
    show_activity(ui);
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
        .add_toast(toast(format!("{what} — this can take a while")));

    let ui = ui.clone();
    let what = what.to_string();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            let mut command = std::process::Command::new(&program);
            command.args(&owned).stdin(std::process::Stdio::null());
            let child = match control {
                Some(control) => control.spawn(&mut command),
                None => command.spawn(),
            };
            child
                .and_then(|mut child| child.wait())
                .map_err(|e| format!("{}: {e}", program.display()))
        })
        .await;
        if let Some(tick) = tick {
            tick.remove();
            ui.download.stop();
        }
        let stopped = ui
            .download_control
            .borrow()
            .as_ref()
            .is_some_and(|c| c.requested());
        finish(&ui);
        let mut asked = false;
        match result {
            Ok(Ok(status)) if status.success() => {
                // The record was written by the child, so the window has to
                // re-read it before it can say anything different.
                ui.model.borrow_mut().reload();
                render(&ui);
                ui.toasts.add_toast(toast(format!("{what} finished")));
            }
            _ if stopped => {
                ui.toasts.add_toast(toast("Runtime download stopped"));
            }
            Ok(Ok(status))
                if runtime_install
                    && status.code()
                        == Some(i32::from(ferestre_core::runtime::EXIT_DOWNGRADE_REFUSED)) =>
            {
                confirm_runtime_downgrade(&ui);
                asked = true;
            }
            Ok(Ok(status)) => {
                ui.model.borrow_mut().reload();
                render(&ui);
                ui.toasts
                    .add_toast(toast(format!("{what} exited {status}")))
            }
            Ok(Err(e)) => ui.toasts.add_toast(toast(&e)),
            Err(_) => ui
                .toasts
                .add_toast(toast(format!("{what}: the process crashed"))),
        }
        // On a first run the runtime download is started instead of the
        // account refresh, which then never happened: Play stayed refused
        // with "identities are still loading" for the whole session. Not
        // while the downgrade question is open, though: the refresh holds
        // the window busy for up to a minute, and an answer of "install"
        // in that time was refused as busy. The dialog refreshes instead.
        if runtime_install && !asked {
            app_updates::invalidate(&ui);
            refresh_account(&ui);
        }
    });
}

/// `install-runtime` refused because the newest published runtime is older
/// than one already installed. That is only ever installed when the person
/// says so: an older release passed off as the newest is how a fixed bug
/// would come back.
fn confirm_runtime_downgrade(ui: &Ui) {
    let dialog = adw::AlertDialog::new(
        Some("Install an older runtime?"),
        Some(
            "The newest published runtime is older than one already installed. \
             The installed runtimes are kept either way, and new launches keep \
             using the newest one unless a title is set to another.\n\n\
             Only install it if you meant to go back to that release.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("install", "Install Older Runtime");
    dialog.set_response_appearance("install", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let window = ui.window.clone();
    let ui = ui.clone();
    dialog.connect_response(None, move |_, response| {
        // The account refresh the first try skipped: the install refreshes
        // when it finishes, and otherwise it is started (or owed) here.
        let busy = ui.busy.borrow().is_some();
        if response == "install" {
            spawn_cli_tracked(
                &ui,
                &["install-runtime", "--allow-downgrade"],
                "Downloading the runtime",
            );
        }
        if response != "install" || busy {
            refresh_account(&ui);
        }
    });
    dialog.present(Some(&window));
}

/// Ask before downloading, and say where it is going.
///
/// A download here is between a few hundred megabytes and 160 GB, so starting
/// one on a single click -- with no size, no destination, and no way back --
/// is not a thing to do to somebody's evening or their disk.
fn confirm_install(ui: &Ui, product_id: &str, name: &str, updating: bool) {
    if *ui.closing.borrow()
        || (ui.busy.borrow().is_some() && ui.download.current.borrow().is_none())
    {
        report_busy(ui);
        return;
    }
    // Rewriting a title's files while it runs leaves it reading half of each
    // build.
    if ui
        .running
        .borrow()
        .contains_key(&product_id.to_ascii_uppercase())
    {
        ui.toasts
            .add_toast(toast(format!("Stop {name} before updating it")));
        return;
    }
    let (target, size) = {
        let model = ui.model.borrow();
        let product = model.catalog.get(&product_id.to_ascii_uppercase());
        let size = product.map(|p| p.size_label()).filter(|s| !s.is_empty());
        (model.install_target(product_id), size)
    };
    let Some((library, folder, _)) = target else {
        ui.toasts
            .add_toast(toast("Cannot work out where to put it"));
        return;
    };

    let heading = if updating {
        format!("Update {name}?")
    } else {
        format!("Install {name}?")
    };
    let body = match &size {
        Some(size) => format!("{size} to download."),
        None => "The catalog does not give a size.".to_string(),
    };

    let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("install", if updating { "Update" } else { "Install" });
    dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("install"));
    dialog.set_close_response("cancel");

    // The destination is editable, because "somewhere on your smallest disk" is
    // not a decision to make on someone's behalf for 160 GB. It names the
    // library folder; the title gets a folder of its own inside it, said
    // below the field as it is typed.
    let where_to = adw::EntryRow::builder()
        .title("Install to")
        .text(library.to_string_lossy().as_ref())
        .build();
    let destination = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .margin_top(6)
        .css_classes(["caption", "dim-label"])
        .build();
    let browse = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Choose install folder")
        .valign(gtk::Align::Center)
        .build();
    browse.update_property(&[gtk::accessible::Property::Label("Choose install folder")]);
    where_to.add_suffix(&browse);
    {
        let (ui, entry) = (ui.clone(), where_to.clone());
        browse.connect_clicked(move |_| folder_picker::open(&ui, entry.upcast_ref()));
    }
    let group = adw::PreferencesGroup::new();
    group.add(&where_to);
    let extra = gtk::Box::new(gtk::Orientation::Vertical, 0);
    extra.append(&group);
    extra.append(&destination);
    dialog.set_extra_child(Some(&extra));

    let home = std::env::var_os("HOME").map(PathBuf::from);
    type Resolve = Rc<dyn Fn(&str) -> Result<PathBuf, String>>;
    let resolve: Resolve = {
        let ui = ui.clone();
        let product_id = product_id.to_string();
        Rc::new(move |text: &str| {
            let model = ui.model.borrow();
            model::install_destination(text, home.as_deref(), &folder, &|dir| {
                model.inspect_folder(&product_id, dir)
            })
        })
    };
    let show = {
        let resolve = Rc::clone(&resolve);
        let dialog = dialog.clone();
        let destination = destination.clone();
        move |text: &str| match resolve(text) {
            Ok(dir) => {
                let free = free_space_label(&dir).unwrap_or_default();
                destination.set_label(&format!("Installs into {}. {free}", dir.display()));
                destination.remove_css_class("error");
                dialog.set_response_enabled("install", true);
            }
            Err(why) => {
                destination.set_label(&why);
                destination.add_css_class("error");
                dialog.set_response_enabled("install", false);
            }
        }
    };
    show(&where_to.text());
    where_to.connect_changed(move |entry| show(&entry.text()));

    let window = ui.window.clone();
    let ui = ui.clone();
    let product_id = product_id.to_string();
    let name = name.to_string();
    dialog.connect_response(None, move |_, response| {
        if response != "install" {
            return;
        }
        let what = if updating { "Updating" } else { "Installing" };
        match resolve(&where_to.text()) {
            Ok(dir) => {
                // `ferestre install` takes the folder as the library, as the
                // field does, and finds the title's folder in it by the same
                // rule. Only a folder that is already this title's -- its
                // install, its stopped download, or its default folder --
                // goes as it is, since its name need not be the one a fresh
                // install would get.
                let argument = ui.model.borrow().install_argument(&product_id, &dir);
                match argument {
                    Some(argument) => {
                        spawn_install(&ui, &product_id, &name, Some(argument.as_path()), what)
                    }
                    None => ui
                        .toasts
                        .add_toast(toast("Cannot work out where to put it")),
                }
            }
            Err(why) => ui.toasts.add_toast(toast(why)),
        }
    });
    dialog.present(Some(&window));
}

/// Ask before removing a title, and say exactly what will go.
///
/// The path and the size are both in the question, because "remove" is a
/// different amount of work to undo depending on whether it is 4 MB of a failed
/// download or 45 GB of a game -- and because a launcher deleting a directory
/// the person cannot see named is a launcher nobody should trust.
///
/// The folder is the one `ferestre uninstall` will delete, found the same way,
/// so a download that stopped before anything recorded it can be removed too.
/// Sizes are measured off the window's thread: walking a large title on a
/// cold disk froze the window before the question even appeared.
fn confirm_remove(ui: &Ui, product_id: &str, name: &str) {
    if ui.busy.borrow().is_some() || *ui.closing.borrow() {
        report_busy(ui);
        return;
    }
    let key = product_id.to_ascii_uppercase();
    if ui.running.borrow().contains_key(&key) {
        ui.toasts
            .add_toast(toast(format!("Stop {name} before removing it")));
        return;
    }
    let (dir, prefixes, measured) = {
        let model = ui.model.borrow();
        (
            model.removal_dir(product_id),
            model.prefixes(product_id),
            current_disk_usage(&model, &key),
        )
    };
    let Some(dir) = dir.filter(|dir| dir.is_dir()) else {
        ui.toasts
            .add_toast(toast(format!("Nothing of {name} is on disk to remove")));
        return;
    };
    // A library recorded as the title's folder is never offered for
    // deletion: `ferestre uninstall` would refuse it anyway, after the person
    // had agreed to lose the whole library.
    let library = ui.model.borrow().removal_library(product_id, &dir);
    if let Some(why) = library {
        confirm_forget_library(ui, product_id, name, &dir, &why);
        return;
    }

    let body =
        {
            let dir = dir.clone();
            move |size: Option<u64>| {
                format!(
                "{} will be deleted, freeing {}.\n\nThe title stays in your library and can be \
                 installed again.",
                dir.display(),
                size.map_or_else(|| "space (measuring…)".to_string(), ferestre_core::human_bytes)
            )
            }
        };
    let dialog = adw::AlertDialog::new(Some(&format!("Remove {name}?")), Some(&body(measured)));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Remove");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");

    // The Wine prefix is the title's C: drive, so it is where saved games are.
    // Off by default, and it says why: removing it silently along with the
    // files throws away the one thing somebody would want back, and leaving it
    // unmentioned is how a `stardew-valley-proton` sits there afterwards with
    // nothing explaining what it is. With account roles, each Player has a
    // copy of its own, and every one is listed.
    let base = prefixes
        .first()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf);
    let prefix_label = {
        let names: Vec<String> = prefixes
            .iter()
            .map(|path| {
                base.as_deref()
                    .and_then(|base| path.strip_prefix(base).ok())
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .collect();
        move |sizes: Option<&[u64]>| {
            let lines: Vec<String> = names
                .iter()
                .enumerate()
                .map(|(i, name)| match sizes.and_then(|s| s.get(i)) {
                    Some(size) => format!("{name}, {}", ferestre_core::human_bytes(*size)),
                    None => format!("{name}, measuring…"),
                })
                .collect();
            format!(
                "Also delete saved games and settings:\n{}",
                lines.join("\n")
            )
        }
    };
    let also_prefix = (!prefixes.is_empty()).then(|| {
        let check = gtk::CheckButton::builder()
            .label(prefix_label(None))
            .active(false)
            .margin_top(6)
            .build();
        dialog.set_extra_child(Some(&check));
        check
    });

    {
        let dialog = dialog.clone();
        let check = also_prefix.clone();
        let dir = dir.clone();
        let prefixes = prefixes.clone();
        glib::spawn_future_local(async move {
            let measured_dir = measured;
            let sizes = gio::spawn_blocking(move || {
                let dir_size = match measured_dir {
                    Some(size) => size,
                    None => ferestre_core::install::tree_size(&dir),
                };
                let prefix_sizes: Vec<u64> = prefixes
                    .iter()
                    .map(|p| ferestre_core::install::tree_size(p))
                    .collect();
                (dir_size, prefix_sizes)
            })
            .await;
            if let Ok((dir_size, prefix_sizes)) = sizes {
                dialog.set_body(&body(Some(dir_size)));
                if let Some(check) = check {
                    check.set_label(Some(&prefix_label(Some(&prefix_sizes))));
                }
            }
        });
    }

    let window = ui.window.clone();
    let ui = ui.clone();
    let product_id = product_id.to_string();
    let name = name.to_string();
    dialog.connect_response(None, move |_, response| {
        if response != "remove" {
            return;
        }
        if ui.running.borrow().contains_key(&key) {
            ui.toasts
                .add_toast(toast(format!("Stop {name} before removing it")));
            return;
        }
        let with_prefixes = also_prefix
            .as_ref()
            .is_some_and(gtk::prelude::CheckButtonExt::is_active);
        let Some(args) =
            model::uninstall_args(&product_id, with_prefixes.then_some(prefixes.as_slice()))
        else {
            ui.toasts.add_toast(toast(format!(
                "Cannot remove {name}: a Wine prefix's folder name is not valid text"
            )));
            return;
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        spawn_cli_tracked(&ui, &args, &format!("Removing {name}"));
    });
    dialog.present(Some(&window));
}

/// Remove for a title whose record names a library of games rather than a
/// folder of its own: nothing is deleted, and the only thing offered is to
/// forget the record and leave every file where it is.
fn confirm_forget_library(ui: &Ui, product_id: &str, name: &str, dir: &Path, why: &str) {
    let dialog = adw::AlertDialog::new(
        Some(&format!("Forget {name}?")),
        Some(&format!(
            "{name} is recorded in {}, but {why}, so it is a library of games rather \
             than {name}'s own folder. It cannot be removed as one title.\n\n\
             Forgetting {name} deletes nothing: its files stay where they are, for \
             you to delete yourself. Installing it again puts it in a folder of its own.",
            dir.display()
        )),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("forget", "Forget");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let window = ui.window.clone();
    let ui = ui.clone();
    let key = product_id.to_ascii_uppercase();
    let product_id = product_id.to_string();
    let name = name.to_string();
    dialog.connect_response(None, move |_, response| {
        if response != "forget" {
            return;
        }
        if ui.running.borrow().contains_key(&key) {
            ui.toasts
                .add_toast(toast(format!("Stop {name} before forgetting it")));
            return;
        }
        let args = model::forget_args(&product_id);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        spawn_cli_tracked(&ui, &args, &format!("Forgetting {name}"));
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
        let mut recipe = model.recipe_for(product_id).cloned().unwrap_or_else(|| {
            let mut blank = Recipe::blank(product_id, name);
            // Where the title is, not where its name's slug would put it: a
            // title found by its package in a folder of another name, or left
            // in games/<product id> by an install with no recipe, otherwise
            // got a recipe pointing at a folder that does not exist.
            blank.install.dir = model
                .paths
                .as_ref()
                .zip(model.product_dir(product_id))
                .and_then(|(paths, dir)| editor::relative_install_dir(paths.games_dir(), &dir));
            blank
        });
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
            .add_toast(toast("Nowhere to save an edit: HOME is not set"));
        return;
    };

    let window = ui.window.clone();
    let ui = ui.clone();
    editor::present(&window, recipe, titles_dir, move |path| {
        ui.model.borrow_mut().reload();
        render(&ui);
        ui.toasts.add_toast(toast(format!(
            "Saved {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )));
    });
}

fn add_to_steam(ui: &Ui, product_id: &str, name: &str) {
    add_to_steam_titles(ui, vec![(product_id.to_string(), name.to_string())]);
}

/// Where Steam keeps shortcuts, or a toast saying why there is nowhere to put
/// them.
fn steam_shortcut_files(ui: &Ui) -> Option<Vec<PathBuf>> {
    // Steam rewrites shortcuts.vdf from memory when it exits, so an edit made
    // underneath a running Steam is simply lost. Refusing is the only honest
    // behaviour; writing it and saying nothing would look like a bug in Steam.
    if steam::is_running() {
        ui.toasts.add_toast(toast(
            "Close Steam first — it overwrites its shortcuts file when it exits",
        ));
        return None;
    }

    let steam_dir = ui
        .model
        .borrow()
        .paths
        .as_ref()
        .and_then(|p| p.steam_dir().map(Path::to_owned));
    let Some(steam_dir) = steam_dir else {
        ui.toasts.add_toast(toast("No Steam installation found"));
        return None;
    };
    let files = steam::shortcut_files(&steam_dir);
    if files.is_empty() {
        ui.toasts.add_toast(toast(
            "No Steam account on this machine has a shortcuts file",
        ));
        return None;
    }
    Some(files)
}

/// Ask before giving every installed title a Steam shortcut. One already in
/// Steam is updated in place, not added twice.
fn confirm_add_all_to_steam(ui: &Ui, titles: Vec<(String, String)>) {
    if ui.busy.borrow().is_some() || *ui.closing.borrow() {
        report_busy(ui);
        return;
    }
    if titles.is_empty() || steam_shortcut_files(ui).is_none() {
        return;
    }
    let count = titles.len();
    let heading = if count == 1 {
        "Add 1 game to Steam?".to_string()
    } else {
        format!("Add {count} games to Steam?")
    };
    let dialog = adw::AlertDialog::new(
        Some(&heading),
        Some(
            "Every installed game gets a Steam shortcut that starts it through Ferestre, with \
             its artwork. Games already in Steam are updated, not added twice.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("add", "Add All");
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("cancel");
    let titles = std::cell::Cell::new(Some(titles));
    dialog.connect_response(
        None,
        glib::clone!(
            #[strong]
            ui,
            move |_, response| {
                if response == "add" {
                    if let Some(titles) = titles.take() {
                        add_to_steam_titles(&ui, titles);
                    }
                }
            }
        ),
    );
    dialog.present(Some(&ui.window));
}

/// Give each title a Steam shortcut, in every Steam account's shortcuts file,
/// reading and writing each file once.
fn add_to_steam_titles(ui: &Ui, titles: Vec<(String, String)>) {
    let Some(files) = steam_shortcut_files(ui) else {
        return;
    };
    let busy = match titles.as_slice() {
        [(_, name)] => format!("Adding {name} to Steam"),
        _ => format!("Adding {} games to Steam", titles.len()),
    };
    if !begin(ui, &busy) {
        report_busy(ui);
        return;
    }
    // Closing waits: a shortcuts file left half way is a Steam library
    // missing games the person was told were being added.
    *ui.wait_on_close.borrow_mut() = true;
    show_activity(ui);
    let (cache, market, known) = {
        let model = ui.model.borrow();
        let known: Vec<_> = titles
            .iter()
            .map(|(product_id, _)| {
                let key = product_id.to_ascii_uppercase();
                (
                    model.catalog.get(&key).cloned(),
                    model.icons.get(&key).cloned(),
                )
            })
            .collect();
        (model.catalog_cache(), model.market(), known)
    };
    let program = steam::shortcut_program(
        std::env::var_os("APPIMAGE").as_deref(),
        std::env::var_os("APPDIR").as_deref(),
        &cli_binary(),
    );
    let start_dir = program.parent().unwrap_or(Path::new("/")).to_path_buf();
    let single = (titles.len() == 1).then(|| titles[0].1.clone());
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || -> anyhow::Result<_> {
            let mut failures = Vec::new();
            let mut products: Vec<_> = titles
                .iter()
                .zip(&known)
                .map(|((product_id, _), (product, _))| {
                    product
                        .clone()
                        .or_else(|| cache.as_ref().and_then(|c| c.get(product_id)))
                })
                .collect();
            if let Some(cache) = &cache {
                // Older metadata caches have only the small list icon. Refresh
                // the titles missing larger library assets, in one request.
                let stale: Vec<String> = titles
                    .iter()
                    .zip(&products)
                    .filter(|(_, product)| product.as_ref().is_none_or(|p| p.artwork.is_empty()))
                    .map(|((product_id, _), _)| product_id.clone())
                    .collect();
                if !stale.is_empty() {
                    let (found, errors) =
                        catalog::revalidate(cache, &stale, &market.0, &market.1, true);
                    failures.extend(errors);
                    for fresh in found {
                        if let Some(slot) = titles
                            .iter()
                            .position(|(id, _)| id.eq_ignore_ascii_case(&fresh.product_id))
                        {
                            products[slot] = Some(fresh);
                        }
                    }
                }
            }
            let prepared: Vec<_> = titles
                .iter()
                .zip(products)
                .zip(&known)
                .map(|(((product_id, name), product), (_, fallback_icon))| {
                    // A row still showing its product id is named after the
                    // catalogue's answer, when the worker got one.
                    let name = product
                        .as_ref()
                        .filter(|_| name.eq_ignore_ascii_case(product_id))
                        .map(|p| p.name.trim())
                        .filter(|n| !n.is_empty())
                        .unwrap_or(name.as_str())
                        .to_string();
                    let artwork = match (&cache, product) {
                        (Some(cache), Some(product)) => {
                            let (files, errors) =
                                steam::prepare_artwork(cache, &product, fallback_icon.as_deref());
                            failures.extend(errors);
                            files
                        }
                        _ => steam::ArtworkFiles {
                            icon: fallback_icon.clone(),
                            ..Default::default()
                        },
                    };
                    let shortcut = steam::Shortcut::new(
                        &name,
                        &program,
                        &start_dir,
                        format!("run {product_id}"),
                    );
                    (shortcut, artwork)
                })
                .collect();
            anyhow::ensure!(
                !steam::is_running(),
                "Close Steam first; it started while artwork was being fetched"
            );
            // Per title: whether any account got a new shortcut, and whether
            // any account has it at all now.
            let mut added = vec![false; prepared.len()];
            let mut done = vec![false; prepared.len()];
            let mut accounts_failed = Vec::new();
            for file in &files {
                let result = steam::load(file).and_then(|mut document| {
                    let mut outcomes = Vec::new();
                    for (shortcut, artwork) in &prepared {
                        let mut shortcut = steam::distinct(&document, shortcut);
                        let app_id = steam::effective_app_id(&document, &shortcut);
                        let previous_icon = steam::existing_shortcut(&document, &shortcut)
                            .and_then(|entry| entry.get("icon"))
                            .and_then(steam::Value::as_str)
                            .filter(|path| {
                                !path.is_empty()
                                    && Path::new(path).is_file()
                                    && *path != program.to_string_lossy()
                            })
                            .map(str::to_string);
                        let (icon, errors) = steam::install_artwork(file, app_id, artwork);
                        failures.extend(errors);
                        shortcut.icon = previous_icon
                            .or_else(|| icon.map(|p| p.to_string_lossy().into_owned()))
                            .unwrap_or_default();
                        outcomes.push(steam::upsert(&mut document, &shortcut)?);
                    }
                    steam::save(file, &document)?;
                    Ok(outcomes)
                });
                match result {
                    Ok(outcomes) => {
                        for (slot, replaced) in outcomes.into_iter().enumerate() {
                            done[slot] = true;
                            added[slot] |= !replaced;
                        }
                    }
                    Err(error) => accounts_failed.push(format!("{}: {error}", file.display())),
                }
            }
            let added_count = added.iter().filter(|a| **a).count();
            let updated_count = done.iter().filter(|d| **d).count() - added_count;
            Ok((added_count, updated_count, failures, accounts_failed))
        })
        .await;
        finish(&ui);
        match result {
            Ok(Ok((added, updated, failures, accounts_failed))) if added + updated > 0 => {
                let what = match &single {
                    Some(name) => {
                        let action = if added == 0 { "Updated" } else { "Added" };
                        format!("{action} \"{name}\" in Steam.")
                    }
                    None => steam_summary(added, updated),
                };
                let them = if single.is_some() { "it" } else { "them" };
                let detail = if failures.is_empty() {
                    format!("Restart Steam to see {them}.")
                } else {
                    format!("Some artwork was unavailable. Restart Steam to see {them}.")
                };
                ui.toasts.add_toast(toast(format!("{what} {detail}")));
                // Said apart from artwork: one Steam account got none of them.
                if !accounts_failed.is_empty() {
                    ui.toasts.add_toast(toast(format!(
                        "Could not update {} of the Steam accounts on this machine: {}",
                        accounts_failed.len(),
                        accounts_failed.join("; ")
                    )));
                }
            }
            Ok(Ok((_, _, failures, accounts_failed))) => ui.toasts.add_toast(toast(format!(
                "Could not add to Steam: {}",
                accounts_failed
                    .into_iter()
                    .chain(failures)
                    .collect::<Vec<_>>()
                    .join("; ")
            ))),
            Ok(Err(error)) => ui.toasts.add_toast(toast(error.to_string())),
            Err(_) => ui
                .toasts
                .add_toast(toast("Could not finish adding to Steam")),
        }
    });
}

/// "Added 3 games and updated 2 in Steam."
fn steam_summary(added: usize, updated: usize) -> String {
    let games = |n: usize| format!("{n} {}", if n == 1 { "game" } else { "games" });
    match (added, updated) {
        (0, updated) => format!("Updated {} in Steam.", games(updated)),
        (added, 0) => format!("Added {} to Steam.", games(added)),
        (added, updated) => format!("Added {} and updated {updated} in Steam.", games(added)),
    }
}

fn account_pressed(ui: &Ui) {
    if ui.model.borrow().account.is_some() {
        account_menu(ui);
        return;
    }
    // A client the loader refuses cannot sign in: say why and offer the fix.
    if client_check::blocks(ui, account_pressed) {
        return;
    }
    if ui.model.borrow().roles.is_some() {
        accounts_ui::open(ui);
    } else {
        sign_in(ui);
    }
}

// --- background work -------------------------------------------------------

/// Guard a background task so two clicks cannot start two of them.
fn begin(ui: &Ui, what: &str) -> bool {
    if ui.busy.borrow().is_some() || *ui.closing.borrow() {
        return false;
    }
    *ui.wait_on_close.borrow_mut() = false;
    ui.close_note.set(None);
    *ui.busy.borrow_mut() = Some(what.to_string());
    ui.spinner.start();
    show_activity(ui);
    true
}

/// Say what closing the window during the task just begun means for it.
fn note_close(ui: &Ui, note: &'static str) {
    ui.close_note.set(Some(note));
    show_activity(ui);
}

/// The note every refresh shares: closing drops the refresh, not the lists
/// already saved from the last one.
const REFRESH_CLOSE_NOTE: &str =
    "You can close the window while this refresh runs. Cached lists are kept.";

fn finish(ui: &Ui) {
    *ui.busy.borrow_mut() = None;
    *ui.wait_on_close.borrow_mut() = false;
    ui.close_note.set(None);
    *ui.download_control.borrow_mut() = None;
    *ui.package_cancel.borrow_mut() = None;
    ui.cancel_package.set_sensitive(true);
    ui.spinner.stop();
    show_activity(ui);
    // Let the completion callback reload records before closing the window.
    let ui = ui.clone();
    glib::idle_add_local_once(move || {
        if *ui.closing.borrow() && ui.busy.borrow().is_none() {
            ui.window.close();
        } else if ui.busy.borrow().is_none() && ui.install_owed.get().is_some() {
            // Only once the window is free, or it would be owed again.
            if let Some((package, then)) = ui.install_owed.take() {
                client_check::install(&ui, package, then);
            }
        } else if ui.install_queue.borrow().next().is_some() {
            queue_ui::start_next(&ui);
        } else if ui.account_owed.get() {
            refresh_account(&ui);
        } else if ui.entitlements_owed.get() {
            refresh_entitlements(&ui, true);
        }
    });
}

fn show_activity(ui: &Ui) {
    let busy = ui.busy.borrow();
    let closing = *ui.closing.borrow();
    ui.activity.set_visible(busy.is_some());
    ui.keep_open.set_visible(closing);
    ui.cancel_package
        .set_visible(busy.is_some() && ui.package_cancel.borrow().is_some());
    let Some(what) = busy.as_deref() else {
        ui.spinner.set_tooltip_text(Some("No activity"));
        ui.spinner
            .update_property(&[gtk::accessible::Property::Label("No activity")]);
        return;
    };
    ui.activity_title.set_label(what);
    let stopping = ui
        .download_control
        .borrow()
        .as_ref()
        .is_some_and(|c| c.requested());
    let moving = what.starts_with("Moving ");
    let detail = if moving && stopping {
        "Stopping the move and cleaning up. The original installation is kept."
    } else if closing && stopping {
        "Stopping the download and cleaning up. Ferestre will close when it is safe."
    } else if stopping {
        "Stopping the download and cleaning up. The window will stay open when cleanup finishes."
    } else if closing {
        "Finishing this operation. Ferestre will close when it is safe."
    } else if !*ui.wait_on_close.borrow() && ui.download_control.borrow().is_none() {
        ui.close_note
            .get()
            .unwrap_or("You can close the window while this runs.")
    } else {
        "Please wait for this to finish before starting another operation."
    };
    ui.activity_detail.set_label(detail);
    let tooltip = format!("{what}\n{detail}");
    ui.spinner.set_tooltip_text(Some(&tooltip));
    ui.spinner
        .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
}

fn report_busy(ui: &Ui) {
    let message = if *ui.closing.borrow() {
        "Ferestre is preparing to close. Choose Keep Window Open to continue.".to_string()
    } else if let Some(what) = ui.busy.borrow().as_deref() {
        format!("{what}. Please wait for it to finish, then try again.")
    } else {
        return;
    };
    ui.toasts.add_toast(toast(&message));
}

/// Closing while a title runs is allowed -- the title is its own process and
/// keeps running -- but it is said first, because the window then cannot stop
/// it, and a window opened later does not know it is running.
fn confirm_close_while_running(ui: &Ui) -> glib::Propagation {
    let running: Vec<String> = ui.running.borrow().keys().cloned().collect();
    if running.is_empty() || ui.close_while_running.get() {
        return glib::Propagation::Proceed;
    }
    if ui.close_dialog_open.replace(true) {
        return glib::Propagation::Stop;
    }
    let names: Vec<String> = {
        let model = ui.model.borrow();
        running
            .iter()
            .map(|key| {
                model
                    .catalog
                    .get(key)
                    .map(|p| p.name.clone())
                    .or_else(|| model.recipe_for(key).map(|r| r.title.name.clone()))
                    .unwrap_or_else(|| key.clone())
            })
            .collect()
    };
    let body = match names.as_slice() {
        [one] => format!(
            "{one} is still running. It keeps running after the window closes, but it can \
             then only be stopped from the game itself."
        ),
        many => format!(
            "{} are still running. They keep running after the window closes, but they can \
             then only be stopped from the games themselves.",
            many.join(", ")
        ),
    };
    let dialog = adw::AlertDialog::new(Some("Close Ferestre?"), Some(&body));
    dialog.add_response("cancel", "Keep Open");
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let window = ui.window.clone();
    let ui = ui.clone();
    dialog.connect_response(None, move |_, response| {
        *ui.close_dialog_open.borrow_mut() = false;
        if response == "close" {
            ui.close_while_running.set(true);
            ui.window.close();
        } else if *ui.closing.borrow() {
            // A close that waited for a task, and then for this answer.
            *ui.closing.borrow_mut() = false;
            show_activity(&ui);
        }
    });
    dialog.present(Some(&window));
    glib::Propagation::Stop
}

fn request_close(ui: &Ui) -> glib::Propagation {
    let Some(what) = ui.busy.borrow().clone() else {
        return confirm_close_while_running(ui);
    };
    if !*ui.wait_on_close.borrow() && ui.download_control.borrow().is_none() {
        return confirm_close_while_running(ui);
    }
    if *ui.closing.borrow() || ui.close_dialog_open.replace(true) {
        return glib::Propagation::Stop;
    }
    let downloading = ui.download_control.borrow().is_some();
    let moving = what.starts_with("Moving ");
    let body = if moving {
        format!("{what} is still in progress. Closing will stop the move and wait for cleanup. The original installation will be kept.")
    } else if downloading {
        format!("{what} is still in progress. Closing will stop the download and wait for cleanup. Downloaded game files will be kept so you can resume later.")
    } else {
        format!("{what} is still in progress. Ferestre will finish this operation before closing. Please keep the app open while it finishes.")
    };
    let dialog = adw::AlertDialog::new(Some("Close Ferestre?"), Some(&body));
    dialog.add_response("cancel", "Keep Open");
    dialog.add_response(
        "close",
        if moving {
            "Stop Move and Close"
        } else if downloading {
            "Stop Download and Close"
        } else {
            "Finish and Close"
        },
    );
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let window = ui.window.clone();
    let ui = ui.clone();
    dialog.connect_response(None, move |_, response| {
        *ui.close_dialog_open.borrow_mut() = false;
        if response != "close" {
            return;
        }
        *ui.closing.borrow_mut() = true;
        if let Some(control) = ui.download_control.borrow().as_ref() {
            control.stop();
        }
        show_activity(&ui);
        if ui.busy.borrow().is_none() {
            ui.window.close();
        }
    });
    dialog.present(Some(&window));
    glib::Propagation::Stop
}

fn sign_in(ui: &Ui) {
    let Some(client) = ui.model.borrow().xodus_cli() else {
        ui.toasts.add_toast(toast(
            "The Xodus client was not found. `ferestre doctor` says where it is looked for.",
        ));
        return;
    };
    if ui.model.borrow().roles_error.is_some() {
        ui.toasts.add_toast(toast(
            "The account roles file cannot be read. Reset it first: the button is at the top of the window.",
        ));
        return;
    }
    if !begin(ui, "Signing in") {
        report_busy(ui);
        return;
    }
    ui.toasts.add_toast(toast("Loading sign-in options"));

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || {
            // A signed-out default profile must be reauthenticated through Add
            // account, which verifies and reuses its existing save namespace.
            if fetch_profiles(&client).is_ok() {
                return Ok(None);
            }
            account::login_command(&client)
                .stdin(std::process::Stdio::null())
                .status()
                .map(Some)
                .map_err(|e| format!("{}: {e}", client.display()))
        })
        .await;
        finish(&ui);
        match result {
            Ok(Ok(None)) => accounts_ui::open(&ui),
            Ok(Ok(Some(status))) if status.success() => refresh_account(&ui),
            Ok(Ok(Some(status))) => ui
                .toasts
                .add_toast(toast(format!("Sign-in exited {status}"))),
            Ok(Err(e)) => ui.toasts.add_toast(toast(&e)),
            Err(_) => ui.toasts.add_toast(toast("The client crashed")),
        }
    });
}

fn refresh_account(ui: &Ui) {
    let (client, cache_dir, roles) = {
        let model = ui.model.borrow();
        (
            model.xodus_cli(),
            model.account_cache_dir(),
            model.roles.clone(),
        )
    };
    let (Some(client), Some(cache_dir)) = (client, cache_dir) else {
        return;
    };
    // Whose account it is depends on the role file; until it can be read, a
    // refresh would answer for whichever account is the default.
    if ui.model.borrow().roles_error.is_some() {
        return;
    }
    if !begin(ui, "Refreshing account") {
        // Owed rather than dropped: the next finish starts it.
        ui.account_owed.set(true);
        return;
    }
    ui.account_owed.set(false);
    note_close(ui, REFRESH_CLOSE_NOTE);

    /// What the client said. Only `SignedOut` is allowed to clear the saved
    /// account; a client that could not answer says nothing about it.
    enum Answer {
        SignedIn(account::Account, Option<PathBuf>),
        SignedOut,
        Unknown,
    }

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let expected = roles.clone();
        let result = gio::spawn_blocking(move || {
            let profiles = if roles.is_some() {
                fetch_profiles(&client).ok()
            } else {
                None
            };
            let answer = (|| {
                let mut command = scope_store(account::command(&client), roles.as_ref());
                let Ok(output) = deadline::output(&mut command, deadline::CLIENT_QUESTION) else {
                    return Answer::Unknown;
                };
                if !output.status.success() {
                    return if model::client_reports_signed_out(&String::from_utf8_lossy(
                        &output.stderr,
                    )) {
                        Answer::SignedOut
                    } else {
                        Answer::Unknown
                    };
                }
                let Ok(account) = account::parse(&String::from_utf8_lossy(&output.stdout)) else {
                    return Answer::Unknown;
                };
                // Best effort: an account with no picture is still an account.
                let avatar = account::avatar(&cache_dir, &account, AVATAR_PX).ok();
                Answer::SignedIn(account, avatar)
            })();
            (answer, profiles)
        })
        .await
        .unwrap_or((Answer::Unknown, None));
        finish(&ui);
        if ui.model.borrow().roles != expected {
            return;
        }
        let (answer, profiles) = result;
        {
            let mut model = ui.model.borrow_mut();
            if let Some(profiles) = profiles {
                model.profiles = Some(profiles);
            }
            // The selected Store account is listed as signed out, even if the
            // client could not be asked about it directly.
            let store_signed_out = model
                .roles
                .as_ref()
                .zip(model.profiles.as_ref())
                .is_some_and(|(roles, profiles)| {
                    !profiles
                        .profiles
                        .iter()
                        .any(|p| p.id == roles.store && p.signed_in)
                });
            match answer {
                Answer::SignedIn(account, avatar) => {
                    let changed = model
                        .account
                        .as_ref()
                        .is_none_or(|old| !account::same_identity(old, &account));
                    if changed {
                        model.ownership = Ownership::Unknown;
                        model.subscriptions.clear();
                        model.refused.clear();
                        model.entitlements_checked = false;
                        if let Some(state) = model.private_state_dir() {
                            let _ = std::fs::remove_file(library::cache_path(&state));
                        }
                    }
                    if let Some(state) = model.private_state_dir() {
                        let _ = account::save_cache(&state, &account);
                    }
                    model.account = Some(account);
                    model.avatar = avatar;
                }
                // Signed out somewhere else -- `xodus-cli logout` in a terminal,
                // or a window closed mid sign-out -- while this window still
                // remembered the account from account.json.
                Answer::SignedOut => model.forget_account(),
                Answer::Unknown if store_signed_out => model.forget_account(),
                Answer::Unknown => {}
            }
        }
        render(&ui);
        refresh_entitlements(&ui, false);
        refresh_updates(&ui, false);
        presence_ui::refresh(&ui, true);
    });
}

/// The client's accounts, as `accounts::fetch` reads them, with a deadline.
fn fetch_profiles(client: &Path) -> anyhow::Result<accounts::Profiles> {
    let mut command = std::process::Command::new(client);
    command.args(["account-profiles", "--json"]);
    let output = deadline::output(&mut command, deadline::CLIENT_QUESTION)?;
    anyhow::ensure!(
        output.status.success(),
        "Cannot read account profiles; update the client and check that your keychain is unlocked"
    );
    accounts::parse_profiles(&output.stdout)
}

fn scope_store(
    mut command: std::process::Command,
    roles: Option<&accounts::Selection>,
) -> std::process::Command {
    if let Some(roles) = roles {
        roles.apply(&mut command);
    }
    command
}

fn refresh_updates(ui: &Ui, force: bool) {
    let (client, state_dir, records) = {
        let model = ui.model.borrow();
        let state_dir = model.private_state_dir();
        let records = model
            .records
            .values()
            .filter(|record| !record.content_ids.is_empty())
            .filter(|record| {
                force
                    || state_dir
                        .as_ref()
                        .is_none_or(|dir| !ferestre_core::updates::is_fresh(dir, record))
            })
            .cloned()
            .collect::<Vec<_>>();
        (model.xodus_cli(), state_dir, records)
    };
    let (Some(client), Some(state_dir)) = (client, state_dir) else {
        return;
    };
    if records.is_empty() || ui.model.borrow().roles_error.is_some() {
        return;
    }
    // Background work with a flag of its own, not the window's busy slot:
    // one stalled request used to refuse every Install, Remove and Sign in
    // with "Checking for game updates" until the window was restarted.
    if ui.checking_updates.replace(true) {
        return;
    }
    let roles = ui.model.borrow().roles.clone();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let expected = roles.clone();
        let checked = gio::spawn_blocking(move || {
            let mut available = Vec::new();
            for record in records {
                let content_id = &record.content_ids[0];
                let mut command = std::process::Command::new(&client);
                command.args(["update", content_id, "--json"]);
                let version = deadline::output(
                    &mut scope_store(command, roles.as_ref()),
                    deadline::CLIENT_QUESTION,
                )
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
                .and_then(|value| {
                    value["version"]
                        .as_str()
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                });
                // Preserve last-known results on failure, but also throttle
                // retries across restarts until tomorrow or manual Refresh.
                let version = ferestre_core::updates::save(&state_dir, &record, version.clone())
                    .unwrap_or(version);
                available.push((record, version));
            }
            available
        })
        .await;
        ui.checking_updates.set(false);
        if ui.model.borrow().roles != expected {
            return;
        }
        if let Ok(available) = checked {
            let mut model = ui.model.borrow_mut();
            for (record, version) in available {
                let key = record.product_id.to_ascii_uppercase();
                if model.records.get(&key) != Some(&record) {
                    continue;
                }
                if let Some(version) = version {
                    model.available.insert(key, version);
                }
            }
            drop(model);
            render(&ui);
        }
    });
}

/// Read account access independently of downloads and catalogue artwork.
/// A failed request retains the last successful snapshot.
fn refresh_entitlements(ui: &Ui, force: bool) {
    // A Refresh that had to wait is still a Refresh.
    let force = force || ui.entitlements_owed.get();
    {
        let model = ui.model.borrow();
        // Nobody to ask about, or no way to tell whose library it would be.
        if (model.account.is_none() && model.roles.is_none()) || model.roles_error.is_some() {
            return;
        }
    }
    if !force {
        let model = ui.model.borrow();
        if model.entitlements_checked
            && model.private_state_dir().is_some_and(|p| {
                ferestre_core::cache::is_fresh(
                    &library::cache_path(&p),
                    ferestre_core::cache::LIBRARY_MAX_AGE,
                )
            })
        {
            return;
        }
        // A check that failed waits a while before trying again by itself.
        if ui
            .entitlements_retry_at
            .get()
            .is_some_and(|at| std::time::Instant::now() < at)
        {
            return;
        }
    }
    let blocked = ui.busy.borrow().is_some() && ui.download.current.borrow().is_none();
    if blocked || *ui.fetching_entitlements.borrow() {
        // A check already in flight answers a forced one too. A busy window
        // does not, so keep the request and say when it will run, once.
        if force && blocked && !ui.entitlements_owed.replace(true) {
            if let Some(what) = ui.busy.borrow().as_deref() {
                ui.toasts.add_toast(toast(format!(
                    "{what} is running. Your library will be checked when it finishes."
                )));
            }
        }
        return;
    }
    ui.entitlements_owed.set(false);
    let (client, state_dir, account) = {
        let model = ui.model.borrow();
        (
            model.xodus_cli(),
            model.private_state_dir(),
            model.account.clone(),
        )
    };
    let (Some(client), Some(state_dir)) = (client, state_dir) else {
        return;
    };
    ui.fetching_entitlements.replace(true);
    let roles = ui.model.borrow().roles.clone();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let expected = roles.clone();
        let result = gio::spawn_blocking(move || {
            let output = deadline::output(
                &mut scope_store(
                    ferestre_core::library::library_command(&client, &Default::default()),
                    roles.as_ref(),
                ),
                deadline::CLIENT_LISTING,
            )
            .ok()?;
            if !output.status.success() {
                return None;
            }
            ferestre_core::library::parse(&String::from_utf8_lossy(&output.stdout)).ok()
        })
        .await;
        ui.fetching_entitlements.replace(false);
        if !matches!(result, Ok(Some(_))) {
            let failures = ui.entitlement_failures.get().saturating_add(1);
            ui.entitlement_failures.set(failures);
            ui.entitlements_retry_at.set(Some(
                std::time::Instant::now() + model::retry_delay(failures),
            ));
        }
        if let Ok(Some(entries)) = result {
            ui.entitlement_failures.set(0);
            ui.entitlements_retry_at.set(None);
            // Ignore a response for an account that signed out or changed
            // while the query was in flight.
            if ui.model.borrow().roles != expected {
                refresh_entitlements(&ui, false);
                return;
            }
            if ui.model.borrow().account != account {
                return;
            }
            let (changed, subscriptions_changed) = {
                let model = ui.model.borrow();
                (
                    model.ownership != Ownership::Known(entries.clone()),
                    model.subscriptions != gamepass::active(&entries),
                )
            };
            let _ = ferestre_core::library::save_cache(&state_dir, &entries);
            ui.model.borrow_mut().set_ownership(entries);
            if changed {
                fetch_gamepass(&ui, subscriptions_changed);
                render(&ui);
            }
        }
    });
}

fn refresh_library(ui: &Ui) {
    let (client, cache) = {
        let model = ui.model.borrow();
        (model.xodus_cli(), model.catalog_cache())
    };
    let (Some(client), Some(cache)) = (client, cache) else {
        ui.toasts.add_toast(toast("The Xodus client was not found"));
        return;
    };
    if ui.model.borrow().roles_error.is_some() {
        ui.toasts.add_toast(toast(
            "The account roles file cannot be read. Reset it first: the button is at the top of the window.",
        ));
        return;
    }
    if !begin(ui, "Refreshing your library") {
        report_busy(ui);
        return;
    }
    note_close(ui, REFRESH_CLOSE_NOTE);
    let (market, language) = ui.model.borrow().market();
    let recipe_ids: Vec<String> = ui
        .model
        .borrow()
        .recipes
        .iter()
        .map(|r| r.title.product_id.clone())
        .collect();
    ui.toasts.add_toast(toast("Asking what this account owns"));

    let roles = ui.model.borrow().roles.clone();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let expected = roles.clone();
        let result = gio::spawn_blocking(move || {
            let output = deadline::output(
                &mut scope_store(
                    ferestre_core::library::library_command(&client, &Default::default()),
                    roles.as_ref(),
                ),
                deadline::CLIENT_LISTING,
            )
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

        if ui.model.borrow().roles != expected {
            return;
        }
        match result {
            Ok(Ok((entries, products, failures))) => {
                let owned = entries.iter().filter(|e| e.is_title()).count();
                {
                    // Written before it is drawn, so the next launch opens on a
                    // library rather than on a button.
                    if let Some(state_dir) = ui.model.borrow().private_state_dir() {
                        let _ = ferestre_core::library::save_cache(&state_dir, &entries);
                    }
                    let mut model = ui.model.borrow_mut();
                    model.set_ownership(entries);
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
                ui.toasts.add_toast(toast(&message));
            }
            Ok(Err(e)) => ui.toasts.add_toast(toast(format!("Library: {e}"))),
            Err(_) => ui.toasts.add_toast(toast("Library: the client crashed")),
        }
    });
}

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
    if have
        && !force
        && paths.as_ref().is_some_and(|dir| {
            ferestre_core::cache::is_fresh(
                &gamepass::cache_path(dir),
                ferestre_core::cache::GAMEPASS_MAX_AGE,
            )
        })
    {
        return;
    }
    // Every redraw of the section asks for this when there is no listing, so
    // a failure has to wait before it is tried again -- otherwise offline,
    // each keystroke in the search box was another pair of requests.
    if !force
        && ui
            .gamepass_retry_at
            .get()
            .is_some_and(|at| std::time::Instant::now() < at)
    {
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
                    let listing = gamepass::fetch(&market, &language)?;
                    let tiers = gamepass::fetch_tiers(&market, &language)?;
                    Ok::<_, anyhow::Error>((listing, tiers))
                }
            })
            .await;
            ui.fetching_gamepass.replace(false);
            let Ok(Ok((listing, tiers))) = fetched else {
                let failures = ui.gamepass_failures.get().saturating_add(1);
                ui.gamepass_failures.set(failures);
                ui.gamepass_retry_at.set(Some(
                    std::time::Instant::now() + model::retry_delay(failures),
                ));
                // Silent on purpose when there is already a listing: a failed
                // refresh of a public list is not worth a toast over the top of
                // whatever someone is doing. Said once otherwise, until the
                // next Refresh, not once per attempt.
                if !have && !ui.gamepass_reported.replace(true) {
                    ui.toasts
                        .add_toast(toast("Could not fetch the Game Pass catalogue"));
                }
                return;
            };
            ui.gamepass_failures.set(0);
            ui.gamepass_retry_at.set(None);
            // The merchandised list is what is kept: which ids the tier feed
            // added is worth knowing again on the next start.
            if let Some(dir) = paths {
                let _ = gamepass::save_cache(&dir, &market, &listing, &tiers);
            }
            {
                let mut model = ui.model.borrow_mut();
                model.gamepass = gamepass::complete_listing(&listing, &tiers);
                model.gamepass_merchandised =
                    listing.iter().map(|id| id.to_ascii_uppercase()).collect();
                model.tiers = tiers;
            }
            preload_gamepass_art(&ui);
            if ui.model.borrow().section == Section::GamePass {
                render(&ui);
            }
        });
    }
}

/// Display the cached metadata while one worker checks missing/stale entries.
/// Each id is attempted once per window or explicit refresh, including names
/// off-page, so failures cannot become a request loop on every redraw.
fn fetch_names(ui: &Ui, product_ids: Vec<String>) {
    if ui.fetching_names.get() {
        return;
    }
    let (cache, market, wanted) = {
        let model = ui.model.borrow();
        let wanted = model::unasked(product_ids, &mut ui.asked_names.borrow_mut());
        (model.catalog_cache(), model.market(), wanted)
    };
    let Some(cache) = cache else { return };
    if wanted.is_empty() {
        return;
    }

    ui.fetching_names.set(true);
    let generation = ui.catalog_generation.get();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let (market, language) = market;
        // In chunks, so filling in five hundred names shows rows as they land
        // rather than after every request has finished. Each chunk is several
        // batched lookups, so this is not a request per await.
        for chunk in wanted.chunks(catalog::BATCH * 5) {
            if ui.catalog_generation.get() != generation {
                break;
            }
            let ids = chunk.to_vec();
            let (cache, market, language) = (cache.clone(), market.clone(), language.clone());
            let found = gio::spawn_blocking(move || {
                let (products, _failures) =
                    catalog::revalidate(&cache, &ids, &market, &language, generation != 0);
                products
            })
            .await;
            let Ok(found) = found else { break };
            if ui.catalog_generation.get() != generation {
                break;
            }
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
        ui.fetching_names.set(false);
        // A section change or explicit refresh may have queued more ids.
        render(&ui);
    });
}

/// Visible pages get priority over the rest of the artwork prefetch queue.
fn fetch_icons(ui: &Ui, product_ids: Vec<String>) {
    artwork::request(ui, product_ids, true);
}

/// Once Game Pass has been opened, continue warming its full catalogue even
/// when another section is visible. Newly resolved metadata joins this queue.
fn preload_gamepass_art(ui: &Ui) {
    if !ui.gamepass_art_requested.get() || *ui.closing.borrow() {
        return;
    }
    let ids = ui.model.borrow().gamepass.clone();
    fetch_names(ui, ids.clone());
    artwork::request(ui, ids, false);
}

/// Validate the selected library in the CLI, off the UI thread, before offering
/// Move. The CLI repeats the check under the title lock when the move starts.
fn choose_move(ui: &Ui, product_id: &str, name: &str) {
    if ui.busy.borrow().is_some() {
        report_busy(ui);
        return;
    }
    let library = ui
        .model
        .borrow()
        .paths
        .as_ref()
        .map(|p| p.games_dir().to_path_buf());
    let Some(library) = library else {
        return;
    };
    let dialog = adw::AlertDialog::new(Some(&format!("Move {name}?")),
        Some("Choose a library folder. The game gets its own folder inside it. Files are verified before the original is removed. Saves stay in their current location."));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("move", "Move");
    dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("move", false);
    let entry = adw::EntryRow::builder()
        .title("Move to")
        .text(library.to_string_lossy())
        .build();
    let browse = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("Choose folder")
        .build();
    entry.add_suffix(&browse);
    let group = adw::PreferencesGroup::new();
    group.add(&entry);
    let detail = gtk::Label::builder()
        .label("Checking files and destination space…")
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption"])
        .build();
    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    extra.append(&group);
    extra.append(&detail);
    dialog.set_extra_child(Some(&extra));
    let chosen = Rc::new(RefCell::new(None::<PathBuf>));
    let generation = Rc::new(std::cell::Cell::new(0u64));
    let window = ui.window.clone();
    {
        let (ui, entry) = (ui.clone(), entry.clone());
        browse.connect_clicked(move |_| folder_picker::open(&ui, entry.upcast_ref()));
    }
    let product = product_id.to_string();
    entry.connect_changed(glib::clone!(
        #[strong]
        detail,
        #[strong]
        dialog,
        #[strong]
        chosen,
        #[strong]
        generation,
        move |entry| {
            generation.set(generation.get() + 1);
            let revision = generation.get();
            *chosen.borrow_mut() = None;
            dialog.set_response_enabled("move", false);
            detail.set_label("Checking files and destination space…");
            let text = entry.text().to_string();
            if text.is_empty() {
                detail.set_label("Choose a destination folder.");
                return;
            }
            let path = if text == "~" {
                std::env::var_os("HOME").map(PathBuf::from)
            } else if let Some(rest) = text.strip_prefix("~/") {
                std::env::var_os("HOME").map(|p| PathBuf::from(p).join(rest))
            } else {
                Some(PathBuf::from(text))
            };
            let Some(path) = path else {
                return;
            };
            let product = product.clone();
            let detail = detail.clone();
            let dialog = dialog.clone();
            let chosen = chosen.clone();
            let generation = generation.clone();
            glib::spawn_future_local(async move {
                // Wait for typing to settle before walking a large install.
                glib::timeout_future(std::time::Duration::from_millis(250)).await;
                if revision != generation.get() {
                    return;
                }
                let result = gio::spawn_blocking(move || {
                    let out = std::process::Command::new(cli_binary())
                        .args(["--json", "move", &product])
                        .arg(&path)
                        .output()
                        .map_err(|e| e.to_string())?;
                    if !out.status.success() {
                        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
                    }
                    let plan: serde_json::Value =
                        serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
                    Ok((path, plan))
                })
                .await;
                if revision != generation.get() {
                    return;
                }
                match result {
                    Ok(Ok((path, plan))) => {
                        detail.set_label(&format!(
                            "{} → {}. {} free.{}",
                            ferestre_core::human_bytes(plan["bytes"].as_u64().unwrap_or(0)),
                            plan["destination"].as_str().unwrap_or(""),
                            ferestre_core::human_bytes(plan["available"].as_u64().unwrap_or(0)),
                            if plan["same_filesystem"] == true {
                                " Same drive: moves immediately."
                            } else {
                                " Includes copying and verification."
                            }
                        ));
                        *chosen.borrow_mut() = Some(path);
                        dialog.set_response_enabled("move", true);
                    }
                    Ok(Err(error)) => detail.set_label(&error),
                    Err(_) => detail.set_label("Could not check destination"),
                }
            });
        }
    ));
    let ui = ui.clone();
    let product = product_id.to_string();
    let name = name.to_string();
    dialog.connect_response(None, move |_, response| {
        generation.set(generation.get() + 1);
        if response == "move" {
            if let Some(path) = chosen.borrow().as_ref() {
                spawn_transfer(&ui, &product, &name, Some(path), "Moving", "move");
            }
        }
    });
    entry.emit_by_name::<()>("changed", &[]);
    dialog.present(Some(&window));
}

#[cfg(test)]
mod catalog_regressions {
    use super::*;

    fn fixture() -> Model {
        Model {
            roles: None,
            roles_error: None,
            profiles: None,
            refused: Default::default(),
            paths: None,
            recipes: Vec::new(),
            runtime: None,
            runtimes: Vec::new(),
            registry: None,
            records: BTreeMap::new(),
            disk_usage: BTreeMap::new(),
            sizes_pending: Default::default(),
            size_generation: 0,
            installed_products: Default::default(),
            ownership: Ownership::Unknown,
            catalog: BTreeMap::new(),
            icons: BTreeMap::new(),
            available: BTreeMap::new(),
            account: None,
            avatar: None,
            gamepass: vec!["9NR1R1XWLCNB".into()],
            gamepass_merchandised: Default::default(),
            subscriptions: Vec::new(),
            entitlements_checked: false,
            tiers: gamepass::parse_tiers(r#"{"9NR1R1XWLCNB":{"PCSubMetadata":{"Included":true}}}"#)
                .unwrap(),
            section: Section::GamePass,
            query: String::new(),
            page: 0,
            show_unsupported: false,
            problem: None,
        }
    }

    /// `xodus-cli logout` in a terminal leaves account.json behind. When the
    /// client then says nobody is signed in, the window drops the account and
    /// the library it was listing, not just the name on the button.
    #[test]
    fn a_signed_out_client_forgets_the_account_and_its_library() {
        let mut model = fixture();
        model.account = Some(account::parse(r#"{"gamertag":"TestPlayer","xuid":"25"}"#).unwrap());
        model.avatar = Some(PathBuf::from("/cache/gamerpic-64.img"));
        model.set_ownership(snapshot(true));
        assert!(!model.subscriptions.is_empty());

        model.forget_account();
        assert!(model.account.is_none());
        assert!(model.avatar.is_none());
        assert_eq!(model.ownership, Ownership::Unknown);
        assert!(model.subscriptions.is_empty());
        assert!(!model.entitlements_checked);
    }

    /// Nothing has been asked of the catalog yet.
    fn no_lookups() -> NameLookups<'static> {
        static NONE: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        NameLookups {
            asked: &NONE,
            fetching: false,
        }
    }

    fn snapshot(ultimate: bool) -> Vec<ferestre_core::library::Entry> {
        ferestre_core::library::parse(if ultimate {
            r#"[{"productId":"CFQ7TTC0P85B","status":"Revoked","productType":"Pass"},
                 {"productId":"CFQ7TTC0KHS0","status":"Active","productType":"Pass"}]"#
        } else {
            r#"[{"productId":"CFQ7TTC0P85B","status":"Active","productType":"Pass"},
                 {"productId":"CFQ7TTC0KHS0","status":"Revoked","productType":"Pass"}]"#
        })
        .unwrap()
    }

    #[test]
    fn upgrade_updates_the_filtered_catalogue_without_a_restart() {
        let mut model = fixture();
        let running = BTreeMap::new();
        model.subscriptions = vec!["Game Pass Premium"];
        let draw = |model: &Model| {
            draw_list(
                model,
                &running,
                None,
                no_lookups(),
                Section::GamePass,
                |row| !row.known_uninstallable(),
            )
        };
        assert_eq!(
            draw(&model).rows.len(),
            1,
            "unverified cached tiers must not hide games"
        );
        model.set_ownership(snapshot(false));
        assert!(draw(&model).rows.is_empty());
        model.set_ownership(snapshot(true));
        let updated = draw(&model);
        assert_eq!(model.subscriptions, vec!["Game Pass Ultimate"]);
        assert_eq!(updated.rows.len(), 1);
        assert!(!updated.rows[0].outside_tier);
    }

    #[test]
    fn installing_title_survives_a_stale_tier_snapshot() {
        let mut model = fixture();
        model.set_ownership(snapshot(false));
        let drawn = draw_list(
            &model,
            &BTreeMap::new(),
            Some("9NR1R1XWLCNB"),
            no_lookups(),
            Section::GamePass,
            |row| !row.known_uninstallable(),
        );
        assert_eq!(drawn.rows.len(), 1);
        assert_eq!(drawn.rows[0].action, Action::Installing);
        assert!(!drawn.rows[0].subtitle.contains("refused"));
    }

    #[test]
    fn name_lookup_includes_hidden_and_off_page_products() {
        let mut model = fixture();
        model.set_ownership(snapshot(false));
        model
            .gamepass
            .extend((0..50).map(|i| format!("9ZZTEST{i:05}")));
        model.page = 99;
        let drawn = draw_list(
            &model,
            &BTreeMap::new(),
            None,
            no_lookups(),
            Section::GamePass,
            |row| !row.known_uninstallable(),
        );
        assert_eq!(drawn.pending_names, 51);
        assert!(drawn.want_names.contains(&"9NR1R1XWLCNB".into()));
        assert!(drawn.page < 99);
        assert!(!drawn.rows.is_empty());
    }

    /// A name the catalog has been asked for and did not describe is not
    /// "still being looked up": counting it kept "Looking up 3 title names…"
    /// on screen for good, and a search with no matches blamed it.
    #[test]
    fn names_asked_for_and_not_found_are_no_longer_pending() {
        let mut model = fixture();
        model.set_ownership(snapshot(false));
        model
            .gamepass
            .extend((0..3).map(|i| format!("9ZZTEST{i:05}")));
        let asked: std::collections::BTreeSet<String> = model
            .gamepass
            .iter()
            .map(|id| id.to_ascii_uppercase())
            .collect();
        let draw = |fetching| {
            draw_list(
                &model,
                &BTreeMap::new(),
                None,
                NameLookups {
                    asked: &asked,
                    fetching,
                },
                Section::GamePass,
                |row| !row.known_uninstallable(),
            )
        };
        assert_eq!(
            draw(true).pending_names,
            4,
            "still pending while a lookup runs"
        );
        assert_eq!(draw(false).pending_names, 0);
    }

    /// Rows with no recipe and Game Pass rows showed Play while the title they
    /// started was running, and a second Play started it again in the same
    /// prefix.
    #[test]
    fn a_running_title_offers_stop_whatever_kind_of_row_it_is() {
        let mut model = fixture();
        model.set_ownership(snapshot(true));
        model
            .installed_products
            .insert("9NR1R1XWLCNB".into(), PathBuf::from("/games/9nr1r1xwlcnb"));
        let mut running: BTreeMap<String, stop::Launch> = [(
            "9NR1R1XWLCNB".to_string(),
            stop::Launch {
                ready: true,
                ..stop::Launch::new(42, std::time::Instant::now())
            },
        )]
        .into();
        for section in [Section::GamePass, Section::Library] {
            if section == Section::Library {
                // Owned outright, with no recipe: an undescribed row.
                model.ownership = Ownership::Known(
                    ferestre_core::library::parse(
                        r#"[{"productId":"9NR1R1XWLCNB","productType":"Game","status":"Active"}]"#,
                    )
                    .unwrap(),
                );
            }
            let drawn = draw_list(&model, &running, None, no_lookups(), section, |_| true);
            let row = drawn
                .rows
                .iter()
                .find(|row| row.product_id.eq_ignore_ascii_case("9NR1R1XWLCNB"))
                .unwrap_or_else(|| panic!("no row in {section:?}"));
            assert_eq!(row.action, Action::Stop, "{section:?}");
            assert!(row.subtitle.contains("running"), "{}", row.subtitle);
            running.get_mut("9NR1R1XWLCNB").unwrap().ready = false;
            let starting = draw_list(&model, &running, None, no_lookups(), section, |_| true);
            let row = starting
                .rows
                .iter()
                .find(|row| row.product_id.eq_ignore_ascii_case("9NR1R1XWLCNB"))
                .unwrap();
            assert_eq!(
                row.action,
                Action::Stop,
                "starting games can still be cancelled"
            );
            assert!(
                row.subtitle.contains("starting") && !row.subtitle.contains("running"),
                "{}",
                row.subtitle
            );
            running.get_mut("9NR1R1XWLCNB").unwrap().ready = true;
        }
    }

    /// Every toast is plain text. Since libadwaita 1.4 a toast's title is
    /// markup unless told otherwise, and a title like "Command & Conquer" or an
    /// error with "<DIR>" in it drew an empty toast.
    #[test]
    fn every_toast_is_plain_text() {
        let sources = [
            ("main.rs", include_str!("main.rs")),
            ("accounts_ui.rs", include_str!("accounts_ui.rs")),
            ("client_check.rs", include_str!("client_check.rs")),
            ("editor.rs", include_str!("editor.rs")),
        ];
        let forbidden = ["adw::Toast", "::new("].concat();
        let builder = ["Toast", "::builder()"].concat();
        for (file, text) in sources {
            assert!(
                !text.contains(&forbidden),
                "{file} makes a toast that parses markup; use toast()"
            );
            for (at, _) in text.match_indices(&builder) {
                let statement = &text[at..at + text[at..].find(".build()").expect("built")];
                assert!(
                    statement.contains(".use_markup(false)"),
                    "{file}: a built toast must turn markup off: {statement}"
                );
            }
        }
    }
}

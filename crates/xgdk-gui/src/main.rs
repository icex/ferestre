//! A window over the launcher.
//!
//! Deliberately thin, and deliberately dumb: every decision -- which recipe
//! applies, whether the runtime can satisfy it, what a refusal should say --
//! comes from [`model`], which is tested without a display, and from
//! `xgdk-core`, which the CLI uses too. Actions are performed by spawning the
//! `xgdk` binary rather than reimplementing them here, so the window cannot
//! drift into doing something the terminal would not.
//!
//! The client is never linked: Xodus is GPL-3.0-only, and the decrypted
//! executable has to stay a child of the process that opened it.

mod model;

use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use model::{Action, Inputs, Ownership, TitleView};
use xgdk_core::paths::Paths;
use xgdk_core::recipe::Recipe;
use xgdk_core::runtime::{InstalledRuntime, Registry};

const APP_ID: &str = "io.github.icex.xgdk";
/// Where someone reports a title that worked, or did not.
const ISSUES_URL: &str = "https://github.com/icex/xgdk-launcher/issues";

/// Everything the window draws from, loaded once and refreshed on demand.
struct Model {
    paths: Option<Paths>,
    recipes: Vec<Recipe>,
    runtime: Option<InstalledRuntime>,
    registry: Option<Registry>,
    /// What the account owns. Unknown until someone asks, and that is the
    /// normal state: the window has to be useful without a network round trip.
    ownership: Ownership,
    problem: Option<String>,
}

impl Model {
    fn load() -> Self {
        let mut problem = None;
        let paths = match Paths::from_env() {
            Ok(p) => Some(p),
            Err(e) => {
                problem = Some(format!("Cannot work out where things live: {e}"));
                None
            }
        };
        let titles_dir = paths.as_ref().and_then(|p| p.titles_dir()).map(Path::to_owned);
        let recipes = titles_dir
            .as_deref()
            .map(|dir| Recipe::load_dir(dir).unwrap_or_default())
            .unwrap_or_default();
        let registry = titles_dir
            .as_deref()
            .and_then(|dir| Registry::load(&dir.join("capabilities.toml")).ok());
        let runtime = paths.as_ref().and_then(|p| InstalledRuntime::discover(p).ok());

        if problem.is_none() && recipes.is_empty() {
            problem = Some(
                "No title recipes found. Point XGDK_TITLES at the titles/ directory.".into(),
            );
        }
        Model {
            paths,
            recipes,
            runtime,
            registry,
            ownership: Ownership::Unknown,
            problem,
        }
    }

    /// Whether a title's install directory is on disk. Absent paths mean the
    /// question cannot be answered, and "not installed" is the safe answer:
    /// the worst it costs is an Install press that finds the files already there.
    fn is_installed(&self, recipe: &Recipe) -> bool {
        self.paths
            .as_ref()
            .and_then(|p| p.install_dir(recipe).ok())
            .is_some_and(|dir| dir.is_dir())
    }

    fn views(&self) -> Vec<TitleView> {
        let installed = |r: &Recipe| self.is_installed(r);
        model::rows(&Inputs {
            recipes: &self.recipes,
            runtime: self.runtime.as_ref(),
            registry: self.registry.as_ref(),
            ownership: &self.ownership,
            installed: &installed,
        })
    }

    fn cli_binary(&self) -> PathBuf {
        let exe = std::env::current_exe().ok();
        model::cli_binary(exe.as_deref().and_then(Path::parent), &|p| p.is_file())
    }
}

/// Run `xgdk` and leave it running. Output goes where the CLI already sends it
/// -- a per-title log file -- rather than being swallowed into a widget that
/// nobody would read while a game is starting.
fn spawn_cli(model: &Model, args: &[&str], toasts: &adw::ToastOverlay) {
    let program = model.cli_binary();
    match std::process::Command::new(&program).args(args).spawn() {
        Ok(_) => toasts.add_toast(adw::Toast::new(&format!("Started: xgdk {}", args.join(" ")))),
        Err(e) => toasts.add_toast(adw::Toast::new(&format!("{}: {e}", program.display()))),
    }
}

fn title_row(view: &TitleView, model: &Rc<RefCell<Model>>, toasts: &adw::ToastOverlay) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&view.name)
        .subtitle(&view.subtitle)
        .subtitle_lines(2)
        .tooltip_text(&view.product_id)
        .build();

    let badge = gtk::Label::new(Some(view.badge));
    badge.add_css_class(view.badge_css);
    badge.add_css_class("caption");
    badge.set_width_chars(12);
    badge.set_xalign(0.0);
    row.add_prefix(&badge);

    let button = gtk::Button::builder()
        .label(view.action.label())
        .valign(gtk::Align::Center)
        .css_classes(["suggested-action"])
        .build();
    button.set_sensitive(view.action.is_enabled());

    match &view.action {
        Action::Blocked(reason) => button.set_tooltip_text(Some(reason)),
        action => {
            let args: Vec<String> = action
                .command(&view.product_id)
                .expect("a non-blocked action runs something")
                .iter()
                .map(|s| s.to_string())
                .collect();
            button.set_tooltip_text(Some(&format!("xgdk {}", args.join(" "))));
            let model = model.clone();
            let toasts = toasts.clone();
            button.connect_clicked(move |_| {
                let borrowed = model.borrow();
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                spawn_cli(&borrowed, &args, &toasts);
            });
        }
    }

    row.add_suffix(&button);
    row.set_activatable_widget(Some(&button));
    row
}

/// Ask the client what the account owns. Blocking, so it runs off the main
/// thread: it signs in, pages through collections and can take seconds.
fn fetch_library(paths: Option<&Paths>) -> anyhow::Result<Vec<xgdk_core::library::Entry>> {
    let client = paths
        .and_then(Paths::xodus_cli)
        .ok_or_else(|| anyhow::anyhow!("the Xodus client was not found"))?;
    let output = xgdk_core::library::library_command(&client, &Default::default())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("{}: {e}", client.display()))?;
    if !output.status.success() {
        anyhow::bail!("{} exited {}", client.display(), output.status);
    }
    xgdk_core::library::parse(&String::from_utf8_lossy(&output.stdout))
}

fn build_ui(app: &adw::Application) {
    let model = Rc::new(RefCell::new(Model::load()));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("xgdk")
        .default_width(760)
        .default_height(620)
        .build();

    let header = adw::HeaderBar::new();
    let sign_in = gtk::Button::builder()
        .label("Check library")
        .tooltip_text("Sign in through the Xodus client and list what this account owns")
        .build();
    // Both on the trailing side: KDE puts the window controls on the left, and
    // a button wedged against the close button is a misclick waiting to happen.
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Reload recipes and re-check the runtime"));
    let spinner = gtk::Spinner::new();
    header.pack_end(&refresh);
    header.pack_end(&sign_in);
    header.pack_end(&spinner);

    let page = adw::PreferencesPage::new();
    let scroll = gtk::ScrolledWindow::builder().vexpand(true).child(&page).build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&scroll));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&toasts));
    window.set_content(Some(&toolbar));

    // `AdwPreferencesPage` has no way to enumerate the groups it was given --
    // walking its children hands back its internal scrolled window instead --
    // so the rebuild removes exactly what it added last time.
    let groups: Rc<RefCell<Vec<adw::PreferencesGroup>>> = Rc::new(RefCell::new(Vec::new()));

    let rebuild = Rc::new({
        let model = model.clone();
        let page = page.clone();
        let groups = groups.clone();
        let toasts = toasts.clone();
        move || {
            for group in groups.borrow_mut().drain(..) {
                page.remove(&group);
            }
            let mut added = Vec::new();

            {
                let m = model.borrow();

                if let Some(problem) = &m.problem {
                    let group = adw::PreferencesGroup::builder().title("Not ready").build();
                    let row = adw::ActionRow::builder().title(problem).build();
                    row.add_css_class("error");
                    group.add(&row);
                    added.push(group);
                }

                let (title, subtitle) = model::runtime_summary(m.runtime.as_ref());
                let runtime_group = adw::PreferencesGroup::builder().title("Runtime").build();
                let runtime_row = adw::ActionRow::builder()
                    .title(&title)
                    .subtitle(&subtitle)
                    .subtitle_lines(2)
                    .build();
                if m.runtime.is_none() {
                    let install = gtk::Button::builder()
                        .label("Install")
                        .valign(gtk::Align::Center)
                        .tooltip_text("xgdk install-runtime — builds a patched Proton, which takes a while")
                        .build();
                    let model = model.clone();
                    let toasts = toasts.clone();
                    install.connect_clicked(move |_| {
                        spawn_cli(&model.borrow(), &["install-runtime"], &toasts);
                    });
                    runtime_row.add_suffix(&install);
                }
                runtime_group.add(&runtime_row);
                added.push(runtime_group);

                let views = m.views();
                let titles = adw::PreferencesGroup::builder()
                    .title("Titles")
                    .description(if m.ownership.is_known() {
                        "Recipes in this build, checked against what this account owns"
                    } else {
                        "Recipes in this build. Check the library to see which of these you own"
                    })
                    .build();
                for view in &views {
                    titles.add(&title_row(view, &model, &toasts));
                }
                added.push(titles);

                let unrecognised = m.ownership.without_recipe(&m.recipes);
                if !unrecognised.is_empty() {
                    let group = adw::PreferencesGroup::builder()
                        .title("Owned, with no recipe yet")
                        .description("Nobody has written a launch recipe for these. Trying one and saying what happened is the most useful thing you can do here.")
                        .build();
                    for product in &unrecognised {
                        let row = adw::ActionRow::builder().title(product).build();
                        let report = gtk::LinkButton::builder()
                            .label("Report")
                            .uri(format!("{ISSUES_URL}/new?title={product}"))
                            .valign(gtk::Align::Center)
                            .build();
                        row.add_suffix(&report);
                        group.add(&row);
                    }
                    added.push(group);
                }
            }

            for group in &added {
                page.add(group);
            }
            *groups.borrow_mut() = added;
        }
    });
    rebuild();

    {
        let model = model.clone();
        let rebuild = rebuild.clone();
        refresh.connect_clicked(move |_| {
            // Reloading throws away a library that was already fetched, so it
            // is carried across: re-reading recipes is not a reason to make
            // someone sign in again.
            let ownership = std::mem::take(&mut model.borrow_mut().ownership);
            let mut fresh = Model::load();
            fresh.ownership = ownership;
            *model.borrow_mut() = fresh;
            rebuild();
        });
    }

    {
        let model = model.clone();
        let rebuild = rebuild.clone();
        let toasts = toasts.clone();
        let spinner = spinner.clone();
        let sign_in_button = sign_in.clone();
        sign_in.connect_clicked(move |_| {
            let paths = model.borrow().paths.as_ref().map(Paths::xodus_cli);
            if paths.flatten().is_none() {
                toasts.add_toast(adw::Toast::new(
                    "The Xodus client was not found. `xgdk doctor` says where it is looked for.",
                ));
                return;
            }
            sign_in_button.set_sensitive(false);
            spinner.start();
            let model = model.clone();
            let rebuild = rebuild.clone();
            let toasts = toasts.clone();
            let spinner = spinner.clone();
            let sign_in_button = sign_in_button.clone();
            glib::spawn_future_local(async move {
                let paths = model.borrow().paths.clone();
                let result = gio::spawn_blocking(move || fetch_library(paths.as_ref())).await;
                spinner.stop();
                sign_in_button.set_sensitive(true);
                match result {
                    Ok(Ok(entries)) => {
                        let owned = entries.iter().filter(|e| e.is_title()).count();
                        model.borrow_mut().ownership = Ownership::Known(entries);
                        rebuild();
                        toasts.add_toast(adw::Toast::new(&format!(
                            "This account owns {owned} games and apps"
                        )));
                    }
                    Ok(Err(e)) => toasts.add_toast(adw::Toast::new(&format!("Library: {e}"))),
                    Err(_) => toasts.add_toast(adw::Toast::new("Library: the client crashed")),
                }
            });
        });
    }

    window.present();
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

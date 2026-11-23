//! The recipe editor.
//!
//! The catalog will list a hundred titles with a handful of recipes between
//! them, so the ordinary case is a title nobody has described. Someone should
//! be able to fill in an executable path and a couple of environment variables,
//! press Play, and hand the result back -- not learn a TOML schema first.
//!
//! Edits are written to the user's own titles directory, never over the
//! packaged one. That is what makes an edit survive an upgrade, and it is what
//! makes "what did I change" answerable: the file is the answer, and it is the
//! thing to attach to an issue.

use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ferestre_core::launch_options::{self, Kind, LaunchOption, Section};
use ferestre_core::recipe::Recipe;
use ferestre_core::steam_import;
use std::collections::BTreeMap;

/// The fields worth putting in front of someone, in the order they matter.
///
/// Not every field a recipe has: `[status]`, `[issues]` and the rest are the
/// project's record of what it verified, and inviting an edit to them would
/// turn a launcher into a place people write untrue compatibility claims. The
/// executable and the environment are what actually make a title start.
pub struct Draft {
    pub name: String,
    pub executable: String,
    pub install_dir: String,
    pub prefix_dir: String,
    /// The settings shown as switches and menus, by variable.
    pub chosen: BTreeMap<&'static str, String>,
    /// Every other variable, in the order the editor lists them.
    pub other: Vec<(String, String)>,
    /// A local game's arguments, as words a shell would split.
    pub arguments: String,
    /// The programs a local game is run through (`gamemoderun mangohud`).
    pub wrapper: String,
}

impl Draft {
    pub fn from_recipe(recipe: &Recipe) -> Self {
        let split = launch_options::split(&recipe.launch.env);
        Draft {
            name: recipe.title.name.clone(),
            executable: recipe.launch.executable.clone(),
            install_dir: recipe.install_dir().to_string(),
            prefix_dir: recipe.prefix_dir(),
            chosen: split.chosen,
            other: split.other,
            arguments: steam_import::quote_words(&recipe.launch.args),
            wrapper: steam_import::quote_words(&recipe.launch.wrapper),
        }
    }

    /// Apply the draft to a recipe, or say what is wrong with it.
    ///
    /// The only hard requirement is the executable, because without one there
    /// is nothing to launch and a recipe that cannot launch is worse than no
    /// recipe: it looks like the launcher failing.
    pub fn apply(&self, recipe: &mut Recipe) -> Result<(), String> {
        let executable = self.executable.trim();
        if executable.is_empty() {
            return Err("The executable is the one thing a recipe cannot do without.".into());
        }
        let local = recipe.is_local();
        if local {
            // A local game's paths are where its files are: absolute is fine,
            // climbing out with `..` is not.
            let climbs = |path: &str| {
                std::path::Path::new(&path.replace('\\', "/"))
                    .components()
                    .any(|c| c == std::path::Component::ParentDir)
            };
            if climbs(executable) {
                return Err("The executable must not contain \"..\".".into());
            }
            let dir = self.install_dir.trim();
            if !dir.starts_with('/') || climbs(dir) {
                return Err(
                    "The game's folder is its full path, like \"/run/media/games/Game\".".into(),
                );
            }
            if climbs(self.prefix_dir.trim()) {
                return Err("The Wine prefix must not contain \"..\".".into());
            }
        } else if executable.starts_with('/') || executable.contains("..") {
            return Err(
                "The executable is a path inside the installed title, like \"Game/Game.exe\"."
                    .into(),
            );
        }

        let environment = launch_options::join(&self.chosen, &self.other)?;
        let launch_words = if local {
            Some(self.launch_words()?)
        } else {
            None
        };

        let name = self.name.trim();
        if !name.is_empty() {
            recipe.title.name = name.to_string();
        }
        recipe.launch.executable = executable.to_string();
        recipe.launch.env = environment;
        if let Some(words) = launch_words {
            recipe.launch.args = words.args;
            recipe.launch.wrapper = words.wrapper;
            // A local game's folder is its own, not a default to follow.
            recipe.install.dir = non_empty(&self.install_dir);
            recipe.install.prefix = non_empty(&self.prefix_dir)
                .filter(|d| *d != format!("{}-proton", recipe.title.slug));
            return Ok(());
        }
        // Empty means "the default", which is the slug -- storing the computed
        // default would freeze it, so a later rename would not follow.
        recipe.install.dir = non_empty(&self.install_dir).filter(|d| *d != recipe.title.slug);
        recipe.install.prefix =
            non_empty(&self.prefix_dir).filter(|d| *d != format!("{}-proton", recipe.title.slug));
        Ok(())
    }
}

impl Draft {
    /// The arguments and wrappers, split by the rules Steam's launch options
    /// are split by, so there is one set of quoting rules.
    fn launch_words(&self) -> Result<steam_import::LaunchOptions, String> {
        let wrapper = self.wrapper.trim();
        let first = wrapper.split_whitespace().next().unwrap_or("");
        if is_assignment(first) {
            return Err("Put environment variables in Environment, not Wrapper commands.".into());
        }
        if wrapper.contains("%command%") {
            return Err("Wrapper commands run the game themselves; leave out %command%.".into());
        }
        let text = format!("{wrapper} %command% {}", self.arguments.trim());
        let words = steam_import::parse_launch_options(&text)
            .map_err(|e| format!("Arguments or wrapper commands: {e}"))?;
        if !words.env.is_empty() {
            return Err("Put environment variables in Environment, not Wrapper commands.".into());
        }
        Ok(words)
    }
}

/// Whether `word` is `NAME=value` with `NAME` matching `[A-Za-z_][A-Za-z0-9_]*`.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A title's folder as a recipe's `install.dir`: relative to the games
/// directory, which is the only place a recipe may name. `None` for a folder
/// outside it, or the games directory itself.
pub fn relative_install_dir(games_dir: &std::path::Path, dir: &std::path::Path) -> Option<String> {
    let relative = dir.strip_prefix(games_dir).ok()?;
    let normal = relative
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    let text = relative.to_str()?;
    (normal && !text.is_empty()).then(|| text.to_string())
}

fn same_recipe(a: &Recipe, b: &Recipe) -> bool {
    match (a.to_toml(), b.to_toml()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// One known setting's switch or menu, and how to read it back.
enum OptionControl {
    Switch(&'static LaunchOption, adw::SwitchRow),
    Choice(&'static LaunchOption, adw::ComboRow),
}

impl OptionControl {
    /// The variable and value to write, or `None` for leaving it out.
    fn value(&self) -> Option<(&'static str, String)> {
        let (option, value) = match (self, self.option().kind) {
            (OptionControl::Switch(option, row), Kind::Switch { on, off }) => {
                (option, if row.is_active() { on } else { off })
            }
            (OptionControl::Choice(option, row), Kind::Choice(choices)) => (
                option,
                choices
                    .get(row.selected() as usize)
                    .and_then(|(value, _)| *value),
            ),
            _ => return None,
        };
        value.map(|value| (option.key, value.to_string()))
    }

    /// Say again what the option does, for another runtime.
    fn describe_for(&self, runtime: Option<&Path>) {
        let subtitle = option_subtitle(self.option(), runtime);
        match self {
            OptionControl::Switch(_, row) => row.set_subtitle(&subtitle),
            OptionControl::Choice(_, row) => row.set_subtitle(&subtitle),
        }
    }

    fn option(&self) -> &'static LaunchOption {
        match self {
            OptionControl::Switch(option, _) | OptionControl::Choice(option, _) => option,
        }
    }
}

/// What an option says under its title; `runtime` is the one the title would
/// launch with, to say when it does not know a flag.
fn option_subtitle(option: &LaunchOption, runtime: Option<&Path>) -> String {
    if option.is_proton_flag() {
        if let Some(dir) = runtime {
            if launch_options::reads(dir, option.key) == Some(false) {
                let name = dir.file_name().unwrap_or_default().to_string_lossy();
                return format!("{} Not known to this game's runtime, {name}.", option.help);
            }
        }
        return option.help.to_string();
    }
    match option.capability {
        Some(capability) => format!("{} Needs a runtime with {capability}.", option.help),
        None => option.help.to_string(),
    }
}

/// A group for each of `sections`, each setting showing what the recipe chose.
fn option_groups(
    page: &adw::PreferencesPage,
    sections: &[Section],
    chosen: &BTreeMap<&'static str, String>,
    runtime: Option<&Path>,
) -> Vec<OptionControl> {
    let mut controls = Vec::new();
    for &section in sections {
        let group = adw::PreferencesGroup::builder()
            .title(section.title())
            .build();
        for option in launch_options::OPTIONS
            .iter()
            .filter(|o| o.section == section)
        {
            let value = chosen.get(option.key).map(String::as_str);
            let subtitle = option_subtitle(option, runtime);
            match option.kind {
                Kind::Switch { on, .. } => {
                    let row = adw::SwitchRow::builder()
                        .title(option.title)
                        .subtitle(&subtitle)
                        .active(value == on)
                        .build();
                    row.set_tooltip_text(Some(option.key));
                    group.add(&row);
                    controls.push(OptionControl::Switch(option, row));
                }
                Kind::Choice(choices) => {
                    let labels: Vec<&str> = choices.iter().map(|(_, label)| *label).collect();
                    let row = adw::ComboRow::builder()
                        .title(option.title)
                        .subtitle(&subtitle)
                        .model(&gtk::StringList::new(&labels))
                        .selected(choices.iter().position(|(v, _)| *v == value).unwrap_or(0) as u32)
                        .build();
                    row.set_tooltip_text(Some(option.key));
                    group.add(&row);
                    controls.push(OptionControl::Choice(option, row));
                }
            }
        }
        page.add(&group);
    }
    controls
}

/// The variables with no switch of their own: added, changed and removed
/// one row at a time.
struct OtherVariables {
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<(gtk::ListBoxRow, gtk::Entry, gtk::Entry)>>,
}

impl OtherVariables {
    fn new(page: &adw::PreferencesPage, values: &[(String, String)]) -> Rc<Self> {
        let group = adw::PreferencesGroup::builder()
            .title("Other variables")
            .description("Passed to the game as they are, for anything not listed above")
            .build();
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add a variable")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        add.update_property(&[gtk::accessible::Property::Label("Add a variable")]);
        group.set_header_suffix(Some(&add));
        page.add(&group);

        let this = Rc::new(OtherVariables {
            group,
            rows: RefCell::new(Vec::new()),
        });
        for (key, value) in values {
            this.add_row(key, value);
        }
        add.connect_clicked(glib::clone!(
            #[weak]
            this,
            move |_| {
                let name = this.add_row("", "");
                name.grab_focus();
            }
        ));
        this
    }

    fn add_row(self: &Rc<Self>, key: &str, value: &str) -> gtk::Entry {
        let name = gtk::Entry::builder()
            .text(key)
            .placeholder_text("NAME")
            .width_chars(22)
            .valign(gtk::Align::Center)
            .build();
        name.update_property(&[gtk::accessible::Property::Label("Variable name")]);
        let entry = gtk::Entry::builder()
            .text(value)
            .placeholder_text("value")
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label("Variable value")]);
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Remove this variable")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        remove.update_property(&[gtk::accessible::Property::Label("Remove this variable")]);
        let line = gtk::Box::builder()
            .spacing(8)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(6)
            .build();
        line.append(&name);
        line.append(&entry);
        line.append(&remove);
        let row = gtk::ListBoxRow::builder()
            .child(&line)
            .activatable(false)
            .selectable(false)
            .build();
        self.group.add(&row);
        self.rows
            .borrow_mut()
            .push((row.clone(), name.clone(), entry));
        remove.connect_clicked(glib::clone!(
            #[weak(rename_to = this)]
            self,
            #[weak]
            row,
            move |_| {
                this.group.remove(&row);
                this.rows.borrow_mut().retain(|(r, _, _)| *r != row);
            }
        ));
        name
    }

    fn values(&self) -> Vec<(String, String)> {
        self.rows
            .borrow()
            .iter()
            .map(|(_, name, value)| (name.text().to_string(), value.text().to_string()))
            .collect()
    }
}

/// The tabs of settings, each with its name and the sections it shows.
const OPTION_TABS: [(&str, &str, &[Section]); 4] = [
    ("display", "Display", &[Section::Display]),
    (
        "compatibility",
        "Compatibility",
        &[
            Section::Controllers,
            Section::Compatibility,
            Section::AntiCheat,
        ],
    ),
    ("upscaling", "Upscaling", &[Section::Upscaling]),
    (PROTON_TAB, "Proton", &[Section::Proton]),
];

/// The tab with the runtime choice, for opening the editor on it.
pub const PROTON_TAB: &str = "proton";

/// The runtimes a title can be given and the one it has.
pub struct Runtimes {
    /// `None` for a game Steam starts, whose runtime is Steam's choice.
    pub choices: Option<Vec<(PathBuf, String)>>,
    /// The choice saved for this title; `None` follows the default.
    pub chosen: Option<PathBuf>,
    /// What a launch uses now.
    pub launch: Option<PathBuf>,
}

impl Runtimes {
    /// The dropdown's entries after "Default", keeping a chosen runtime that
    /// is not installed any more so that saving does not drop it.
    fn entries(&self) -> Vec<(PathBuf, String)> {
        let mut entries = self.choices.clone().unwrap_or_default();
        if let Some(chosen) = &self.chosen {
            if !entries.iter().any(|(path, _)| path == chosen) {
                entries.push((
                    chosen.clone(),
                    format!("{} (not installed)", chosen.display()),
                ));
            }
        }
        entries
    }

    fn selected(&self, entries: &[(PathBuf, String)]) -> u32 {
        self.chosen
            .as_ref()
            .and_then(|chosen| entries.iter().position(|(path, _)| path == chosen))
            .map_or(0, |i| i as u32 + 1)
    }
}

fn entry_at(entries: &[(PathBuf, String)], selected: u32) -> Option<&PathBuf> {
    (selected as usize)
        .checked_sub(1)
        .and_then(|i| entries.get(i))
        .map(|(path, _)| path)
}

/// What the editor is opened with besides the recipe.
pub struct Setup {
    pub titles_dir: PathBuf,
    pub runtimes: Runtimes,
    /// The neural rendering tab's content.
    pub neural: gtk::Widget,
    /// The tab to open on; the first one unless named.
    pub tab: Option<&'static str>,
}

/// Show the editor for a recipe. `on_saved` is handed the path it wrote;
/// `on_reset` runs after the title's edits were set aside by "Reset to
/// defaults"; `on_runtime` saves a changed runtime choice.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    recipe: Recipe,
    setup: Setup,
    on_runtime: impl Fn(Option<&Path>) -> Result<(), String> + 'static,
    on_saved: impl Fn(Option<PathBuf>) + 'static,
    on_reset: impl Fn() + 'static,
) {
    let Setup {
        titles_dir,
        runtimes,
        neural,
        tab,
    } = setup;
    let draft = Draft::from_recipe(&recipe);
    let local = recipe.is_local();
    let recipe = Rc::new(RefCell::new(recipe));

    let dialog = adw::Dialog::builder()
        .title("Set up this title")
        .content_width(860)
        .content_height(720)
        .build();

    let name = adw::EntryRow::builder()
        .title("Name")
        .text(&draft.name)
        .build();
    let executable = adw::EntryRow::builder()
        .title(if local {
            "Executable, inside the game's folder or a full path"
        } else {
            "Executable, inside the installed title"
        })
        .text(&draft.executable)
        .build();
    let install_dir = adw::EntryRow::builder()
        .title(if local {
            "Game folder"
        } else {
            "Install directory"
        })
        .text(&draft.install_dir)
        .build();
    // Only a local game takes arguments and wrappers: the client's run
    // command takes neither.
    let arguments = adw::EntryRow::builder()
        .title("Arguments")
        .text(&draft.arguments)
        .visible(local)
        .build();
    let wrapper = adw::EntryRow::builder()
        .title("Wrapper commands")
        .text(&draft.wrapper)
        .visible(local)
        .build();
    let prefix_dir = adw::EntryRow::builder()
        .title("Wine prefix directory")
        .text(&draft.prefix_dir)
        .build();

    // The name first, because that is what a person recognises -- a row that
    // says "Product 9MSVBF0KZFVW" is asking somebody to set up something the
    // window has not told them the name of. The id stays, since it is what a
    // Store URL carries and what the recipe file is named after, and it is the
    // only thing there is when the catalog has not answered yet.
    let identity = adw::PreferencesGroup::builder()
        .title("Title")
        .description({
            let recipe = recipe.borrow();
            let id = &recipe.title.product_id;
            let named = recipe.title.name.trim();
            let what = if named.is_empty() || named.eq_ignore_ascii_case(id) {
                format!("Product {id}")
            } else {
                format!("{named} ({id})")
            };
            format!("{what} — saved to your own titles directory, so an upgrade will not undo it")
        })
        .build();
    identity.add(&name);
    identity.add(&executable);
    identity.add(&install_dir);
    identity.add(&prefix_dir);
    if local {
        identity.add(&arguments);
        identity.add(&wrapper);
    }

    let stack = gtk::Stack::builder().vexpand(true).build();
    let page = adw::PreferencesPage::new();
    page.add(&identity);
    stack.add_titled(&page, Some("general"), "General");

    let runtime_entries = Rc::new(runtimes.entries());
    let runtime_row = adw::ComboRow::builder()
        .title("Runtime")
        .subtitle("The Proton this game runs with. Default follows Settings.")
        .build();
    let mut controls = Vec::new();
    for (id, title, sections) in OPTION_TABS {
        let page = adw::PreferencesPage::new();
        if id == PROTON_TAB && runtimes.choices.is_some() {
            let labels: Vec<&str> = std::iter::once("Default")
                .chain(runtime_entries.iter().map(|(_, label)| label.as_str()))
                .collect();
            runtime_row.set_model(Some(&gtk::StringList::new(&labels)));
            runtime_row.set_selected(runtimes.selected(&runtime_entries));
            let group = adw::PreferencesGroup::new();
            group.add(&runtime_row);
            page.add(&group);
        }
        controls.extend(option_groups(
            &page,
            sections,
            &draft.chosen,
            runtimes.launch.as_deref(),
        ));
        stack.add_titled(&page, Some(id), title);
    }
    let controls = Rc::new(controls);
    let page = adw::PreferencesPage::new();
    let others = Rc::new(OtherVariables::new(&page, &draft.other));
    stack.add_titled(&page, Some("variables"), "Variables");
    stack.add_titled(&neural, Some("neural"), "Neural rendering (experimental)");
    if let Some(tab) = tab {
        stack.set_visible_child_name(tab);
    }
    let first_runtime = runtime_row.selected();
    {
        // Which flags a runtime knows changes with the runtime. Which one
        // "Default" is, is known only while it is what a launch uses.
        let (controls, entries) = (controls.clone(), runtime_entries.clone());
        let launch = runtimes.launch.clone();
        runtime_row.connect_selected_notify(move |row| {
            let runtime = match entry_at(&entries, row.selected()) {
                Some(path) => Some(path.clone()),
                None if first_runtime == 0 => launch.clone(),
                None => None,
            };
            for control in controls.iter().filter(|c| c.option().is_proton_flag()) {
                control.describe_for(runtime.as_deref());
            }
        });
    }
    let switcher = gtk::StackSwitcher::builder()
        .stack(&stack)
        .halign(gtk::Align::Center)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(6)
        .build();
    let tabs = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&switcher)
        .build();

    let header = adw::HeaderBar::new();
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);

    // A way back from an experiment: whatever was changed here, the title
    // returns to the recipe Ferestre ships (or detects). Only when there is an
    // edit to undo.
    let product_id = recipe.borrow().title.product_id.clone();
    let has_edit = Recipe::path_for(&product_id, &titles_dir)
        .map(|path| path.exists())
        .unwrap_or(false);
    // A local game's recipe is the game: there are no defaults behind it.
    let local = recipe.borrow().is_local();
    let reset = gtk::Button::builder()
        .label("Reset to defaults")
        .css_classes(["destructive-action"])
        .sensitive(has_edit && !local)
        .tooltip_text(if local {
            "A local game has no defaults; its settings here are the game"
        } else if has_edit {
            "Undo every change made here for this title"
        } else {
            "Nothing has been changed for this title"
        })
        .build();
    header.pack_start(&reset);

    let toasts = crate::notifications::Toasts::new();
    toasts.set_child(Some(&stack));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&tabs);
    toolbar.set_content(Some(toasts.widget()));
    dialog.set_child(Some(&toolbar));

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }

    {
        let dialog = dialog.clone();
        let toasts = toasts.clone();
        let titles_dir = titles_dir.clone();
        let name = recipe.borrow().title.name.clone();
        let on_reset = Rc::new(on_reset);
        reset.connect_clicked(move |button| {
            // The file the reset sets aside, named in full: it is where the edit
            // can be got back from by hand.
            let kept = Recipe::path_for(&product_id, &titles_dir)
                .map(|path| path.with_extension("toml.bak").display().to_string())
                .unwrap_or_else(|_| "a .bak file".into());
            let confirm = adw::AlertDialog::new(
                Some(&format!("Reset {name} to its defaults?")),
                Some(&format!(
                    "Every launch setting changed here for this title is undone. It uses \
                     the settings Ferestre ships for it again, or, for a title Ferestre has \
                     none for, works them out again the next time it starts.\n\n\
                     The settings it has now are kept in {kept}, replacing an older copy \
                     there."
                )),
            );
            confirm.add_response("cancel", "Cancel");
            confirm.add_response("reset", "Reset");
            confirm.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
            confirm.set_default_response(Some("cancel"));
            confirm.set_close_response("cancel");
            let dialog = dialog.clone();
            let toasts = toasts.clone();
            let titles_dir = titles_dir.clone();
            let product_id = product_id.clone();
            let on_reset = on_reset.clone();
            confirm.connect_response(None, move |_, response| {
                if response != "reset" {
                    return;
                }
                match Recipe::reset_in(&product_id, &titles_dir) {
                    Ok(_) => {
                        on_reset();
                        dialog.close();
                    }
                    Err(e) => toasts.add_toast(crate::toast(format!("Could not reset: {e}"))),
                }
            });
            confirm.present(Some(button));
        });
    }

    save.connect_clicked(glib::clone!(
        #[strong]
        dialog,
        #[strong]
        recipe,
        #[strong]
        toasts,
        #[strong]
        name,
        #[strong]
        executable,
        #[strong]
        install_dir,
        #[strong]
        prefix_dir,
        #[strong]
        controls,
        #[strong]
        others,
        #[strong]
        arguments,
        #[strong]
        wrapper,
        move |_| {
            let draft = Draft {
                name: name.text().to_string(),
                executable: executable.text().to_string(),
                install_dir: install_dir.text().to_string(),
                prefix_dir: prefix_dir.text().to_string(),
                chosen: controls.iter().filter_map(OptionControl::value).collect(),
                other: others.values(),
                arguments: arguments.text().to_string(),
                wrapper: wrapper.text().to_string(),
            };
            let mut edited = recipe.borrow().clone();
            if let Err(complaint) = draft.apply(&mut edited) {
                toasts.add_toast(crate::toast(&complaint));
                return;
            }
            if runtime_row.selected() != first_runtime {
                let path = entry_at(&runtime_entries, runtime_row.selected());
                if let Err(e) = on_runtime(path.map(PathBuf::as_path)) {
                    toasts.add_toast(crate::toast(format!("Could not save the runtime: {e}")));
                    return;
                }
                // Only the runtime changed: an edit of the recipe would be
                // one nobody made, and would keep winning over a shipped one.
                if same_recipe(&edited, &recipe.borrow()) {
                    on_saved(None);
                    dialog.close();
                    return;
                }
            }
            // Somebody has looked at it now: an edited recipe is theirs, and
            // keeps winning over one shipped later, as a generated one would
            // not.
            edited.generated = false;
            match edited.save_to(&titles_dir) {
                Ok(path) => {
                    on_saved(Some(path));
                    dialog.close();
                }
                Err(e) => toasts.add_toast(crate::toast(format!("Could not save: {e}"))),
            }
        }
    ));

    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Recipe {
        Recipe::blank("9ZZTESTNEW1", "Test Title")
    }

    #[test]
    fn a_draft_round_trips_through_a_recipe() {
        let mut recipe = blank();
        recipe.launch.executable = "Game/Test.exe".into();
        recipe.launch.env.insert("B".into(), "2".into());
        recipe.launch.env.insert("A".into(), "1".into());

        let draft = Draft::from_recipe(&recipe);
        assert_eq!(draft.executable, "Game/Test.exe");
        assert_eq!(
            draft.other,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string())
            ],
            "sorted, so a diff is readable"
        );
        assert_eq!(
            draft.install_dir, "test-title",
            "the default, shown not hidden"
        );

        let mut applied = blank();
        draft.apply(&mut applied).expect("applies");
        assert_eq!(applied.launch.executable, "Game/Test.exe");
        assert_eq!(applied.launch.env, recipe.launch.env);
    }

    /// A recipe that cannot launch is worse than no recipe: it reads as the
    /// launcher failing rather than as an unfinished description.
    #[test]
    fn an_empty_executable_is_refused() {
        let draft = Draft {
            name: "Test".into(),
            executable: "   ".into(),
            install_dir: String::new(),
            prefix_dir: String::new(),
            chosen: BTreeMap::new(),
            other: Vec::new(),
            arguments: String::new(),
            wrapper: String::new(),
        };
        assert!(draft.apply(&mut blank()).is_err());
    }

    /// The executable is a path inside the installed title. An absolute path
    /// or one climbing out of it launches something that is not the game.
    #[test]
    fn an_executable_that_escapes_the_install_is_refused() {
        for attempt in ["/usr/bin/id", "../../usr/bin/id", "Game/../../etc/passwd"] {
            let draft = Draft {
                name: "Test".into(),
                executable: attempt.into(),
                install_dir: String::new(),
                prefix_dir: String::new(),
                chosen: BTreeMap::new(),
                other: Vec::new(),
                arguments: String::new(),
                wrapper: String::new(),
            };
            assert!(
                draft.apply(&mut blank()).is_err(),
                "{attempt} should be refused"
            );
        }
    }

    /// Known settings open as their controls and everything else as a
    /// variable row, and saving puts the same environment back.
    #[test]
    fn settings_and_other_variables_round_trip() {
        let mut recipe = blank();
        recipe.launch.executable = "Game.exe".into();
        for (key, value) in [
            ("FERESTRE_NATIVE_WAYLAND", "1"),
            ("FERESTRE_PERFORMANCE", "0"),
            ("DXVK_HUD", "fps"),
        ] {
            recipe.launch.env.insert(key.into(), value.into());
        }
        let draft = Draft::from_recipe(&recipe);
        assert_eq!(draft.chosen.len(), 2);
        assert_eq!(
            draft.other,
            vec![("DXVK_HUD".to_string(), "fps".to_string())]
        );

        let mut applied = blank();
        draft.apply(&mut applied).expect("applies");
        assert_eq!(applied.launch.env, recipe.launch.env);

        // A variable row that names a setting shown above is refused, not
        // silently merged with it.
        let clash = Draft {
            other: vec![("FERESTRE_NATIVE_WAYLAND".into(), "1".into())],
            ..Draft::from_recipe(&recipe)
        };
        let complaint = clash.apply(&mut blank()).expect_err("refuses");
        assert!(complaint.contains("Native Wayland"), "{complaint}");
    }

    #[test]
    fn every_section_is_on_one_tab() {
        for section in Section::ALL {
            let tabs = OPTION_TABS
                .iter()
                .filter(|(_, _, sections)| sections.contains(&section))
                .count();
            assert_eq!(tabs, 1, "{section:?}");
        }
    }

    #[test]
    fn a_missing_chosen_runtime_stays_chosen() {
        let runtimes = Runtimes {
            choices: Some(vec![("/r/a".into(), "A".into())]),
            chosen: Some("/r/gone".into()),
            launch: None,
        };
        let entries = runtimes.entries();
        let selected = runtimes.selected(&entries);
        assert_eq!(
            entry_at(&entries, selected),
            Some(&PathBuf::from("/r/gone"))
        );
        assert_eq!(entry_at(&entries, 0), None, "the default");
    }

    #[test]
    fn choosing_only_a_runtime_is_not_an_edit_of_the_recipe() {
        let mut recipe = blank();
        recipe.launch.executable = "Game.exe".into();
        let mut edited = recipe.clone();
        let draft = Draft::from_recipe(&recipe);
        draft.apply(&mut edited).unwrap();
        assert!(same_recipe(&edited, &recipe));
        edited.launch.env.insert("A".into(), "1".into());
        assert!(!same_recipe(&edited, &recipe));
    }

    #[test]
    fn a_flag_the_runtime_does_not_read_says_so() {
        let dir = std::env::temp_dir().join(format!("ferestre-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proton"), "PROTON_FSR4_UPGRADE").unwrap();
        let subtitle =
            |key| option_subtitle(launch_options::find(key).expect("listed"), Some(&dir));
        assert!(subtitle("PROTON_USE_OPTISCALER").contains("Not known"));
        assert!(!subtitle("PROTON_FSR4_UPGRADE").contains("Not known"));
        assert!(!subtitle("FERESTRE_NATIVE_WAYLAND").contains("Not known"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Storing the computed default would freeze it, so a later rename would
    /// not follow the directory.
    #[test]
    fn directories_left_at_their_default_are_not_written_out() {
        let recipe = blank();
        let draft = Draft {
            executable: "Game.exe".into(),
            ..Draft::from_recipe(&recipe)
        };
        let mut applied = blank();
        draft.apply(&mut applied).expect("applies");
        assert!(applied.install.dir.is_none());
        assert!(applied.install.prefix.is_none());
        assert_eq!(applied.install_dir(), "test-title");

        let moved = Draft {
            install_dir: "somewhere-else".into(),
            ..draft
        };
        let mut applied = blank();
        moved.apply(&mut applied).expect("applies");
        assert_eq!(applied.install.dir.as_deref(), Some("somewhere-else"));
    }

    /// "Set up" on a title with no recipe fills in where the title is, so the
    /// saved recipe points at it rather than at its name's slug.
    #[test]
    fn set_up_points_at_the_folder_the_title_is_in() {
        let games = std::path::Path::new("/home/u/Games");
        let at = |dir: &str| relative_install_dir(games, std::path::Path::new(dir));
        assert_eq!(
            at("/home/u/Games/9nr1r1xwlcnb").as_deref(),
            Some("9nr1r1xwlcnb")
        );
        assert_eq!(
            at("/home/u/Games/Forza Horizon/game").as_deref(),
            Some("Forza Horizon/game")
        );
        assert_eq!(
            at("/mnt/elsewhere/fh5"),
            None,
            "a recipe can only name a folder under games"
        );
        assert_eq!(at("/home/u/Games"), None);

        let mut recipe = Recipe::blank("9NR1R1XWLCNB", "Forza Horizon 6");
        recipe.install.dir = at("/home/u/Games/9nr1r1xwlcnb");
        let draft = Draft {
            executable: "Game.exe".into(),
            ..Draft::from_recipe(&recipe)
        };
        assert_eq!(draft.install_dir, "9nr1r1xwlcnb");
        draft.apply(&mut recipe).expect("applies");
        assert_eq!(recipe.install_dir(), "9nr1r1xwlcnb");
    }

    fn local() -> Recipe {
        Recipe::parse(
            "schema = 1\n[title]\nproduct-id = \"LOCAL-T\"\nname = \"Local Test\"\n\
             slug = \"local-t\"\nkind = \"local\"\n[install]\ndir = \"/run/media/d/Local Test\"\n\
             [launch]\nexecutable = 'Game.exe'\n[status]\nstate = \"untested\"\n\
             summary = \"Added by hand\"\n",
        )
        .unwrap()
    }

    #[test]
    fn a_local_draft_keeps_absolute_paths_arguments_and_wrappers() {
        let recipe = local();
        let draft = Draft {
            executable: "/run/media/d/Local Test/bin/Game.exe".into(),
            prefix_dir: "/run/media/d/prefixes/Local Test".into(),
            arguments: "-dx12 \"a b\"".into(),
            wrapper: "gamemoderun".into(),
            ..Draft::from_recipe(&recipe)
        };
        assert_eq!(draft.install_dir, "/run/media/d/Local Test");
        let mut applied = recipe.clone();
        draft.apply(&mut applied).expect("applies");
        assert_eq!(applied.launch.args, ["-dx12", "a b"]);
        assert_eq!(applied.launch.wrapper, ["gamemoderun"]);
        assert_eq!(
            applied.install.dir.as_deref(),
            Some("/run/media/d/Local Test")
        );
        assert_eq!(
            applied.launch.executable,
            "/run/media/d/Local Test/bin/Game.exe"
        );
        assert_eq!(
            applied.install.prefix.as_deref(),
            Some("/run/media/d/prefixes/Local Test")
        );
        // And back: the words are shown so they parse the same again.
        let again = Draft::from_recipe(&applied);
        assert_eq!(again.arguments, "-dx12 'a b'");
        assert_eq!(again.wrapper, "gamemoderun");

        let climbing = Draft {
            executable: "/run/media/d/../../etc/x.exe".into(),
            ..Draft::from_recipe(&recipe)
        };
        assert!(climbing.apply(&mut recipe.clone()).is_err());

        let xbox = Draft {
            executable: "/usr/bin/id".into(),
            ..Draft::from_recipe(&blank())
        };
        assert!(xbox.apply(&mut blank()).is_err());
    }

    #[test]
    fn an_environment_word_is_not_a_wrapper() {
        let draft = Draft {
            wrapper: "DXVK_HDR=1 gamemoderun".into(),
            ..Draft::from_recipe(&local())
        };
        let complaint = draft.apply(&mut local()).expect_err("refuses");
        assert!(complaint.contains("Environment"), "{complaint}");
    }
}

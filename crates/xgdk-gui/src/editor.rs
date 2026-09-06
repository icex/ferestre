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
use std::path::PathBuf;
use std::rc::Rc;

use xgdk_core::recipe::Recipe;

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
    /// `KEY=VALUE` per line, as it is typed.
    pub environment: String,
}

impl Draft {
    pub fn from_recipe(recipe: &Recipe) -> Self {
        Draft {
            name: recipe.title.name.clone(),
            executable: recipe.launch.executable.clone(),
            install_dir: recipe.install_dir().to_string(),
            prefix_dir: recipe.prefix_dir(),
            environment: recipe
                .launch
                .env
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("\n"),
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
        if executable.starts_with('/') || executable.contains("..") {
            return Err(
                "The executable is a path inside the installed title, like \"Game/Game.exe\"."
                    .into(),
            );
        }

        let mut environment = std::collections::BTreeMap::new();
        for (number, line) in self.environment.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!(
                    "Line {} of the environment is not KEY=VALUE: {line:?}",
                    number + 1
                ));
            };
            let key = key.trim();
            if key.is_empty() {
                return Err(format!("Line {} of the environment has no name.", number + 1));
            }
            environment.insert(key.to_string(), value.trim().to_string());
        }

        let name = self.name.trim();
        if !name.is_empty() {
            recipe.title.name = name.to_string();
        }
        recipe.launch.executable = executable.to_string();
        recipe.launch.env = environment;
        // Empty means "the default", which is the slug -- storing the computed
        // default would freeze it, so a later rename would not follow.
        recipe.install.dir = non_empty(&self.install_dir).filter(|d| *d != recipe.title.slug);
        recipe.install.prefix =
            non_empty(&self.prefix_dir).filter(|d| *d != format!("{}-proton", recipe.title.slug));
        Ok(())
    }
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Show the editor for a recipe. `on_saved` is handed the path it wrote.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    recipe: Recipe,
    titles_dir: PathBuf,
    on_saved: impl Fn(PathBuf) + 'static,
) {
    let draft = Draft::from_recipe(&recipe);
    let recipe = Rc::new(RefCell::new(recipe));

    let dialog = adw::Dialog::builder()
        .title("Set up this title")
        .content_width(560)
        .content_height(620)
        .build();

    let name = adw::EntryRow::builder().title("Name").text(&draft.name).build();
    let executable = adw::EntryRow::builder()
        .title("Executable, inside the installed title")
        .text(&draft.executable)
        .build();
    let install_dir = adw::EntryRow::builder()
        .title("Install directory")
        .text(&draft.install_dir)
        .build();
    let prefix_dir = adw::EntryRow::builder()
        .title("Wine prefix directory")
        .text(&draft.prefix_dir)
        .build();

    let environment = gtk::TextView::builder()
        .monospace(true)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    environment.buffer().set_text(&draft.environment);

    let identity = adw::PreferencesGroup::builder()
        .title("Title")
        .description(&format!(
            "Product {} — saved to your own titles directory, so an upgrade will not undo it",
            recipe.borrow().title.product_id
        ))
        .build();
    identity.add(&name);
    identity.add(&executable);
    identity.add(&install_dir);
    identity.add(&prefix_dir);

    let env_group = adw::PreferencesGroup::builder()
        .title("Environment")
        .description("One KEY=VALUE per line, passed to the title when it starts")
        .build();
    let env_frame = gtk::Frame::builder().height_request(140).child(&environment).build();
    env_group.add(&env_frame);

    let page = adw::PreferencesPage::new();
    page.add(&identity);
    page.add(&env_group);

    let header = adw::HeaderBar::new();
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&page));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&toasts));
    dialog.set_child(Some(&toolbar));

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }

    save.connect_clicked(glib::clone!(
        #[strong] dialog,
        #[strong] recipe,
        #[strong] toasts,
        #[strong] name,
        #[strong] executable,
        #[strong] install_dir,
        #[strong] prefix_dir,
        #[strong] environment,
        move |_| {
            let buffer = environment.buffer();
            let draft = Draft {
                name: name.text().to_string(),
                executable: executable.text().to_string(),
                install_dir: install_dir.text().to_string(),
                prefix_dir: prefix_dir.text().to_string(),
                environment: buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string(),
            };
            let mut edited = recipe.borrow().clone();
            if let Err(complaint) = draft.apply(&mut edited) {
                toasts.add_toast(adw::Toast::new(&complaint));
                return;
            }
            match edited.save_to(&titles_dir) {
                Ok(path) => {
                    on_saved(path);
                    dialog.close();
                }
                Err(e) => toasts.add_toast(adw::Toast::new(&format!("Could not save: {e}"))),
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
        assert_eq!(draft.environment, "A=1\nB=2", "sorted, so a diff is readable");
        assert_eq!(draft.install_dir, "test-title", "the default, shown not hidden");

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
            environment: String::new(),
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
                environment: String::new(),
            };
            assert!(draft.apply(&mut blank()).is_err(), "{attempt} should be refused");
        }
    }

    #[test]
    fn the_environment_is_parsed_and_complains_with_a_line_number() {
        let draft = Draft {
            name: "Test".into(),
            executable: "Game.exe".into(),
            install_dir: String::new(),
            prefix_dir: String::new(),
            environment: "# a comment\n\n  A = 1  \nB=has=equals\n".into(),
        };
        let mut recipe = blank();
        draft.apply(&mut recipe).expect("applies");
        assert_eq!(recipe.launch.env.get("A").map(String::as_str), Some("1"));
        assert_eq!(
            recipe.launch.env.get("B").map(String::as_str),
            Some("has=equals"),
            "only the first = separates"
        );
        assert_eq!(recipe.launch.env.len(), 2, "comments and blanks are not variables");

        let bad = Draft {
            environment: "A=1\nnot a variable\n".into(),
            ..draft
        };
        let complaint = bad.apply(&mut blank()).expect_err("refuses");
        assert!(complaint.contains("Line 2"), "{complaint}");
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
}

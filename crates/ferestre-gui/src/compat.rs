//! How well a title is known to run: the badge on its tile, and the same in
//! full in its details.
use super::*;
use ferestre_core::recipe::{Issue, Recipe, Status, TitleState};

/// What was seen when a title was run.
#[derive(Debug, Clone)]
pub struct Report {
    pub state: TitleState,
    pub summary: String,
    /// The day it was last run, as written in the recipe.
    pub checked: Option<String>,
    pub issues: Vec<Issue>,
}

impl Report {
    fn new(status: &Status, issues: &[Issue]) -> Report {
        Report {
            state: status.state,
            summary: status.summary.clone(),
            checked: status.last_verified.as_ref().map(|day| day.to_string()),
            issues: issues.to_vec(),
        }
    }

    pub fn untested() -> Report {
        Report {
            state: TitleState::Untested,
            summary: String::new(),
            checked: None,
            issues: Vec::new(),
        }
    }

    /// The words on the badge, and its style: those of "Jump back in".
    pub fn badge(&self) -> (&'static str, &'static str) {
        match self.state {
            TitleState::Playable => ("Playable", "ok"),
            TitleState::PlayableWithIssues => ("Playable with issues", "warning"),
            TitleState::Menus | TitleState::Broken => ("Not playable", "error"),
            TitleState::Untested => ("Untested", "untested"),
        }
    }

    pub fn label(&self) -> &'static str {
        self.badge().0
    }

    /// The sentence under the label in the details.
    fn meaning(&self) -> &'static str {
        match self.state {
            TitleState::Playable => "Played, and nothing is known to be broken.",
            TitleState::PlayableWithIssues => "Plays, with problems to know about first.",
            TitleState::Menus => "Starts and reaches its menus. Nobody has got into play.",
            TitleState::Broken => "Starts, and does not get far enough to play.",
            TitleState::Untested => "Nobody has reported running it here. It may still work.",
        }
    }
}

/// The reports of the packaged recipes, by uppercase product id. An edited
/// copy of a recipe keeps the state it was copied with, so these come first.
pub fn packaged(recipes: &[Recipe]) -> BTreeMap<String, Report> {
    recipes
        .iter()
        .filter(|recipe| recipe.status.state != TitleState::Untested)
        .map(|recipe| {
            (
                recipe.title.product_id.to_ascii_uppercase(),
                Report::new(&recipe.status, &recipe.issues),
            )
        })
        .collect()
}

/// The report for the first of `ids` that has one.
pub fn report(packaged: &BTreeMap<String, Report>, recipes: &[Recipe], ids: &[&str]) -> Report {
    ids.iter()
        .find_map(|id| {
            packaged.get(&id.to_ascii_uppercase()).cloned().or_else(|| {
                recipes
                    .iter()
                    .find(|r| r.title.product_id.eq_ignore_ascii_case(id))
                    .filter(|r| r.status.state != TitleState::Untested)
                    .map(|r| Report::new(&r.status, &r.issues))
            })
        })
        .unwrap_or_else(Report::untested)
}

/// The badge on a tile, and at the head of the details' section.
pub fn badge(report: &Report) -> gtk::Label {
    let (words, class) = report.badge();
    let badge = gtk::Label::builder()
        .label(words)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["tile-badge", class])
        .can_target(false)
        .build();
    badge.update_property(&[gtk::accessible::Property::Label(&format!(
        "Compatibility: {words}"
    ))]);
    badge
}

/// The details' section.
pub fn section(report: &Report) -> gtk::Box {
    let words = |text: &str, classes: &[&str]| {
        gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(classes.to_vec())
            .build()
    };
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.add_css_class("details-card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    head.append(&words("Compatibility", &["heading"]));
    head.append(&badge(report));
    section.append(&head);
    section.append(&words(report.meaning(), &[]));
    if !report.summary.is_empty() {
        section.append(&words(&report.summary, &["dim-label"]));
    }
    if let Some(day) = &report.checked {
        section.append(&words(
            &format!("Last checked {day}"),
            &["caption", "dim-label"],
        ));
    }
    if report.issues.is_empty() {
        return section;
    }
    let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
    for issue in &report.issues {
        let one = gtk::Box::new(gtk::Orientation::Vertical, 2);
        one.append(&words(&issue.symptom, &["heading"]));
        if let Some(fix) = &issue.fix {
            one.append(&words(fix, &["dim-label"]));
        }
        list.append(&one);
    }
    match report.state {
        // Met before and dealt with: there for whoever meets one again.
        TitleState::Playable | TitleState::Untested => {
            list.set_margin_top(8);
            let expander = gtk::Expander::builder()
                .label("Problems seen before, and what to do")
                .child(&list)
                .build();
            section.append(&expander);
        }
        _ => {
            let warning = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            warning.add_css_class("compat-warning");
            let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
            icon.set_valign(gtk::Align::Start);
            warning.append(&icon);
            list.set_hexpand(true);
            warning.append(&list);
            section.append(&warning);
        }
    }
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(id: &str, state: &str) -> Recipe {
        Recipe::parse(&format!(
            "schema = 1\n[title]\nproduct-id = \"{id}\"\nname = \"N\"\nslug = \"n\"\n\
             [launch]\nexecutable = \"Game.exe\"\n[status]\nstate = \"{state}\"\n\
             summary = \"{state} summary\"\n"
        ))
        .unwrap()
    }

    #[test]
    fn a_packaged_result_outlives_an_edited_copy() {
        let shipped = packaged(&[recipe("9zza", "playable-with-issues")]);
        let edited = [recipe("9ZZA", "playable"), recipe("9ZZB", "broken")];
        assert_eq!(
            report(&shipped, &edited, &["9ZZA"]).label(),
            "Playable with issues"
        );
        assert_eq!(report(&shipped, &edited, &["9zzb"]).label(), "Not playable");
    }

    #[test]
    fn nothing_known_is_untested_and_a_bundle_uses_its_game() {
        let shipped = packaged(&[recipe("9ZZGAME", "playable"), recipe("9ZZNEW", "untested")]);
        assert_eq!(report(&shipped, &[], &["9ZZNEW"]).label(), "Untested");
        assert_eq!(report(&shipped, &[], &["OTHER"]).label(), "Untested");
        assert_eq!(
            report(&shipped, &[], &["BUNDLE", "9ZZGAME"]).label(),
            "Playable"
        );
    }
}

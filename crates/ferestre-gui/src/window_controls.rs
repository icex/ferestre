//! Follow the desktop's GTK decoration setting, including split-view headers.
use super::*;

// Adwaita puts controls at the outside edges of an expanded split view. Avoid
// adding an empty sidebar toolbar when the desktop puts every control at right.
fn sidebar_has_controls(layout: &str, collapsed: bool, fullscreen: bool) -> bool {
    if fullscreen {
        return false;
    }
    let (left, right) = layout.split_once(':').unwrap_or((layout, ""));
    has_buttons(left) || (collapsed && has_buttons(right))
}

fn has_buttons(side: &str) -> bool {
    side.split(',')
        .any(|name| matches!(name.trim(), "close" | "minimize" | "maximize" | "icon"))
}

pub(super) fn install(
    window: &adw::ApplicationWindow,
    split: &adw::NavigationSplitView,
    sidebar: &adw::HeaderBar,
    content: &adw::HeaderBar,
) {
    // A full group of start-side controls would force the icon rail wider.
    // Use the same desktop layout at the start of the content header while
    // both panes are visible. Adwaita handles the other layouts itself.
    let rail_controls = gtk::WindowControls::new(gtk::PackType::Start);
    content.pack_start(&rail_controls);
    // Pack the desktop controls inside the fullscreen button at the end.
    let end_controls = gtk::WindowControls::new(gtk::PackType::End);
    content.set_show_end_title_buttons(false);
    content.pack_end(&end_controls);
    let (weak_window, weak_split, weak_sidebar, weak_content, weak_rail_controls, weak_end) = (
        window.downgrade(),
        split.downgrade(),
        sidebar.downgrade(),
        content.downgrade(),
        rail_controls.downgrade(),
        end_controls.downgrade(),
    );
    let update = Rc::new(move || {
        let (
            Some(window),
            Some(split),
            Some(sidebar),
            Some(content),
            Some(rail_controls),
            Some(end_controls),
        ) = (
            weak_window.upgrade(),
            weak_split.upgrade(),
            weak_sidebar.upgrade(),
            weak_content.upgrade(),
            weak_rail_controls.upgrade(),
            weak_end.upgrade(),
        )
        else {
            return;
        };
        let fullscreen = window.is_fullscreen();
        let collapsed = split.is_collapsed();
        let layout = window.settings().gtk_decoration_layout();
        let layout = layout.as_deref().unwrap_or_default();
        let has_sidebar_controls = sidebar_has_controls(layout, collapsed, fullscreen);
        let moved = !collapsed && window.has_css_class("compact-sidebar") && has_sidebar_controls;
        sidebar.set_visible(has_sidebar_controls && !moved);
        sidebar.set_show_start_title_buttons(!fullscreen);
        sidebar.set_show_end_title_buttons(!fullscreen);
        content.set_show_start_title_buttons(!fullscreen && !moved);
        rail_controls.set_visible(moved);
        let right = layout.split_once(':').map_or("", |(_, right)| right);
        end_controls.set_visible(!fullscreen && has_buttons(right));
    });
    let refresh = update.clone();
    window
        .settings()
        .connect_gtk_decoration_layout_notify(move |_| refresh());
    let refresh = update.clone();
    split.connect_collapsed_notify(move |_| refresh());
    let refresh = update.clone();
    window.connect_css_classes_notify(move |_| refresh());
    let refresh = update.clone();
    window.connect_fullscreened_notify(move |_| refresh());
    update();
}

#[cfg(test)]
mod tests {
    use super::sidebar_has_controls;

    #[test]
    fn desktop_left_controls_have_a_sidebar_header() {
        for layout in [
            "close,minimize,maximize:",
            "close:",
            "close:minimize,maximize",
        ] {
            assert!(sidebar_has_controls(layout, false, false), "{layout}");
        }
    }

    #[test]
    fn right_controls_do_not_leave_an_empty_desktop_sidebar_header() {
        for layout in [":minimize,maximize,close", "menu:close", ":close"] {
            assert!(!sidebar_has_controls(layout, false, false), "{layout}");
        }
    }

    #[test]
    fn collapsed_navigation_keeps_controls_on_either_visible_page() {
        for layout in [
            "close,minimize,maximize:",
            ":minimize,maximize,close",
            ":close",
        ] {
            assert!(sidebar_has_controls(layout, true, false), "{layout}");
        }
    }

    #[test]
    fn fullscreen_has_no_sidebar_window_controls() {
        for layout in ["close:", ":close", "close:minimize,maximize"] {
            for collapsed in [false, true] {
                assert!(!sidebar_has_controls(layout, collapsed, true));
            }
        }
    }

    #[test]
    fn empty_or_unsupported_layouts_leave_no_blank_toolbar() {
        for layout in ["", ":", "menu:", "unknown:unsupported"] {
            for collapsed in [false, true] {
                assert!(!sidebar_has_controls(layout, collapsed, false));
            }
        }
    }

    #[test]
    fn layout_without_a_separator_uses_the_start_side() {
        assert!(sidebar_has_controls(
            "close,minimize,maximize",
            false,
            false
        ));
    }
}

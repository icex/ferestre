//! Follow the desktop's GTK decoration setting, including split-view headers.
use super::*;

// Adwaita puts controls at the outside edges of an expanded split view. Avoid
// adding an empty sidebar toolbar when the desktop puts every control at right.
fn sidebar_has_controls(layout: &str, collapsed: bool, fullscreen: bool) -> bool {
    if fullscreen {
        return false;
    }
    let (left, right) = layout.split_once(':').unwrap_or((layout, ""));
    let has_buttons = |side: &str| {
        side.split(',')
            .any(|name| matches!(name.trim(), "close" | "minimize" | "maximize" | "icon"))
    };
    has_buttons(left) || (collapsed && has_buttons(right))
}

pub(super) fn install(
    window: &adw::ApplicationWindow,
    split: &adw::NavigationSplitView,
    sidebar: &adw::HeaderBar,
    content: &adw::HeaderBar,
) {
    // No decoration-layout override: both headers inherit GtkSettings and its
    // desktop notifications. Native split-view integration suppresses controls
    // in the middle and restores them on whichever page is visible when narrow.
    let (weak_window, weak_split, weak_sidebar, weak_content) = (
        window.downgrade(),
        split.downgrade(),
        sidebar.downgrade(),
        content.downgrade(),
    );
    let update = Rc::new(move || {
        let (Some(window), Some(split), Some(sidebar), Some(content)) = (
            weak_window.upgrade(),
            weak_split.upgrade(),
            weak_sidebar.upgrade(),
            weak_content.upgrade(),
        ) else {
            return;
        };
        let fullscreen = window.is_fullscreen();
        for header in [&sidebar, &content] {
            header.set_show_start_title_buttons(!fullscreen);
            header.set_show_end_title_buttons(!fullscreen);
        }
        let layout = window.settings().gtk_decoration_layout();
        sidebar.set_visible(sidebar_has_controls(
            layout.as_deref().unwrap_or_default(),
            split.is_collapsed(),
            fullscreen,
        ));
    });
    let refresh = update.clone();
    window
        .settings()
        .connect_gtk_decoration_layout_notify(move |_| refresh());
    let refresh = update.clone();
    split.connect_collapsed_notify(move |_| refresh());
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

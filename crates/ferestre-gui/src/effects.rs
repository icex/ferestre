//! The page-switch transition and the blur behind dialogs, and when not to
//! draw them.
//!
//! Both are CSS (see `STYLE`): a short `page-enter` animation on a page that
//! replaced another, and a `filter: blur()` on the window's content while an
//! `adw::Dialog` is up. The window carries the classes that pick between them:
//!
//! - `blur-ok`: blur is affordable. A blurred frame costs about 1-3 ms on a
//!   GPU even at 32:9, but 15-800 ms on cairo or software GL, so it is set
//!   only on a hardware renderer, and taken away for the session if two of
//!   the first transitions' frames come in slow, or one frame is very slow
//!   (llvmpipe reports itself as GL).
//! - `reduced-motion`: the desktop asked for no animation
//!   (`gtk-enable-animations` false). GTK already skips CSS animations and
//!   transitions then; the class makes that explicit and also keeps the
//!   dialog blur off, so nothing moves or smears.
//! - `dialog-open`: a dialog is presented on the window.
//!
//! Without `blur-ok` a page fades in on opacity alone, which costs about the
//! same as drawing nothing extra on every renderer, and a dialog gets
//! libadwaita's own dimming, restyled darker.
use super::*;

/// Classes on the window that say what the content may do.
const BLUR_OK: &str = "blur-ok";
const REDUCED_MOTION: &str = "reduced-motion";
const DIALOG_OPEN: &str = "dialog-open";
/// The class a page root carries for its entry animation.
pub(crate) const PAGE_ENTER: &str = "page-enter";

thread_local! {
    /// Set once frames during transitions were too slow for blur: kept for
    /// the rest of the session, so it does not flicker on and off.
    static SLOW: Cell<bool> = const { Cell::new(false) };
    /// Transitions measured so far, and how many of them were slow.
    static MEASURED: Cell<u32> = const { Cell::new(0) };
    static SLOW_SAMPLES: Cell<u32> = const { Cell::new(0) };
    /// A measurement is running (its tick callback is on the window).
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}

/// How many transitions are timed before blur is trusted for the session.
const MEASURE_TRANSITIONS: u32 = 4;
/// Frames skipped at the start of a transition: they carry the new page's
/// build and layout, which is not what blur costs.
const SKIP_FRAMES: i64 = 2;
/// Frames timed after those.
const SAMPLE_FRAMES: i64 = 4;
/// A transition is slow when its frames average more than this (µs)...
const SLOW_FRAME: i64 = 25_000;
/// ...and blur goes after this many slow ones, or at once for a single frame
/// this long, which only a software rasteriser takes.
const SLOW_TRANSITIONS: u32 = 2;
const HOPELESS_FRAME: i64 = 120_000;
/// Long enough for the page animation to have finished: the window's classes
/// change only then, so a running animation is not swapped and restarted.
const AFTER_ANIMATION_MS: u64 = 260;

fn animations_enabled() -> bool {
    gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
}

/// A renderer that draws blur in software: cairo always, and GL when told to
/// use the software rasteriser.
fn software_renderer(window: &impl IsA<gtk::Native>) -> bool {
    let cairo = window
        .renderer()
        .is_some_and(|r| r.type_().name() == "GskCairoRenderer");
    cairo
        || std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_some_and(|v| v != "0")
        || std::env::var("GSK_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("cairo"))
}

fn sync(window: &gtk::Widget) {
    let animate = animations_enabled();
    let native = window.downcast_ref::<gtk::Window>();
    let blur = animate
        && !SLOW.with(Cell::get)
        && native.is_some_and(|w| w.is_realized() && !software_renderer(w));
    set_class(window, REDUCED_MOTION, !animate);
    set_class(window, BLUR_OK, blur);
}

fn set_class(widget: &gtk::Widget, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

/// Watch the main window: its renderer once realised, the animation setting,
/// and its dialogs. `shell` is the widget blurred behind a dialog.
pub(crate) fn install(window: &adw::ApplicationWindow, shell: &impl IsA<gtk::Widget>) {
    shell.add_css_class("app-shell");
    let widget: gtk::Widget = window.clone().upcast();
    sync(&widget);
    window.connect_realize(|w| sync(w.upcast_ref()));
    window.connect_map(|w| sync(w.upcast_ref()));
    if let Some(settings) = gtk::Settings::default() {
        let weak = window.downgrade();
        settings.connect_gtk_enable_animations_notify(move |_| {
            if let Some(window) = weak.upgrade() {
                sync(window.upcast_ref());
            }
        });
    }
    window.connect_visible_dialog_notify(|w| {
        set_class(w.upcast_ref(), DIALOG_OPEN, w.visible_dialog().is_some());
    });
}

/// The same for a secondary window (Accounts), which takes the main window's
/// verdict on blur rather than measuring again.
pub(crate) fn install_secondary(window: &adw::Window, main: &adw::ApplicationWindow) {
    window.add_css_class("ferestre-dialog-host");
    if let Some(content) = window.content() {
        content.add_css_class("app-shell");
    }
    let copy = |window: &adw::Window, main: &adw::ApplicationWindow| {
        set_class(window.upcast_ref(), BLUR_OK, main.has_css_class(BLUR_OK));
        set_class(
            window.upcast_ref(),
            REDUCED_MOTION,
            main.has_css_class(REDUCED_MOTION),
        );
    };
    copy(window, main);
    let weak = main.downgrade();
    window.connect_visible_dialog_notify(move |w| {
        if let Some(main) = weak.upgrade() {
            copy(w, &main);
        }
        set_class(w.upcast_ref(), DIALOG_OPEN, w.visible_dialog().is_some());
    });
}

/// Mark a freshly built page root to animate in, when it replaced a different
/// section (a redraw of the same page must not flash). The first few
/// transitions with blur also time their frames.
pub(crate) fn enter(page: &impl IsA<gtk::Widget>, window: &adw::ApplicationWindow) {
    // Software rendering at 32:9 draws a full-window fade at a few frames
    // per second: a choppy fade is worse than none.
    if (software_renderer(window) || SLOW.with(Cell::get)) && window.width() > 2560 {
        return;
    }
    page.add_css_class(PAGE_ENTER);
    if !window.has_css_class(BLUR_OK)
        || MEASURING.with(Cell::get)
        || MEASURED.with(Cell::get) >= MEASURE_TRANSITIONS
    {
        return;
    }
    MEASURING.with(|m| m.set(true));
    let last = Cell::new(0i64);
    let seen = Cell::new(0i64);
    let total = Cell::new(0i64);
    let longest = Cell::new(0i64);
    // On the window, not the page: a redraw replaces the page, and a callback
    // on it would go with it before it had decided anything.
    window.add_tick_callback(move |window, clock| {
        let now = clock.frame_time();
        let previous = last.replace(now);
        if previous == 0 {
            return glib::ControlFlow::Continue;
        }
        seen.set(seen.get() + 1);
        if seen.get() <= SKIP_FRAMES {
            return glib::ControlFlow::Continue;
        }
        let interval = now - previous;
        total.set(total.get() + interval);
        longest.set(longest.get().max(interval));
        let sampled = seen.get() - SKIP_FRAMES;
        if sampled < SAMPLE_FRAMES && longest.get() < HOPELESS_FRAME {
            return glib::ControlFlow::Continue;
        }
        MEASURING.with(|m| m.set(false));
        MEASURED.with(|m| m.set(m.get() + 1));
        let verdict = slow_verdict(
            total.get() / sampled,
            longest.get(),
            SLOW_SAMPLES.with(Cell::get),
        );
        if verdict.is_some() {
            SLOW_SAMPLES.with(|s| s.set(s.get() + 1));
        }
        if verdict == Some(true) {
            SLOW.with(|s| s.set(true));
            let weak = window.downgrade();
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(AFTER_ANIMATION_MS),
                move || {
                    if let Some(window) = weak.upgrade() {
                        sync(window.upcast_ref());
                    }
                },
            );
        }
        glib::ControlFlow::Break
    });
}

/// For one timed transition: `None` when it was fast, `Some(false)` when it
/// was slow but blur gets another chance, `Some(true)` when blur should go.
fn slow_verdict(average: i64, longest: i64, slow_before: u32) -> Option<bool> {
    if longest >= HOPELESS_FRAME {
        return Some(true);
    }
    (average > SLOW_FRAME).then_some(slow_before + 1 >= SLOW_TRANSITIONS)
}

/// Duration for a widget's own transition (a revealer or a stack): none when
/// the desktop asked for reduced motion.
pub(crate) fn duration(ms: u32) -> u32 {
    if animations_enabled() {
        ms
    } else {
        0
    }
}

/// How long the light takes over a tile (the CSS has the same), and how soon
/// after being left a tile is the same one still: a redraw replaces it.
const SHIMMER_MS: u64 = 850;
const SAME_TILE: std::time::Duration = std::time::Duration::from_millis(500);

thread_local! {
    /// The tile selected last, and when it was left.
    static SELECTED: RefCell<Option<(String, Option<std::time::Instant>)>> =
        const { RefCell::new(None) };
}

/// Whether selecting `name` is news, for the last one selected.
fn newly_selected(last: &Option<(String, Option<std::time::Instant>)>, name: &str) -> bool {
    match last {
        Some((last, left)) if last == name => left.is_some_and(|at| at.elapsed() >= SAME_TILE),
        _ => true,
    }
}

/// A band of light over `surface` when `button` is selected: by the pointer,
/// the keyboard or a controller.
pub(crate) fn shimmer(button: &impl IsA<gtk::Widget>, surface: &gtk::Overlay) {
    let light = gtk::Box::builder()
        .can_target(false)
        .can_focus(false)
        .css_classes(["tile-shimmer"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    surface.add_overlay(&light);
    let button = button.upcast_ref::<gtk::Widget>();
    let select = {
        let (button, light) = (button.downgrade(), light.downgrade());
        move || {
            let (Some(button), Some(light)) = (button.upgrade(), light.upgrade()) else {
                return;
            };
            let name = button.widget_name().to_string();
            let news = SELECTED.with(|s| newly_selected(&s.borrow(), &name));
            SELECTED.with(|s| s.replace(Some((name, None))));
            if !news || !animations_enabled() || light.has_css_class("run") {
                return;
            }
            light.add_css_class("run");
            let light = light.downgrade();
            let length = std::time::Duration::from_millis(SHIMMER_MS + 50);
            glib::timeout_add_local_once(length, move || {
                if let Some(light) = light.upgrade() {
                    light.remove_css_class("run");
                }
            });
        }
    };
    let leave = {
        let button = button.downgrade();
        move || {
            let Some(button) = button.upgrade() else {
                return;
            };
            let name = button.widget_name();
            SELECTED.with(|s| {
                if let Some((last, left)) = s.borrow_mut().as_mut() {
                    if *last == name {
                        *left = Some(std::time::Instant::now());
                    }
                }
            });
        }
    };
    let focus = gtk::EventControllerFocus::new();
    let on = select.clone();
    focus.connect_enter(move |_| on());
    let off = leave.clone();
    focus.connect_leave(move |_| off());
    button.add_controller(focus);
    let pointer = gtk::EventControllerMotion::new();
    pointer.connect_enter(move |_, _, _| select());
    let off = leave.clone();
    pointer.connect_leave(move |_| off());
    button.add_controller(pointer);
    button.connect_unmap(move |_| leave());
}

/// Frosted glass on surfaces drawn inside the window, where GTK can do it:
/// `backdrop-filter` arrived in GTK 4.22, and an older GTK would only print a
/// parse warning for it, so this is a second stylesheet loaded on 4.22 and
/// later, and it applies only where blur is affordable (`blur-ok`).
pub(crate) fn load_glass(display: &gdk::Display) {
    if gtk::check_version(4, 22, 0).is_some() {
        return;
    }
    let css = gtk::CssProvider::new();
    css.load_from_data(GLASS);
    gtk::style_context_add_provider_for_display(
        display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
}

const GLASS: &str = "
window.blur-ok .notification, window.blur-ok .notification-more {
  background-color: alpha(#24272d, 0.56);
  backdrop-filter: blur(22px) saturate(1.35);
  box-shadow: 0 8px 28px alpha(black, 0.45), inset 0 0 0 1px alpha(white, 0.10);
}
window.blur-ok floating-sheet > sheet, window.blur-ok bottom-sheet > sheet {
  background-color: alpha(#22252b, 0.58);
  backdrop-filter: blur(26px) saturate(1.4);
  box-shadow: 0 18px 60px alpha(black, 0.55), inset 0 0 0 1px alpha(white, 0.11);
}
/* What stands on the glass must not paint over it. */
window.blur-ok floating-sheet > sheet toolbarview, window.blur-ok floating-sheet > sheet scrolledwindow,
window.blur-ok floating-sheet > sheet preferencespage, window.blur-ok floating-sheet > sheet headerbar {
  background-color: transparent;
}
window.blur-ok.dialog-open floating-sheet > dimming, window.blur-ok.dialog-open bottom-sheet > dimming {
  background-color: alpha(black, 0.30);
}
window.blur-ok .search-pill {
  background-color: alpha(#2a2d33, 0.7);
  backdrop-filter: blur(14px);
}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_slow_transition_is_not_enough() {
        assert_eq!(slow_verdict(16_700, 20_000, 0), None);
        assert_eq!(slow_verdict(30_000, 40_000, 0), Some(false));
        assert_eq!(slow_verdict(30_000, 40_000, 1), Some(true));
    }

    #[test]
    fn a_software_frame_decides_at_once() {
        assert_eq!(slow_verdict(16_700, HOPELESS_FRAME, 0), Some(true));
        assert_eq!(slow_verdict(900_000, 900_000, 0), Some(true));
    }

    #[test]
    fn a_redrawn_tile_is_not_a_new_selection() {
        let now = std::time::Instant::now();
        assert!(newly_selected(&None, "a"));
        assert!(!newly_selected(&Some(("a".into(), None)), "a"));
        assert!(!newly_selected(&Some(("a".into(), Some(now))), "a"));
        assert!(newly_selected(
            &Some(("a".into(), Some(now - 2 * SAME_TILE))),
            "a"
        ));
        assert!(newly_selected(&Some(("a".into(), None)), "b"));
    }
}

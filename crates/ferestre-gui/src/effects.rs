//! The page-switch transition and the blur behind dialogs, and when not to
//! draw them.
//!
//! Both are CSS (see `STYLE`): a short `page-enter` animation on a page that
//! replaced another, and a `filter: blur()` on the window's content while an
//! `adw::Dialog` is up. The window carries the classes that pick between them:
//!
//! - `blur-ok`: blur is affordable. A blurred frame costs about 1-3 ms on a
//!   GPU even at 32:9, but 15-800 ms on cairo or software GL, so it is set
//!   only on a hardware renderer, and taken away for the session if the first
//!   transition's frames come in slow (llvmpipe reports itself as GL).
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
    /// Set once frames during a transition were too slow for blur: kept for
    /// the rest of the session, so it does not flicker on and off.
    static SLOW: Cell<bool> = const { Cell::new(false) };
    /// Whether the frame-time check has run.
    static MEASURED: Cell<bool> = const { Cell::new(false) };
}

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
/// section (a redraw of the same page must not flash). The first transition
/// with blur also times its frames.
pub(crate) fn enter(page: &impl IsA<gtk::Widget>, window: &adw::ApplicationWindow) {
    page.add_css_class(PAGE_ENTER);
    if !window.has_css_class(BLUR_OK) || MEASURED.with(Cell::get) {
        return;
    }
    MEASURED.with(|m| m.set(true));
    let last = Cell::new(0i64);
    let frames = Cell::new(0i64);
    let total = Cell::new(0i64);
    let weak = window.downgrade();
    page.add_tick_callback(move |_, clock| {
        let now = clock.frame_time();
        if last.get() != 0 {
            total.set(total.get() + now - last.get());
            frames.set(frames.get() + 1);
        }
        last.set(now);
        if frames.get() < 6 {
            return glib::ControlFlow::Continue;
        }
        if total.get() / frames.get() > 25_000 {
            SLOW.with(|s| s.set(true));
            if let Some(window) = weak.upgrade() {
                sync(window.upcast_ref());
            }
        }
        glib::ControlFlow::Break
    });
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
  background-color: alpha(#24272d, 0.68);
  backdrop-filter: blur(18px);
}
window.blur-ok .hero-caption {
  background-image: none;
  background-color: alpha(#15171b, 0.45);
  backdrop-filter: blur(16px);
}
window.blur-ok .search-pill {
  background-color: alpha(#2a2d33, 0.7);
  backdrop-filter: blur(14px);
}
";

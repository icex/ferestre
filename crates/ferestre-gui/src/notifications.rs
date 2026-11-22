//! Notifications that stack instead of queueing.
//!
//! `adw::ToastOverlay` shows one toast at a time and queues the rest: three
//! results arriving together took fifteen seconds to read through, a High
//! toast pushed the one on screen back into the queue to reappear after the
//! fact ("Starting X" after X had already stopped), and a queued toast was
//! invisible to a screen reader and to the tests until its turn came.
//!
//! [`Toasts`] takes the same `adw::Toast` objects, as plain data, and shows
//! them all at once in a column at the bottom of the content, newest at the
//! bottom, in the content's bottom-right corner. As many as fit in about 60% of the content's height are shown;
//! older ordinary ones fold into a "+N more" button (High ones never fold).
//! Each has its own timer, which stops while the pointer or the keyboard is
//! on the column and while it is folded away. The same text again restarts
//! the one already shown and counts it ("×2") rather than adding a row.
//!
//! The column takes no keyboard or controller focus until asked (F6 from the
//! navigation, see controller.rs), so moving around the window never falls
//! into it.
use super::*;

/// Rows shown at most, however tall the window.
const MOST: usize = 6;
/// The share of the content's height the column may cover.
const SHARE: f64 = 0.6;
/// A row's height and the gap between rows, for the capacity estimate.
const ROW: f64 = 56.0;
const GAP: f64 = 6.0;
/// How many past notifications the history keeps.
const HISTORY: usize = 50;
const TICK_MS: u64 = 200;

struct Entry {
    id: u64,
    title: String,
    /// A launch's toasts share a key, so "X stopped" replaces "Starting X".
    key: Option<String>,
    high: bool,
    /// Milliseconds left on screen; `None` stays until dismissed.
    remaining: Cell<Option<i64>>,
    timeout: Option<i64>,
    count: Cell<u32>,
    count_label: gtk::Label,
    revealer: gtk::Revealer,
    toast: adw::Toast,
}

struct Inner {
    overlay: gtk::Overlay,
    clamp: adw::Clamp,
    scroller: gtk::ScrolledWindow,
    rows: gtk::Box,
    more: gtk::Button,
    dismiss_all: gtk::Button,
    entries: RefCell<Vec<Entry>>,
    next_id: Cell<u64>,
    capacity: Cell<usize>,
    height: Cell<i32>,
    expanded: Cell<bool>,
    hovered: Cell<bool>,
    ticking: Cell<bool>,
    history: RefCell<std::collections::VecDeque<String>>,
    unread: Cell<usize>,
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
}

#[derive(Clone)]
pub(crate) struct Toasts(Rc<Inner>);

impl Toasts {
    pub(crate) fn new() -> Self {
        let rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(GAP as i32)
            .valign(gtk::Align::End)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .child(&rows)
            .build();
        let more = gtk::Button::builder()
            .halign(gtk::Align::Center)
            .css_classes(["notification-more"])
            .visible(false)
            .build();
        let dismiss_all = gtk::Button::builder()
            .label("Dismiss all")
            .halign(gtk::Align::Center)
            .css_classes(["notification-more"])
            .visible(false)
            .build();
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::End)
            .build();
        header.append(&more);
        header.append(&dismiss_all);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(GAP as i32)
            .css_classes(["notification-stack"])
            .build();
        column.append(&header);
        column.append(&scroller);
        // One width for every card, so the column has a straight edge.
        column.set_size_request(320, -1);
        // Bottom right, out of the way of the first columns of the grid and
        // of the page's own controls, and as wide as a sentence needs.
        let clamp = adw::Clamp::builder()
            .maximum_size(440)
            .tightening_threshold(440)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_start(12)
            .margin_end(16)
            .margin_bottom(16)
            .child(&column)
            .visible(false)
            .build();
        // Keyboard and controller focus only when asked for (F6).
        clamp.set_can_focus(false);
        let overlay = gtk::Overlay::new();
        // Measures the content area, so the capacity follows the window.
        let gauge = gtk::DrawingArea::builder()
            .can_target(false)
            .can_focus(false)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        overlay.add_overlay(&gauge);
        overlay.add_overlay(&clamp);
        let inner = Rc::new(Inner {
            overlay,
            clamp: clamp.clone(),
            scroller,
            rows,
            more: more.clone(),
            dismiss_all: dismiss_all.clone(),
            entries: RefCell::new(Vec::new()),
            next_id: Cell::new(1),
            capacity: Cell::new(4),
            height: Cell::new(0),
            expanded: Cell::new(false),
            hovered: Cell::new(false),
            ticking: Cell::new(false),
            history: RefCell::new(Default::default()),
            unread: Cell::new(0),
            listeners: RefCell::new(Vec::new()),
        });
        let toasts = Toasts(inner);
        let weak = Rc::downgrade(&toasts.0);
        gauge.connect_resize(move |_, _, height| {
            let Some(inner) = weak.upgrade() else { return };
            if inner.height.replace(height) == height {
                return;
            }
            let weak = Rc::downgrade(&inner);
            // Not during the allocation that reported it.
            glib::idle_add_local_once(move || {
                if let Some(inner) = weak.upgrade() {
                    Toasts(inner).relayout();
                }
            });
        });
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&toasts.0);
        motion.connect_enter(move |_, _, _| {
            if let Some(inner) = weak.upgrade() {
                inner.hovered.set(true);
            }
        });
        let weak = Rc::downgrade(&toasts.0);
        motion.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.hovered.set(false);
            }
        });
        clamp.add_controller(motion);
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(|controller| {
            // Focus left the column: it is out of the focus chain again.
            if let Some(widget) = controller.widget() {
                widget.set_can_focus(false);
            }
        });
        clamp.add_controller(focus);
        let weak = Rc::downgrade(&toasts.0);
        more.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.expanded.set(!inner.expanded.get());
                Toasts(inner).relayout();
            }
        });
        let weak = Rc::downgrade(&toasts.0);
        dismiss_all.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Toasts(inner).dismiss_all();
            }
        });
        toasts
    }

    pub(crate) fn set_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        self.0.overlay.set_child(child);
    }

    pub(crate) fn widget(&self) -> &gtk::Overlay {
        &self.0.overlay
    }

    /// Show a toast. Takes the same object `adw::ToastOverlay` would.
    pub(crate) fn add_toast(&self, toast: adw::Toast) {
        self.add(toast, None);
    }

    /// Show a toast that replaces any earlier one with the same key, such as a
    /// launch's "Starting X" when "X stopped" arrives.
    pub(crate) fn add_toast_replacing(&self, key: &str, toast: adw::Toast) {
        self.add(toast, Some(key.to_ascii_uppercase()));
    }

    fn add(&self, toast: adw::Toast, key: Option<String>) {
        let inner = &self.0;
        let title = toast.title().map(|t| t.to_string()).unwrap_or_default();
        if title.is_empty() {
            return;
        }
        let high = toast.priority() == adw::ToastPriority::High;
        let timeout = match toast.timeout() {
            0 => None,
            seconds => Some(i64::from(seconds) * 1000),
        };
        self.remember(&title);
        if let Some(key) = &key {
            let stale: Vec<u64> = inner
                .entries
                .borrow()
                .iter()
                .filter(|e| e.key.as_ref() == Some(key))
                .map(|e| e.id)
                .collect();
            for id in stale {
                self.remove(id);
            }
        }
        // The same words again: count them on the row already shown.
        if let Some(entry) = inner.entries.borrow().iter().find(|e| e.title == title) {
            entry.count.set(entry.count.get() + 1);
            entry
                .count_label
                .set_label(&format!("×{}", entry.count.get()));
            entry.count_label.set_visible(true);
            entry.remaining.set(timeout.or(entry.timeout));
            announce(entry.revealer.upcast_ref(), &title, high);
            return;
        }
        let id = inner.next_id.get();
        inner.next_id.set(id + 1);
        let (revealer, count_label) = self.row(id, &toast, &title, high);
        inner.rows.append(&revealer);
        inner.entries.borrow_mut().push(Entry {
            id,
            title: title.clone(),
            key,
            high,
            remaining: Cell::new(timeout),
            timeout,
            count: Cell::new(1),
            count_label,
            revealer: revealer.clone(),
            toast,
        });
        self.relayout();
        revealer.set_transition_duration(effects::duration(180));
        revealer.set_reveal_child(true);
        announce(revealer.upcast_ref(), &title, high);
        self.tick();
    }

    fn row(
        &self,
        id: u64,
        toast: &adw::Toast,
        title: &str,
        high: bool,
    ) -> (gtk::Revealer, gtk::Label) {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .css_classes(["notification"])
            .accessible_role(gtk::AccessibleRole::Alert)
            .build();
        if high {
            card.add_css_class("high");
        }
        card.update_property(&[gtk::accessible::Property::Label(title)]);
        let label = gtk::Label::builder()
            .label(title)
            .use_markup(toast.uses_markup())
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        card.append(&label);
        let count = gtk::Label::builder()
            .css_classes(["notification-count"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        card.append(&count);
        if let Some(text) = toast.button_label().filter(|t| !t.is_empty()) {
            let button = gtk::Button::builder()
                .label(text.as_str())
                .valign(gtk::Align::Center)
                .build();
            let weak = Rc::downgrade(&self.0);
            let source = toast.clone();
            button.connect_clicked(move |button| {
                if let Some(name) = source.action_name() {
                    let target = source.action_target_value();
                    let _ = button.activate_action(&name, target.as_ref());
                }
                source.emit_by_name::<()>("button-clicked", &[]);
                if let Some(inner) = weak.upgrade() {
                    Toasts(inner).remove(id);
                }
            });
            card.append(&button);
        }
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Dismiss notification")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        close.update_property(&[gtk::accessible::Property::Label("Dismiss notification")]);
        let weak = Rc::downgrade(&self.0);
        close.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let toasts = Toasts(inner);
                let had_focus = toasts.has_focus();
                toasts.remove(id);
                if had_focus {
                    toasts.focus_newest();
                }
            }
        });
        card.append(&close);
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(0)
            .child(&card)
            .build();
        (revealer, count)
    }

    fn remember(&self, title: &str) {
        let mut history = self.0.history.borrow_mut();
        history.push_front(title.to_string());
        history.truncate(HISTORY);
        drop(history);
        self.0.unread.set((self.0.unread.get() + 1).min(HISTORY));
        for listener in self.0.listeners.borrow().iter() {
            listener();
        }
    }

    /// Called whenever the history or unread count changes.
    pub(crate) fn connect_changed(&self, f: impl Fn() + 'static) {
        self.0.listeners.borrow_mut().push(Box::new(f));
    }

    /// Notifications so far this session, newest first.
    pub(crate) fn history(&self) -> Vec<String> {
        self.0.history.borrow().iter().cloned().collect()
    }

    pub(crate) fn unread(&self) -> usize {
        self.0.unread.get()
    }

    pub(crate) fn mark_read(&self) {
        if self.0.unread.replace(0) != 0 {
            for listener in self.0.listeners.borrow().iter() {
                listener();
            }
        }
    }

    pub(crate) fn clear_history(&self) {
        self.0.history.borrow_mut().clear();
        self.mark_read();
        for listener in self.0.listeners.borrow().iter() {
            listener();
        }
    }

    fn remove(&self, id: u64) {
        let entry = {
            let mut entries = self.0.entries.borrow_mut();
            let Some(at) = entries.iter().position(|e| e.id == id) else {
                return;
            };
            entries.remove(at)
        };
        self.0.rows.remove(&entry.revealer);
        entry.toast.emit_by_name::<()>("dismissed", &[]);
        self.relayout();
    }

    pub(crate) fn dismiss_all(&self) {
        let ids: Vec<u64> = self.0.entries.borrow().iter().map(|e| e.id).collect();
        for id in ids {
            self.remove(id);
        }
        self.0.expanded.set(false);
        self.relayout();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.entries.borrow().is_empty()
    }

    /// Whether the keyboard focus is on a notification.
    pub(crate) fn has_focus(&self) -> bool {
        self.0
            .clamp
            .root()
            .and_then(|root| root.focus())
            .is_some_and(|focus| focus.is_ancestor(&self.0.clamp))
    }

    /// Put the focus on the newest notification's first button (F6).
    pub(crate) fn focus_newest(&self) -> bool {
        let Some(revealer) = self.0.entries.borrow().last().map(|e| e.revealer.clone()) else {
            return false;
        };
        self.0.clamp.set_can_focus(true);
        let card = revealer.child();
        let button = card
            .and_then(|card| card.last_child())
            .filter(|w| w.is::<gtk::Button>());
        match button {
            Some(button) => button.grab_focus(),
            None => {
                self.0.clamp.set_can_focus(false);
                false
            }
        }
    }

    /// Dismiss the notification that holds the focus (controller Back).
    pub(crate) fn dismiss_focused(&self) -> bool {
        let Some(focus) = self.0.clamp.root().and_then(|r| r.focus()) else {
            return false;
        };
        let id = self
            .0
            .entries
            .borrow()
            .iter()
            .find(|e| focus.is_ancestor(&e.revealer))
            .map(|e| e.id);
        match id {
            Some(id) => {
                self.remove(id);
                self.0.clamp.set_can_focus(false);
                true
            }
            None => false,
        }
    }

    /// Which rows show, given the room there is; "+N more" for the rest.
    fn relayout(&self) {
        let inner = &self.0;
        let height = f64::from(inner.height.get().max(0));
        let room = ((height * SHARE - 40.0) / (ROW + GAP)).floor();
        let capacity = if room.is_finite() && room > 0.0 {
            (room as usize).clamp(1, MOST)
        } else {
            1
        };
        inner.capacity.set(capacity);
        inner
            .scroller
            .set_max_content_height((height * SHARE) as i32);
        let entries = inner.entries.borrow();
        let expanded = inner.expanded.get();
        let highs: Vec<bool> = entries.iter().map(|e| e.high).collect();
        let shown = fold(&highs, capacity, expanded);
        let mut hidden = Vec::new();
        for (e, show) in entries.iter().zip(&shown) {
            e.revealer.set_visible(*show);
            if !show {
                hidden.push(e.title.clone());
            }
        }
        let folded = hidden.len();
        inner.more.set_visible(folded > 0 || expanded);
        inner.dismiss_all.set_visible(expanded || folded > 0);
        if expanded {
            inner.more.set_label("Show fewer");
            inner
                .more
                .update_property(&[gtk::accessible::Property::Description("")]);
        } else {
            inner.more.set_label(&format!("+{folded} more"));
            // A folded notification stays readable to assistive technology.
            inner
                .more
                .update_property(&[gtk::accessible::Property::Description(&hidden.join(" · "))]);
        }
        let empty = entries.is_empty();
        drop(entries);
        if empty {
            inner.expanded.set(false);
            inner.clamp.set_can_focus(false);
        }
        inner.clamp.set_visible(!empty);
    }

    /// Count the visible timers down while nothing holds them.
    fn tick(&self) {
        if self.0.ticking.replace(true) {
            return;
        }
        let weak = Rc::downgrade(&self.0);
        let mut last = std::time::Instant::now();
        glib::timeout_add_local(std::time::Duration::from_millis(TICK_MS), move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(last).as_millis() as i64;
            last = now;
            let toasts = Toasts(inner);
            if toasts.0.entries.borrow().is_empty() {
                toasts.0.ticking.set(false);
                return glib::ControlFlow::Break;
            }
            let held = toasts.0.hovered.get() || toasts.has_focus();
            if held {
                return glib::ControlFlow::Continue;
            }
            let mut expired = Vec::new();
            for e in toasts.0.entries.borrow().iter() {
                if !e.revealer.is_visible() {
                    continue;
                }
                if let Some(left) = e.remaining.get() {
                    let left = left - elapsed;
                    e.remaining.set(Some(left));
                    if left <= 0 {
                        expired.push(e.id);
                    }
                }
            }
            for id in expired {
                toasts.remove(id);
            }
            glib::ControlFlow::Continue
        });
    }
}

/// Which notifications show, oldest first, given whether each is High: every
/// High one, the newest ordinary one, then ordinary ones newest first while
/// there is room. Expanded shows everything.
fn fold(highs: &[bool], capacity: usize, expanded: bool) -> Vec<bool> {
    let mut shown: Vec<bool> = highs.to_vec();
    let mut used = highs.iter().filter(|h| **h).count();
    let mut newest = true;
    for (i, high) in highs.iter().enumerate().rev() {
        if *high {
            continue;
        }
        if expanded || newest || used < capacity {
            shown[i] = true;
            used += 1;
        }
        newest = false;
    }
    shown
}

/// Have a screen reader read a new notification out. `gtk_accessible_announce`
/// is GTK 4.14; the crate targets 4.10, so it is looked up at run time and
/// skipped where it does not exist.
fn announce(widget: &gtk::Widget, text: &str, high: bool) {
    type Announce = unsafe extern "C" fn(*mut c_void, *const c_char, c_int);
    use std::ffi::{c_char, c_int, c_void, CString};
    thread_local! {
        static FUNCTION: Option<Announce> = {
            if gtk::check_version(4, 14, 0).is_some() {
                None
            } else {
                // SAFETY: looks a symbol up in the already loaded GTK.
                let symbol = unsafe {
                    libc::dlsym(libc::RTLD_DEFAULT, c"gtk_accessible_announce".as_ptr())
                };
                // SAFETY: GTK's own declaration of this function.
                (!symbol.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, Announce>(symbol) })
            }
        };
    }
    let Some(function) = FUNCTION.with(|f| *f) else {
        return;
    };
    let Ok(text) = CString::new(text) else { return };
    use glib::translate::ToGlibPtr;
    let pointer: *mut gtk::ffi::GtkWidget = widget.to_glib_none().0;
    // GTK_ACCESSIBLE_ANNOUNCEMENT_PRIORITY_MEDIUM = 1, HIGH = 2.
    // SAFETY: a live widget (every GtkWidget is a GtkAccessible) and a
    // NUL-terminated string that outlives the call.
    unsafe { function(pointer.cast(), text.as_ptr(), if high { 2 } else { 1 }) };
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn everything_shows_when_there_is_room() {
        assert_eq!(fold(&[false, false, false], 6, false), [true, true, true]);
    }

    #[test]
    fn the_oldest_ordinary_ones_fold_first() {
        assert_eq!(
            fold(&[false, false, false, false], 2, false),
            [false, false, true, true]
        );
    }

    #[test]
    fn high_ones_never_fold_and_the_newest_ordinary_one_always_shows() {
        assert_eq!(
            fold(&[true, false, true, false, false], 2, false),
            [true, false, true, false, true]
        );
        assert_eq!(
            fold(&[true, true, true, false], 1, false),
            [true, true, true, true]
        );
    }

    #[test]
    fn expanded_shows_all() {
        assert_eq!(fold(&[false, true, false, false], 1, true), [true; 4]);
    }
}

//! Read mapped SDL controllers without grabbing devices or injecting global input.
//! SDL is optional and loaded at runtime, as it already is for Wine controllers.
use super::*;
use std::{
    ffi::{c_char, c_int, c_void},
    time::{Duration, Instant},
};

type Handle = *mut c_void;
type Init = unsafe extern "C" fn(u32) -> c_int;
type Quit = unsafe extern "C" fn(u32);
type Count = unsafe extern "C" fn() -> c_int;
type IsController = unsafe extern "C" fn(c_int) -> c_int;
type Open = unsafe extern "C" fn(c_int) -> Handle;
type Close = unsafe extern "C" fn(Handle);
type Update = unsafe extern "C" fn();
type Poll = unsafe extern "C" fn(*mut c_void) -> c_int;
type Attached = unsafe extern "C" fn(Handle) -> c_int;
type Button = unsafe extern "C" fn(Handle, c_int) -> u8;
type Axis = unsafe extern "C" fn(Handle, c_int) -> i16;
type Kind = unsafe extern "C" fn(Handle) -> c_int;
type Hint = unsafe extern "C" fn(*const c_char, *const c_char) -> c_int;
const GAMECONTROLLER: u32 = 0x00002000;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) enum Family {
    #[default]
    Xbox,
    PlayStation,
    DualSense,
    PlayStation3,
}
impl Family {
    fn from_sdl_type(kind: i32) -> Self {
        match kind {
            3 => Self::PlayStation3,
            4 => Self::PlayStation,
            7 => Self::DualSense,
            _ => Self::Xbox,
        }
    }
    fn controls(
        self,
    ) -> (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ) {
        match self {
            Self::Xbox => ("Ⓐ", "Ⓑ", "Ⓨ", "View ▣", "Menu ☰", "LB / RB"),
            Self::PlayStation => ("✕", "○", "△", "Share", "Options ☰", "L1 / R1"),
            Self::DualSense => ("✕", "○", "△", "Create", "Options ☰", "L1 / R1"),
            Self::PlayStation3 => ("✕", "○", "△", "Select", "Start", "L1 / R1"),
        }
    }
}

fn navigation_focus(ui: &Ui) -> bool {
    GtkWindowExt::focus(&ui.window).is_some_and(|focus| {
        focus.is_ancestor(&ui.sidebar)
            || focus.is_ancestor(&ui.recent_games)
            || focus.widget_name().starts_with("navigation-")
    })
}
fn game_name(focus: &gtk::Widget) -> Option<String> {
    let name = focus.widget_name();
    let id = name
        .strip_prefix("game-")
        .or_else(|| name.strip_prefix("feature-"))?;
    id.strip_suffix("-primary")
        .or_else(|| id.strip_suffix("-more"))
        .map(str::to_owned)
}
fn focused_game(ui: &Ui) -> Option<String> {
    let mut focus = GtkWindowExt::focus(&ui.window);
    while let Some(widget) = focus {
        if let Some(id) = game_name(&widget) {
            return Some(id);
        }
        focus = widget.parent();
    }
    None
}
fn game_buttons(ui: &Ui) -> Vec<gtk::Widget> {
    fn collect(root: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
        let name = root.widget_name();
        if name.starts_with("game-") && name.ends_with("-primary") && root.is_sensitive() {
            out.push(root.clone());
            return;
        }
        let mut child = root.first_child();
        while let Some(widget) = child {
            collect(&widget, out);
            child = widget.next_sibling();
        }
    }
    let mut out = vec![];
    collect(ui.content.upcast_ref(), &mut out);
    out
}
fn scroll_position(current: f64, page: f64, lower: f64, upper: f64, top: f64, height: f64) -> f64 {
    let start = top - 12.0;
    let end = top + height + 12.0;
    let wanted = if height + 24.0 > page || start < current {
        start
    } else if end > current + page {
        end - page
    } else {
        current
    };
    wanted.max(lower).min((upper - page).max(lower))
}
fn reveal_focus(ui: &Ui) -> bool {
    let Some(focus) = GtkWindowExt::focus(&ui.window).filter(|w| w.is_ancestor(&ui.content)) else {
        return true;
    };
    let Some(bounds) = focus.compute_bounds(&ui.content) else {
        return false;
    };
    let adjustment = ui.scroll.vadjustment();
    let page = adjustment.page_size();
    if page <= 0.0 || bounds.height() <= 0.0 {
        return false;
    }
    adjustment.set_value(scroll_position(
        adjustment.value(),
        page,
        adjustment.lower(),
        adjustment.upper(),
        f64::from(bounds.y()),
        f64::from(bounds.height()),
    ));
    true
}
fn reveal_on_frame(ui: &Ui) {
    let ui = ui.clone();
    let window = ui.window.clone();
    let frames = Cell::new(0u8);
    window.add_tick_callback(move |_, _| {
        frames.set(frames.get() + 1);
        // Resolve after the first layout of a newly selected section.
        if frames.get() < 2 {
            return glib::ControlFlow::Continue;
        }
        if reveal_focus(&ui) || frames.get() >= 4 {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
pub(super) fn focus_content(ui: &Ui) {
    ui.window.set_focus_visible(true);
    ui.split.set_show_content(true);
    let saved = ui.last_game_focus.borrow().clone();
    let button = saved
        .and_then(|name| named_widget(ui.content.upcast_ref(), &name))
        .filter(|w| w.is_sensitive())
        .or_else(|| game_buttons(ui).into_iter().next());
    if let Some(button) = button {
        button.grab_focus();
    } else {
        ui.content.child_focus(gtk::DirectionType::TabForward);
    }
    ui.window.set_focus_visible(true);
    ui.window.add_css_class("controller-navigation");
    reveal_focus(ui);
    reveal_on_frame(ui);
    update_hints(ui);
}
fn navigation_buttons(ui: &Ui) -> Vec<gtk::Widget> {
    let mut items: Vec<gtk::Widget> = vec![ui.account_button.clone().upcast()];
    for index in 0..Section::NAV.len() {
        if let Some(row) = ui.sidebar.row_at_index(index as i32) {
            items.push(row.upcast());
        }
    }
    let mut recent = ui.recent_games.first_child();
    while let Some(widget) = recent {
        if widget.widget_name().starts_with("recent-") {
            items.push(widget.clone());
        }
        recent = widget.next_sibling();
    }
    for name in [
        "navigation-downloads",
        "navigation-settings",
        "navigation-fullscreen",
        "navigation-help",
    ] {
        if let Some(button) = named_widget(ui.window.upcast_ref(), name) {
            items.push(button);
        }
    }
    items
}
fn focus_navigation(ui: &Ui) {
    if ui.split.is_collapsed() {
        ui.split.set_show_content(false);
    }
    if let Some(row) = ui
        .sidebar
        .selected_row()
        .or_else(|| ui.sidebar.row_at_index(0))
    {
        row.grab_focus();
    }
    ui.window.set_focus_visible(true);
    ui.window.add_css_class("controller-navigation");
    update_hints(ui);
}
fn activate_navigation(ui: &Ui) {
    let Some(focus) = GtkWindowExt::focus(&ui.window) else {
        return;
    };
    let enter_games = focus.is_ancestor(&ui.sidebar)
        || matches!(
            focus.widget_name().as_str(),
            "navigation-downloads" | "navigation-settings"
        );
    focus.activate();
    if enter_games {
        let ui = ui.clone();
        glib::idle_add_local_once(move || focus_content(&ui));
    }
}
fn move_navigation(ui: &Ui, down: bool) {
    let buttons = navigation_buttons(ui);
    if buttons.is_empty() {
        return;
    }
    let focus = GtkWindowExt::focus(&ui.window);
    let index = buttons
        .iter()
        .position(|w| focus.as_ref() == Some(w))
        .unwrap_or(1);
    let next = if down {
        (index + 1).min(buttons.len() - 1)
    } else {
        index.saturating_sub(1)
    };
    buttons[next].grab_focus();
}
#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}
fn spatial_target(rects: &[Rect], current: usize, action: Action) -> Option<usize> {
    let from = rects.get(current)?;
    let center = |r: &Rect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
    let score = |to: &Rect| -> Option<(u8, f32, f32)> {
        let (a, b) = center(from);
        let (x, y) = center(to);
        let (gap, overlap, cross) = match action {
            Action::Left if to.x + to.width <= from.x + 1.0 => (
                from.x - (to.x + to.width),
                to.y < from.y + from.height && to.y + to.height > from.y,
                (y - b).abs(),
            ),
            Action::Right if to.x >= from.x + from.width - 1.0 => (
                to.x - (from.x + from.width),
                to.y < from.y + from.height && to.y + to.height > from.y,
                (y - b).abs(),
            ),
            Action::Up if to.y + to.height <= from.y + 1.0 => (
                from.y - (to.y + to.height),
                to.x < from.x + from.width && to.x + to.width > from.x,
                (x - a).abs(),
            ),
            Action::Down if to.y >= from.y + from.height - 1.0 => (
                to.y - (from.y + from.height),
                to.x < from.x + from.width && to.x + to.width > from.x,
                (x - a).abs(),
            ),
            _ => return None,
        };
        Some((u8::from(!overlap), gap.max(0.0), cross))
    };
    rects
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != current)
        .filter_map(|(i, r)| score(r).map(|score| (i, score)))
        .min_by(|a, b| {
            a.1 .0
                .cmp(&b.1 .0)
                .then(a.1 .1.total_cmp(&b.1 .1))
                .then(a.1 .2.total_cmp(&b.1 .2))
        })
        .map(|(i, _)| i)
}
fn main_controls(ui: &Ui) -> Vec<gtk::Widget> {
    fn collect(ui: &Ui, root: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
        if !root.is_visible()
            || !root.is_mapped()
            || !root.is_sensitive()
            || root.is_ancestor(&ui.sidebar)
            || root.is_ancestor(&ui.recent_games)
            || root.widget_name().starts_with("navigation-")
            || root.is::<gtk::WindowControls>()
        {
            return;
        }
        if root.is::<gtk::SearchEntry>()
            || root.is::<gtk::Entry>()
            || root.is::<gtk::DropDown>()
            || root.is::<gtk::Button>()
            || root.is::<gtk::Switch>()
        {
            out.push(root.clone());
            return;
        }
        let mut child = root.first_child();
        while let Some(widget) = child {
            collect(ui, &widget, out);
            child = widget.next_sibling();
        }
    }
    let mut controls = vec![];
    collect(ui, ui.window.upcast_ref(), &mut controls);
    controls
}
fn move_main(ui: &Ui, action: Action) -> bool {
    let Some(focus) = GtkWindowExt::focus(&ui.window) else {
        focus_content(ui);
        return true;
    };
    let buttons = main_controls(ui);
    let Some(index) = buttons
        .iter()
        .position(|w| *w == focus || focus.is_ancestor(w))
    else {
        return false;
    };
    let bounds: Option<Vec<Rect>> = buttons
        .iter()
        .map(|w| {
            w.compute_bounds(&ui.window).map(|r| Rect {
                x: r.x(),
                y: r.y(),
                width: r.width(),
                height: r.height(),
            })
        })
        .collect();
    let Some(bounds) = bounds else { return false };
    let next = if focus.is_ancestor(&ui.content) {
        let indices: Vec<usize> = buttons
            .iter()
            .enumerate()
            .filter(|(_, w)| w.is_ancestor(&ui.content))
            .map(|(i, _)| i)
            .collect();
        let local: Vec<Rect> = indices.iter().map(|i| bounds[*i]).collect();
        let current = indices.iter().position(|i| *i == index).unwrap();
        spatial_target(&local, current, action)
            .map(|i| indices[i])
            .or_else(|| {
                if matches!(action, Action::Up | Action::Down) {
                    spatial_target(&bounds, index, action)
                } else {
                    None
                }
            })
    } else {
        spatial_target(&bounds, index, action)
    };
    if let Some(next) = next {
        buttons[next].grab_focus();
    } else if action == Action::Left {
        focus_navigation(ui);
    }
    true
}
pub(super) fn update_hints(ui: &Ui) {
    if let Some(focus) = GtkWindowExt::focus(&ui.window) {
        if focus.widget_name().starts_with("game-") && focus.widget_name().ends_with("-primary") {
            *ui.last_game_focus.borrow_mut() = Some(focus.widget_name().to_string());
        }
    }
    let family = ui.controller_family.get();
    let (a, b, y, view, menu, shoulders) = family.controls();
    let badge = |glyph: &str, color: &str| {
        format!("<span foreground='{color}' weight='bold'>{glyph}</span>")
    };
    let a = badge(
        a,
        if family != Family::Xbox {
            "#8ebdff"
        } else {
            "#8bd77b"
        },
    );
    let b = badge(b, "#ff929e");
    let y = badge(
        y,
        if family != Family::Xbox {
            "#8bd7bd"
        } else {
            "#f2d16b"
        },
    );
    let action = focused_game(ui)
        .and_then(|id| ui.game_actions.borrow().get(&id).cloned())
        .unwrap_or_else(|| "Select".into());
    let home = if family == Family::Xbox {
        "Xbox / Home"
    } else {
        "PS"
    };
    let markup = if ui.window.visible_dialog().is_some() || focused_popup(ui).is_some() {
        format!("D-pad  Choose   ·   {a} Select   ·   {b} Back / Close")
    } else if navigation_focus(ui) {
        format!("↑ / ↓  Choose section   ·   →  Games   ·   {a} Open   ·   {b} Return to games   ·   {view} Navigation")
    } else {
        format!("D-pad / Left stick  Move   ·   {a} {}   ·   {b} Back   ·   {y} Search   ·   {view} Navigation   ·   {menu} More   ·   {shoulders} Sections", glib::markup_escape_text(&action))
    };
    let markup = format!("{markup}   ·   {home} Ferestre");
    if ui.controller_hints.label().as_str() != markup {
        ui.controller_hints.set_markup(&markup);
    }
    ui.navigation_hint.set_markup(&format!(
        "↑ / ↓ Choose   ·   → Games
{a} Open   ·   {b} Return"
    ));
    ui.navigation_hint.set_visible(
        ui.controller_connected.get()
            || ui.window.is_fullscreen()
            || ui.window.has_css_class("controller-navigation"),
    );
    ui.controller_hints.set_visible(
        ui.controller_connected.get()
            || ui.window.is_fullscreen()
            || ui.window.has_css_class("controller-navigation"),
    );
}
pub(super) fn show_help(ui: &Ui) {
    let (a, b, y, view, menu, shoulders) = ui.controller_family.get().controls();
    let home = if ui.controller_family.get() == Family::Xbox {
        "Xbox / Home"
    } else {
        "PS"
    };
    let text = format!("{home}: bring Ferestre to the front while a game is running.\n\nD-pad or left stick: move between games.\n\n{a}: select or launch the highlighted game.\n{b}: go back or close a menu.\n{y}: search.\n{view}: open or close navigation.\n{menu}: options for the highlighted game.\n{shoulders}: change sections.\n\nIn navigation, ↑/↓ chooses a section; → or {a} opens its games. ← from the first game returns to navigation.\n\nFullscreen is available in navigation and the window header (keyboard F11).");
    let dialog = adw::AlertDialog::new(Some("Controller controls"), Some(&text));
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    dialog.present(Some(&ui.window));
}

struct Pad {
    library: Handle,
    pads: Vec<(i32, Handle)>,
    instance: IsController,
    pump: Update,
    poll: Poll,
    quit: Quit,
    count: Count,
    is_controller: IsController,
    open: Open,
    close: Close,
    update: Update,
    attached: Attached,
    button: Button,
    axis: Axis,
    kind: Option<Kind>,
}
impl Pad {
    fn load() -> Option<Self> {
        // The soname is fixed; no arbitrary plugin/library path is accepted.
        unsafe {
            let library = libc::dlopen(
                c"libSDL2-2.0.so.0".as_ptr(),
                libc::RTLD_NOW | libc::RTLD_LOCAL,
            );
            if library.is_null() {
                return None;
            }
            let loaded = (|| {
                let init: Init = symbol(library, c"SDL_InitSubSystem")?;
                let quit: Quit = symbol(library, c"SDL_QuitSubSystem")?;
                let count = symbol(library, c"SDL_NumJoysticks")?;
                let is_controller = symbol(library, c"SDL_IsGameController")?;
                let open = symbol(library, c"SDL_GameControllerOpen")?;
                let close = symbol(library, c"SDL_GameControllerClose")?;
                let update = symbol(library, c"SDL_GameControllerUpdate")?;
                let instance = symbol(library, c"SDL_JoystickGetDeviceInstanceID")?;
                let pump = symbol(library, c"SDL_PumpEvents")?;
                let poll = symbol(library, c"SDL_PollEvent")?;
                let attached = symbol(library, c"SDL_GameControllerGetAttached")?;
                let button = symbol(library, c"SDL_GameControllerGetButton")?;
                let axis = symbol(library, c"SDL_GameControllerGetAxis")?;
                let hint: Hint = symbol(library, c"SDL_SetHint")?;
                // GTK owns our window. SDL has no window of its own with
                // which to identify focus; dispatch below checks GTK focus.
                hint(c"SDL_NO_SIGNAL_HANDLERS".as_ptr(), c"1".as_ptr());
                hint(
                    c"SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS".as_ptr(),
                    c"1".as_ptr(),
                );
                if init(GAMECONTROLLER) != 0 {
                    return None;
                }
                Some(Self {
                    library,
                    pads: Vec::new(),
                    instance,
                    pump,
                    poll,
                    quit,
                    count,
                    is_controller,
                    open,
                    close,
                    update,
                    attached,
                    button,
                    axis,
                    kind: symbol(library, c"SDL_GameControllerGetType"),
                })
            })();
            if loaded.is_none() {
                libc::dlclose(library);
            }
            loaded
        }
    }
    fn sample(&mut self) -> Vec<Sample> {
        // GTK owns the main loop. SDL still needs its own event pump for
        // hotplug discovery; updating button state alone does not discover pads.
        unsafe {
            (self.pump)();
            // SDL2's event union is 56 bytes on our 64-bit target. Extra room
            // and u64 alignment also cover the compatibility library's ABI.
            let mut event = [0u64; 8];
            for _ in 0..256 {
                if (self.poll)(event.as_mut_ptr().cast()) == 0 {
                    break;
                }
            }
            (self.update)();
            self.pads.retain(|(_, handle)| {
                if (self.attached)(*handle) != 0 {
                    true
                } else {
                    (self.close)(*handle);
                    false
                }
            });
            for index in 0..(self.count)().clamp(0, 64) {
                let id = (self.instance)(index);
                if id < 0
                    || self.pads.iter().any(|(present, _)| *present == id)
                    || (self.is_controller)(index) == 0
                {
                    continue;
                }
                let handle = (self.open)(index);
                if !handle.is_null() {
                    self.pads.push((id, handle));
                }
            }
            self.pads
                .iter()
                .map(|(id, handle)| {
                    let mut buttons = 0u16;
                    for button in [0, 1, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14] {
                        if (self.button)(*handle, button) != 0 {
                            buttons |= 1 << button;
                        }
                    }
                    Sample {
                        id: *id,
                        family: Family::from_sdl_type(
                            self.kind.map(|kind| kind(*handle)).unwrap_or(0),
                        ),
                        input: Input {
                            buttons,
                            x: (self.axis)(*handle, 0),
                            y: (self.axis)(*handle, 1),
                        },
                    }
                })
                .collect()
        }
    }
}
impl Drop for Pad {
    fn drop(&mut self) {
        unsafe {
            for (_, handle) in self.pads.drain(..) {
                (self.close)(handle);
            }
            (self.quit)(GAMECONTROLLER);
            libc::dlclose(self.library);
        }
    }
}

unsafe fn symbol<T: Copy>(library: Handle, name: &std::ffi::CStr) -> Option<T> {
    let pointer = unsafe { libc::dlsym(library, name.as_ptr()) };
    if pointer.is_null() || std::mem::size_of::<T>() != std::mem::size_of::<Handle>() {
        return None;
    }
    // All callers specify a function pointer matching the named SDL declaration.
    Some(unsafe { std::mem::transmute_copy(&pointer) })
}
#[derive(Clone, Copy)]
struct Sample {
    id: i32,
    family: Family,
    input: Input,
}

#[derive(Default)]
struct Controllers(BTreeMap<i32, (HomeButton, Mapper)>);
impl Controllers {
    fn sample(&mut self, samples: &[Sample]) -> Option<Family> {
        self.0.retain(|id, _| samples.iter().any(|s| s.id == *id));
        let mut home = None;
        for sample in samples {
            let (button, _) = self.0.entry(sample.id).or_insert_with(|| {
                let mut mapper = Mapper::default();
                mapper.suppress(sample.input);
                (HomeButton::default(), mapper)
            });
            if button.pressed(true, sample.input) {
                home = Some(sample.family);
            }
        }
        home
    }
    fn suppress(&mut self, samples: &[Sample]) {
        for sample in samples {
            if let Some((_, mapper)) = self.0.get_mut(&sample.id) {
                mapper.suppress(sample.input);
            }
        }
    }
    fn actions(&mut self, samples: &[Sample], now: Instant) -> Vec<(Family, Action)> {
        samples
            .iter()
            .flat_map(|sample| {
                self.0
                    .get_mut(&sample.id)
                    .unwrap()
                    .1
                    .actions(sample.input, now)
                    .into_iter()
                    .map(|action| (sample.family, action))
            })
            .collect()
    }
}

#[derive(Clone, Copy, Default)]
struct Input {
    buttons: u16,
    x: i16,
    y: i16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Up,
    Down,
    Left,
    Right,
    Accept,
    Back,
    Search,
    Previous,
    Next,
    Fullscreen,
    Navigation,
    Options,
}
/// A newly attached or already-held Guide button must be released first.
#[derive(Default)]
struct HomeButton {
    connected: bool,
    held: bool,
}
impl HomeButton {
    fn pressed(&mut self, connected: bool, input: Input) -> bool {
        let held = input.buttons & (1 << 5) != 0;
        let pressed = connected && self.connected && held && !self.held;
        self.connected = connected;
        self.held = held;
        pressed
    }
}
#[derive(Default)]
struct Mapper {
    previous: u16,
    direction: Option<Action>,
    repeat: Option<Instant>,
}
impl Mapper {
    fn actions(&mut self, input: Input, now: Instant) -> Vec<Action> {
        let held = |button: u32| (input.buttons & (1u16 << button)) != 0u16;
        let mut result = Vec::new();
        for (button, action) in [
            (0, Action::Accept),
            (1, Action::Back),
            (3, Action::Search),
            (9, Action::Previous),
            (10, Action::Next),
            (4, Action::Navigation),
            (6, Action::Options),
        ] {
            if held(button) && self.previous & (1u16 << button) == 0u16 {
                result.push(action);
            }
        }
        let direction = if held(11) || input.y < -18000 {
            Some(Action::Up)
        } else if held(12) || input.y > 18000 {
            Some(Action::Down)
        } else if held(13) || input.x < -18000 {
            Some(Action::Left)
        } else if held(14) || input.x > 18000 {
            Some(Action::Right)
        } else {
            None
        };
        if direction != self.direction {
            if let Some(direction) = direction {
                result.push(direction);
            }
            self.repeat = direction.map(|_| now + Duration::from_millis(400));
        } else if self.repeat.is_some_and(|at| now >= at) {
            if let Some(direction) = direction {
                result.push(direction);
                self.repeat = Some(now + Duration::from_millis(120));
            }
        }
        self.previous = input.buttons;
        self.direction = direction;
        result
    }
    fn suppress(&mut self, input: Input) {
        self.previous = input.buttons;
        self.direction = None;
        self.repeat = None;
    }
}
fn focused_popup(ui: &Ui) -> Option<gtk::Popover> {
    let mut node = GtkWindowExt::focus(&ui.window);
    while let Some(widget) = node {
        if let Ok(popover) = widget.clone().downcast::<gtk::Popover>() {
            return Some(popover);
        }
        node = widget.parent();
    }
    None
}
fn active_app_window(ui: &Ui) -> Option<gtk::Window> {
    ui.window
        .application()?
        .windows()
        .into_iter()
        .find(|w| w.is_active())
}
fn dispatch(ui: &Ui, action: Action) {
    ui.window.set_focus_visible(true);
    ui.window.add_css_class("controller-navigation");
    if let Some(active) =
        active_app_window(ui).filter(|w| *w != ui.window.clone().upcast::<gtk::Window>())
    {
        active.set_focus_visible(true);
        match action {
            Action::Up | Action::Down | Action::Left | Action::Right => {
                active.child_focus(match action {
                    Action::Up => gtk::DirectionType::Up,
                    Action::Down => gtk::DirectionType::Down,
                    Action::Left => gtk::DirectionType::Left,
                    _ => gtk::DirectionType::Right,
                });
            }
            Action::Accept => {
                if let Some(focus) = GtkWindowExt::focus(&active) {
                    focus.activate();
                }
            }
            Action::Back => active.close(),
            _ => (),
        }
        return;
    }
    let dialog = ui.window.visible_dialog();
    match action {
        Action::Up | Action::Down | Action::Left | Action::Right => {
            let direction = match action {
                Action::Up => gtk::DirectionType::Up,
                Action::Down => gtk::DirectionType::Down,
                Action::Left => gtk::DirectionType::Left,
                _ => gtk::DirectionType::Right,
            };
            if let Some(popover) = focused_popup(ui) {
                popover.child_focus(direction);
            } else if let Some(dialog) = dialog {
                dialog.child_focus(direction);
            } else if navigation_focus(ui) {
                match action {
                    Action::Right => {
                        let focus = GtkWindowExt::focus(&ui.window);
                        if focus.as_ref().is_some_and(|w| {
                            w.is_ancestor(&ui.sidebar)
                                || matches!(
                                    w.widget_name().as_str(),
                                    "navigation-downloads" | "navigation-settings"
                                )
                        }) {
                            activate_navigation(ui);
                        } else {
                            focus_content(ui);
                        }
                    }
                    Action::Up | Action::Down => move_navigation(ui, action == Action::Down),
                    _ => (),
                }
            } else if !move_main(ui, action)
                && !ui.window.child_focus(direction)
                && action == Action::Left
            {
                focus_navigation(ui);
            }
        }
        Action::Accept if dialog.is_none() && navigation_focus(ui) => activate_navigation(ui),
        Action::Accept => {
            if let Some(focus) = GtkWindowExt::focus(&ui.window) {
                let enter = focus.widget_name().starts_with("section-")
                    || focus.widget_name().starts_with("fullscreen-nav-");
                focus.activate();
                if enter {
                    let ui = ui.clone();
                    glib::idle_add_local_once(move || focus_content(&ui));
                }
            } else {
                focus_content(ui);
            }
        }
        Action::Back => {
            if let Some(dialog) = dialog {
                dialog.close();
                return;
            }
            let mut ancestor = GtkWindowExt::focus(&ui.window);
            while let Some(widget) = ancestor {
                if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
                    popover.popdown();
                    focus_content(ui);
                    return;
                }
                ancestor = widget.parent();
            }
            if navigation_focus(ui) {
                focus_content(ui);
            } else {
                focus_navigation(ui);
            }
        }
        Action::Navigation if dialog.is_none() => {
            if navigation_focus(ui) {
                focus_content(ui);
            } else {
                focus_navigation(ui);
            }
        }
        Action::Options if dialog.is_none() => {
            if let Some(id) = focused_game(ui) {
                if let Some(menu) =
                    named_widget(ui.content.upcast_ref(), &format!("game-{id}-more"))
                        .and_then(|w| w.downcast::<gtk::MenuButton>().ok())
                {
                    menu.popup();
                    if let Some(popover) = menu.popover() {
                        popover.child_focus(gtk::DirectionType::TabForward);
                    }
                }
            } else {
                show_help(ui);
            }
        }
        Action::Search if dialog.is_none() => {
            if !ui.search.is_visible() {
                select_section(ui, Section::Library);
            }
            ui.split.set_show_content(true);
            ui.search.grab_focus();
        }
        Action::Previous | Action::Next if dialog.is_none() => {
            let fullscreen_sections = handheld::SECTIONS.map(|(_, section)| section);
            let sections = if ui.window.is_fullscreen() {
                &fullscreen_sections[..]
            } else {
                &Section::NAV[..]
            };
            let current = sections
                .iter()
                .position(|s| *s == ui.model.borrow().section)
                .unwrap_or(0);
            let index = if action == Action::Next {
                (current + 1) % sections.len()
            } else {
                (current + sections.len() - 1) % sections.len()
            };
            select_section(ui, sections[index]);
            focus_content(ui);
        }
        Action::Fullscreen if dialog.is_none() => {
            if ui.window.is_fullscreen() {
                ui.window.unfullscreen();
            } else {
                ui.window.fullscreen();
            }
        }
        _ => (),
    }
    reveal_focus(ui);
    reveal_on_frame(ui);
    update_hints(ui);
}
pub(super) fn install(ui: &Ui) {
    let keys = gtk::EventControllerKey::new();
    let key_ui = ui.clone();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if key == gdk::Key::F11 {
            dispatch(&key_ui, Action::Fullscreen);
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::F6 {
            if GtkWindowExt::focus(&key_ui.window).is_some_and(|w| w.is_ancestor(&key_ui.content)) {
                focus_navigation(&key_ui);
            } else {
                focus_content(&key_ui);
            }
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Menu {
            dispatch(&key_ui, Action::Options);
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::Escape {
            dispatch(&key_ui, Action::Back);
            return glib::Propagation::Stop;
        }
        if key_ui.window.visible_dialog().is_none()
            && focused_popup(&key_ui).is_none()
            && GtkWindowExt::focus(&key_ui.window).is_none_or(|w| !w.is::<gtk::Editable>())
        {
            let action = match key {
                gdk::Key::Up => Some(Action::Up),
                gdk::Key::Down => Some(Action::Down),
                gdk::Key::Left => Some(Action::Left),
                gdk::Key::Right => Some(Action::Right),
                gdk::Key::Return | gdk::Key::space if navigation_focus(&key_ui) => {
                    Some(Action::Accept)
                }
                _ => None,
            };
            if let Some(action) = action {
                dispatch(&key_ui, action);
                return glib::Propagation::Stop;
            }
        }
        if modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::f {
            dispatch(&key_ui, Action::Search);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    ui.window.add_controller(keys);
    let mode_ui = ui.clone();
    ui.window
        .connect_focus_widget_notify(move |_| update_hints(&mode_ui));
    let pointer = gtk::GestureClick::new();
    pointer.set_button(0);
    pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pointer_window = ui.window.clone();
    pointer.connect_pressed(move |_, _, _, _| {
        pointer_window.remove_css_class("controller-navigation")
    });
    ui.window.add_controller(pointer);
    let Some(mut pad) = Pad::load() else {
        return;
    };
    let ui = ui.clone();
    let mut controllers = Controllers::default();
    let mut was_active = false;
    let mut focus_pending = false;
    glib::timeout_add_local(Duration::from_millis(25), move || {
        if !ui.window.is_visible() {
            return glib::ControlFlow::Break;
        }
        let was_connected = ui.controller_connected.get();
        let samples = pad.sample();
        let connected = !samples.is_empty();
        ui.controller_connected.set(connected);
        if samples.len() == 1 || connected && !was_connected {
            ui.controller_family.set(samples[0].family);
        }
        update_hints(&ui);
        if let Some(family) = controllers.sample(&samples) {
            ui.controller_family.set(family);
            window_activation::present(&ui);
            controllers.suppress(&samples);
            was_active = false;
            focus_pending = true;
            return glib::ControlFlow::Continue;
        }
        if connected && !was_connected {
            focus_pending = true;
        }
        if focus_pending && ui.window.is_active() {
            focus_content(&ui);
            focus_pending = false;
        }
        if active_app_window(&ui).is_none() {
            controllers.suppress(&samples);
            was_active = false;
            return glib::ControlFlow::Continue;
        }
        if !was_active {
            controllers.suppress(&samples);
            was_active = true;
            return glib::ControlFlow::Continue;
        }
        for (family, action) in controllers.actions(&samples, Instant::now()) {
            ui.controller_family.set(family);
            dispatch(&ui, action);
        }
        glib::ControlFlow::Continue
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_controls_are_revealed_without_moving_an_already_visible_item() {
        assert_eq!(scroll_position(100., 500., 0., 2000., 200., 200.), 100.);
        assert_eq!(scroll_position(100., 500., 0., 2000., 650., 200.), 362.);
        assert_eq!(scroll_position(100., 500., 0., 2000., 20., 100.), 8.);
        assert_eq!(scroll_position(0., 280., 0., 2000., 100., 400.), 88.);
        assert_eq!(
            scroll_position(88., 280., 0., 2000., 100., 400.),
            88.,
            "oversized cards must not oscillate"
        );
        assert_eq!(scroll_position(0., 500., 0., 600., 550., 100.), 100.);
    }
    #[test]
    fn playstation_models_use_their_own_button_labels() {
        for kind in [3, 4, 7] {
            let family = Family::from_sdl_type(kind);
            assert_ne!(family, Family::Xbox);
            assert_eq!(family.controls().0, "✕");
            assert_eq!(family.controls().1, "○");
            assert_eq!(family.controls().2, "△");
            assert_eq!(family.controls().5, "L1 / R1");
        }
        assert_eq!(Family::from_sdl_type(7).controls().3, "Create");
        assert_eq!(Family::from_sdl_type(4).controls().3, "Share");
        assert_eq!(Family::from_sdl_type(3).controls().3, "Select");
        for kind in [0, 1, 2, 6, 999, -1] {
            assert_eq!(Family::from_sdl_type(kind), Family::Xbox);
        }
    }
    #[test]
    fn view_opens_navigation_and_menu_opens_options() {
        let now = Instant::now();
        for (button, expected) in [(4, Action::Navigation), (6, Action::Options)] {
            let mut mapper = Mapper::default();
            let input = Input {
                buttons: 1 << button,
                ..Default::default()
            };
            assert_eq!(mapper.actions(input, now), vec![expected]);
            assert!(mapper
                .actions(input, now + Duration::from_secs(1))
                .is_empty());
        }
    }
    #[test]
    fn home_works_in_background_once_per_press_and_not_on_attachment() {
        let mut home = HomeButton::default();
        let held = Input {
            buttons: 1 << 5,
            ..Default::default()
        };
        assert!(!home.pressed(true, held));
        assert!(!home.pressed(true, held));
        assert!(!home.pressed(true, Input::default()));
        assert!(home.pressed(true, held));
        assert!(!home.pressed(true, held));
        assert!(!home.pressed(false, Input::default()));
        assert!(!home.pressed(true, held));
        assert!(!home.pressed(true, Input::default()));
        assert!(home.pressed(true, held));
        let mut mapper = Mapper::default();
        assert!(
            mapper.actions(held, Instant::now()).is_empty(),
            "Home must not also navigate or activate a game"
        );
    }
    #[test]
    fn every_controller_has_independent_home_and_navigation_edges() {
        let now = Instant::now();
        let mut pads = Controllers::default();
        let xbox = Sample {
            id: 1,
            family: Family::Xbox,
            input: Input::default(),
        };
        let mut ps = Sample {
            id: 2,
            family: Family::DualSense,
            input: Input::default(),
        };
        assert_eq!(pads.sample(&[xbox, ps]), None);
        ps.input.buttons = 1 << 5;
        assert_eq!(pads.sample(&[xbox, ps]), Some(Family::DualSense));
        assert_eq!(pads.sample(&[xbox, ps]), None);
        ps.input.buttons = 0;
        pads.sample(&[xbox, ps]);
        ps.input.buttons = 1;
        assert_eq!(
            pads.actions(&[xbox, ps], now),
            vec![(Family::DualSense, Action::Accept)]
        );
        assert!(pads.actions(&[xbox, ps], now).is_empty());
        pads.sample(&[xbox]);
        assert_eq!(pads.sample(&[xbox, ps]), None);
        assert!(
            pads.actions(&[xbox, ps], now).is_empty(),
            "held input on reconnect is suppressed"
        );
    }
    #[test]
    fn spatial_navigation_visits_tabs_cards_and_pagination() {
        let rectangles = [
            Rect {
                x: 0.,
                y: 0.,
                width: 600.,
                height: 44.,
            },
            Rect {
                x: 0.,
                y: 60.,
                width: 120.,
                height: 48.,
            },
            Rect {
                x: 140.,
                y: 60.,
                width: 120.,
                height: 48.,
            },
            Rect {
                x: 0.,
                y: 140.,
                width: 196.,
                height: 378.,
            },
            Rect {
                x: 214.,
                y: 140.,
                width: 196.,
                height: 378.,
            },
            Rect {
                x: 0.,
                y: 600.,
                width: 88.,
                height: 48.,
            },
        ];
        assert_eq!(spatial_target(&rectangles, 3, Action::Up), Some(1));
        assert_eq!(spatial_target(&rectangles, 1, Action::Up), Some(0));
        assert_eq!(spatial_target(&rectangles, 3, Action::Right), Some(4));
        assert_eq!(spatial_target(&rectangles, 4, Action::Left), Some(3));
        assert_eq!(spatial_target(&rectangles, 3, Action::Down), Some(5));
        assert_eq!(spatial_target(&rectangles, 3, Action::Left), None);
        assert_eq!(spatial_target(&rectangles, usize::MAX, Action::Down), None);
    }
    #[test]
    fn overlapping_card_options_do_not_steal_horizontal_game_navigation() {
        let rectangles = [
            Rect {
                x: 0.,
                y: 100.,
                width: 196.,
                height: 378.,
            },
            Rect {
                x: 140.,
                y: 106.,
                width: 48.,
                height: 48.,
            },
            Rect {
                x: 220.,
                y: 100.,
                width: 196.,
                height: 378.,
            },
        ];
        assert_eq!(spatial_target(&rectangles, 0, Action::Right), Some(2));
    }
    #[test]
    fn stick_deadzone_edges_and_repeat_do_not_repeat_accept() {
        let mut mapper = Mapper::default();
        let now = Instant::now();
        assert!(mapper
            .actions(
                Input {
                    x: 15000,
                    ..Default::default()
                },
                now
            )
            .is_empty());
        let input = Input {
            buttons: 1,
            x: 30000,
            y: 0,
        };
        assert_eq!(
            mapper.actions(input, now),
            vec![Action::Accept, Action::Right]
        );
        assert!(mapper
            .actions(input, now + Duration::from_millis(399))
            .is_empty());
        assert_eq!(
            mapper.actions(input, now + Duration::from_millis(400)),
            vec![Action::Right]
        );
        mapper.suppress(input);
        assert!(!mapper.actions(input, now).contains(&Action::Accept));
    }
    #[test]
    fn mapped_virtual_sdl_controller_reaches_the_same_dispatch_input() {
        let Some(mut pad) = Pad::load() else {
            eprintln!("SDL2 unavailable; virtual-device check skipped");
            return;
        };
        unsafe {
            type Attach = unsafe extern "C" fn(c_int, c_int, c_int, c_int) -> c_int;
            type JoystickOpen = unsafe extern "C" fn(c_int) -> Handle;
            type SetButton = unsafe extern "C" fn(Handle, c_int, u8) -> c_int;
            type Detach = unsafe extern "C" fn(c_int) -> c_int;
            let attach: Attach =
                symbol(pad.library, c"SDL_JoystickAttachVirtual").expect("SDL virtual API");
            let open: JoystickOpen = symbol(pad.library, c"SDL_JoystickOpen").unwrap();
            let set: SetButton = symbol(pad.library, c"SDL_JoystickSetVirtualButton").unwrap();
            let close: Close = symbol(pad.library, c"SDL_JoystickClose").unwrap();
            let detach: Detach = symbol(pad.library, c"SDL_JoystickDetachVirtual").unwrap();
            let index = attach(1, 6, 15, 0);
            assert!(index >= 0);
            let joystick = open(index);
            assert!(!joystick.is_null());
            // Select this exact private virtual device, never a physical pad.
            let id = (pad.instance)(index);
            assert!(id >= 0);
            pad.sample();
            unsafe extern "C" fn ps5_type(_: Handle) -> c_int {
                7
            }
            pad.kind = Some(ps5_type);
            assert_eq!(
                pad.sample()
                    .into_iter()
                    .find(|s| s.id == id)
                    .unwrap()
                    .family,
                Family::DualSense
            );
            assert_eq!(set(joystick, 0, 1), 0);
            assert_eq!(set(joystick, 12, 1), 0);
            let input = pad.sample().into_iter().find(|s| s.id == id).unwrap().input;
            let actions = Mapper::default().actions(input, Instant::now());
            assert!(actions.contains(&Action::Accept));
            assert!(actions.contains(&Action::Down));
            close(joystick);
            assert_eq!(detach(index), 0);
        }
    }
}

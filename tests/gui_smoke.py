#!/usr/bin/env python3
"""Drive the window the way a person does, and check what it says.

The unit tests cover every decision the GUI makes, because that logic lives in
`model.rs` and imports no toolkit. What they cannot cover is whether any of it
is wired to a widget: a row that renders with the wrong label, a button that
calls nothing, a section that never draws. That is what this checks, and it
checks it by pressing the buttons rather than by comparing screenshots -- a
pixel diff fails on a font change and passes on a dead button, which is exactly
backwards.

It drives the real window through AT-SPI, the accessibility bus, so the thing
under test is the shipped binary with no test hooks compiled into it.

    tests/gui_smoke.py [--binary target/debug/ferestre-gui] [--keep]

Needs at-spi2-core and the Atspi GObject bindings. Exits non-zero on the first
failed expectation, and prints what it found so a failure is diagnosable
without a rerun.
"""

import argparse
import ctypes
import ctypes.util
import glob
import json
import os
import re
import shutil
import shlex
import http.server
import threading
import subprocess
import struct
import sys
import tempfile
import time
import tomllib
import warnings

import gi

# PyGObject warns that Atspi.Action.get_action_name is deprecated. The
# replacement is not in this version, and the noise buries real output.
warnings.filterwarnings("ignore", category=DeprecationWarning, module="gui_smoke")

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib  # noqa: E402

# Long enough for a redraw to land on a loaded machine, short enough that a
# hang is reported as a failure rather than as a hung test run.
TIMEOUT = 20.0
POLL = 0.2


class Failure(Exception):
    pass


def find_app(pid, deadline):
    """The application object *this* launcher registered, once it appears.

    By process id, not by name. A leftover window from an earlier run answers to
    the same name, and attaching to it produces failures that describe a process
    the test did not start -- which is a long way to walk before noticing.
    """
    while time.monotonic() < deadline:
        desktop = Atspi.get_desktop(0)
        for i in range(desktop.get_child_count()):
            child = desktop.get_child_at_index(i)
            if child is None:
                continue
            try:
                if child.get_process_id() == pid:
                    return child
            except Exception:
                continue
        time.sleep(POLL)
    raise Failure(f"no accessible application with pid {pid}; is AT-SPI running?")


# libadwaita nests hard: ToolbarView > ToastOverlay > ScrolledWindow > Box >
# PreferencesPage > its own scroller > Clamp > Box > PreferencesGroup > Box >
# ListBox > row > Box > Label is fourteen deep before any text appears.
TREE_DEPTH = 30


def walk(node, depth=0, limit=TREE_DEPTH):
    """Every node in the tree, breadth of the whole window.

    A vanished node is skipped rather than raised. The tree is live and a
    redraw destroys widgets while this is reading them, so asking a node that
    has just gone fails with "No such interface" -- which is the window working
    normally, not a failed expectation. Every caller here polls, so the next
    pass sees the tree that replaced it.
    """
    yield node, depth
    if depth >= limit:
        return
    try:
        count = node.get_child_count()
    except GLib.GError:
        return
    for i in range(count):
        try:
            child = node.get_child_at_index(i)
        except GLib.GError:
            continue
        if child is not None:
            yield from walk(child, depth + 1, limit)


def describe(node):
    try:
        role = node.get_role_name()
        name = node.get_name() or ""
    except GLib.GError:
        return "<gone>"
    return f"{role}:{name!r}"


def texts(root):
    """Every label and name in the tree, for substring assertions."""
    found = []
    for node, _ in walk(root):
        try:
            name = node.get_name()
        except GLib.GError:
            continue
        if name:
            found.append(name)
    return found


def wait_for(root, predicate, what, deadline):
    last = None
    while time.monotonic() < deadline:
        # A node can vanish between being listed and being asked about while
        # the list it sits in is rebuilt; that is "not yet", not a failure.
        try:
            last = predicate(root)
        except GLib.GError:
            last = None
        if last:
            return last
        time.sleep(POLL)
    raise Failure(f"timed out waiting for {what}")


# at-spi2 does not spell every role the same way across versions: a GtkButton
# comes back as "button" on one machine and "push button" on another, and a test
# that hard-codes either finds nothing on the other. It found nothing on CI for
# five commits, which is exactly as useful as having no test.
ROLE_ALIASES = {
    "button": ("button", "push button"),
    "push button": ("button", "push button"),
    # The other two pairs this test depends on. Widening is safe because every
    # lookup that uses them also matches on an exact name.
    "text": ("text", "entry"),
    "entry": ("text", "entry"),
    "dialog": ("dialog", "alert"),
    "alert": ("dialog", "alert"),
}


def role_matches(node_role, wanted):
    return node_role in ROLE_ALIASES.get(wanted, (wanted,))


def find_all(root, role=None, name=None, name_contains=None):
    """Matching nodes, each once.

    The tree reaches some objects by more than one path, so a raw walk reports
    one dialog as three and one button as two. `path` is the object's identity
    on the bus, which is the only thing that deduplicates correctly.
    """
    out = []
    seen = set()
    for node, _ in walk(root):
        try:
            if node.path in seen:
                continue
            seen.add(node.path)
            if role and not role_matches(node.get_role_name(), role):
                continue
            node_name = node.get_name() or ""
        except GLib.GError:
            continue
        if name is not None and node_name != name:
            continue
        if name_contains is not None and name_contains not in node_name:
            continue
        out.append(node)
    return out


def actionable(node, levels=6):
    """The nearest thing that can be clicked: this node, or an ancestor.

    A name in the tree usually belongs to a label, and a label does nothing.
    What responds is the row or button that contains it, so a search by text
    has to climb before it clicks.
    """
    at = node
    for _ in range(levels):
        if at is None:
            break
        action = at.get_action_iface()
        if action is not None and action.get_n_actions() > 0:
            return at, action
        at = at.get_parent()
    return None, None


def click(node):
    target, action = actionable(node)
    if action is None:
        raise Failure(f"nothing around {describe(node)} can be clicked")
    for i in range(action.get_n_actions()):
        if action.get_action_name(i) in ("click", "activate", "press"):
            action.do_action(i)
            return target
    action.do_action(0)
    return target


def window_origin(pid):
    """Where on the X screen the window this process mapped begins.

    GTK 4 reports every extent relative to its own window and answers zero for
    screen coordinates, so a pointer event cannot be aimed from AT-SPI alone.
    The X server knows where the window went; ask it, by the same process id
    the application was found by.
    """
    x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong] + [ctypes.c_void_p] * 4
    x11.XGetWindowProperty.argtypes = (
        [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_long, ctypes.c_long]
        + [ctypes.c_int, ctypes.c_ulong]
        + [ctypes.c_void_p] * 5
    )
    x11.XTranslateCoordinates.argtypes = (
        [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int]
        + [ctypes.c_void_p] * 3
    )
    x11.XFree.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]

    display = x11.XOpenDisplay(None)
    if not display:
        raise Failure("cannot open the X display to aim the pointer; is DISPLAY set?")
    try:
        root = x11.XDefaultRootWindow(display)
        wm_pid = x11.XInternAtom(display, b"_NET_WM_PID", 1)

        def owner(window):
            kind, fmt = ctypes.c_ulong(), ctypes.c_int()
            count, left = ctypes.c_ulong(), ctypes.c_ulong()
            data = ctypes.POINTER(ctypes.c_ulong)()
            if x11.XGetWindowProperty(
                display, window, wm_pid, 0, 1, 0, 0,
                ctypes.byref(kind), ctypes.byref(fmt), ctypes.byref(count),
                ctypes.byref(left), ctypes.byref(data),
            ) != 0 or not data:
                return None
            try:
                return data[0] if count.value else None
            finally:
                x11.XFree(data)

        def search(window):
            if owner(window) == pid:
                return window
            root_out, parent = ctypes.c_ulong(), ctypes.c_ulong()
            children = ctypes.POINTER(ctypes.c_ulong)()
            count = ctypes.c_uint()
            if not x11.XQueryTree(
                display, window, ctypes.byref(root_out), ctypes.byref(parent),
                ctypes.byref(children), ctypes.byref(count),
            ):
                return None
            try:
                for i in range(count.value):
                    found = search(children[i])
                    if found:
                        return found
            finally:
                if children:
                    x11.XFree(children)
            return None

        window = search(root) if wm_pid else None
        if window is None:
            raise Failure(f"no X window belongs to pid {pid}")
        x, y, child = ctypes.c_int(), ctypes.c_int(), ctypes.c_ulong()
        x11.XTranslateCoordinates(
            display, window, root, 0, 0, ctypes.byref(x), ctypes.byref(y), ctypes.byref(child)
        )
        return x.value, y.value
    finally:
        x11.XCloseDisplay(display)


def press(node, pid):
    """Click a widget with the pointer, the way a person does.

    Not the same as `click`. An accessibility action activates a button and
    leaves the keyboard focus where it was; a real click gives the button the
    focus first. The window has to survive that button being destroyed under
    its own focus, and only a pointer click puts it in that position.
    """
    origin_x, origin_y = window_origin(pid)
    box = node.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
    x = origin_x + box.x + box.width // 2
    y = origin_y + box.y + box.height // 2
    Atspi.generate_mouse_event(x, y, "abs")
    time.sleep(0.15)
    Atspi.generate_mouse_event(x, y, "b1c")


# A GtkLabel advertises clipboard and selection actions. They are real, and
# none of them is "press this". Treating them as a click is how a test reports
# success while nothing happened.
TEXT_ACTIONS = {
    "clipboard.copy",
    "clipboard.cut",
    "clipboard.paste",
    "selection.delete",
    "selection.select-all",
    "link.open",
    "link.copy",
    "menu.popup",
}


def press_actions(node):
    """The actions on this node that actually do something, if any."""
    action = node.get_action_iface()
    if action is None:
        return None, []
    names = [action.get_action_name(i) for i in range(action.get_n_actions())]
    useful = [n for n in names if n not in TEXT_ACTIONS]
    return action, useful


def click_named(root, name, what=None):
    """Click the first thing carrying this name that can actually be clicked."""
    for candidate in find_all(root, name=name):
        action, useful = press_actions(candidate)
        if useful:
            action.do_action(
                next(
                    (
                        i
                        for i in range(action.get_n_actions())
                        if action.get_action_name(i) == useful[0]
                    ),
                    0,
                )
            )
            return candidate
    dump(root, f"looking for {name!r}")
    raise Failure(f"nothing clickable called {what or name!r}")


def select_in_list(root, name, what=None):
    """Select a row by name through the Selection interface.

    A `GtkListBoxRow` exposes no action at all -- what it exposes is being
    selectable, on its parent list -- so a sidebar is driven by selecting,
    not by pressing.
    """
    for node, _ in walk(root):
        if node.get_role_name() != "list":
            continue
        selection = node.get_selection_iface()
        if selection is None:
            continue
        for index in range(node.get_child_count()):
            child = node.get_child_at_index(index)
            if child is not None and child.get_name() == name:
                selection.select_child(index)
                return child
    dump(root, f"looking for a selectable {name!r}")
    raise Failure(f"no selectable row called {what or name!r}")


def row_button(root, title, label):
    """The button on the row for `title` that reads `label`.

    Two things make this less obvious than it sounds. A row's action button
    takes its accessible name from the row rather than from its own text, so
    there is no button called "Install" to find -- what says Install is a label
    inside it. And the row is only identified by its title, so the search has to
    start there: the moment two titles are installable, "the Install button" is
    ambiguous.
    """
    for button in find_all(root, role="button", name=title):
        for node, _ in walk(button):
            if node.get_role_name() == "label" and (node.get_name() or "") == label:
                return button
    return None


MENU_PREFIX = "More actions for "


def row_menus(root):
    """Every row's "More actions" button, by the title it names."""
    return {
        (node.get_name() or "")[len(MENU_PREFIX):]: node
        for node in find_all(root, name_contains=MENU_PREFIX)
        if (node.get_name() or "").startswith(MENU_PREFIX)
    }


def menu_items(app, menu):
    """Open a row's menu and return {item name: button}; close_menu() shuts it.

    The secondary actions live in a popover, and a popover's contents are in
    the accessible tree only while it is open -- the reason there is no Edit
    button to count until this has run.
    """
    targets = [menu] + [
        node for node, _ in walk(menu) if node is not menu and node.get_role_name() == "toggle button"
    ]
    for target in targets:
        click(target)
        try:
            items = wait_for(
                app,
                lambda root: {
                    node.get_name(): node
                    for node in find_all(root, role="button")
                    if node.get_name() in MENU_ITEMS
                }
                or None,
                "the row's menu to open",
                time.monotonic() + 5,
            )
            return items
        except Failure:
            continue
    raise Failure("the row's menu would not open")


def close_menu(app):
    """Escape closes the open popover; its items then leave the tree."""
    for node in find_all(app, role="button"):
        if node.get_name() in MENU_ITEMS:
            break
    else:
        return
    Atspi.generate_keyboard_event(0xFF1B, None, Atspi.KeySynthType.SYM)
    wait_for(
        app,
        lambda root: not any(n.get_name() in MENU_ITEMS for n in find_all(root, role="button")) or None,
        "the row's menu to close",
        time.monotonic() + 5,
    )


MENU_ITEMS = {"Open Store page", "Choose runtime", "Add to Steam", "Edit launch settings", "Remove from disk"}


def row_action(app, frame, item, title=None):
    """Click `item` in the menu of `title`'s row, or of the first row that has it."""
    menus = row_menus(frame)
    for name, menu in ([(title, menus[title])] if title else menus.items()):
        items = menu_items(app, menu)
        if item in items:
            click(items[item])
            return name
        close_menu(app)
    raise Failure(f"no row offers {item!r}")


def check_game_row_layout(root, title, size, action):
    button = row_button(root, title, action)
    check(button is not None, f"{title} keeps its {action} action", root)
    bounds = lambda node: node.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
    viewport = button.get_parent()
    while viewport and viewport.get_role_name() != "scroll pane":
        viewport = viewport.get_parent()
    # Offscreen label coordinates differ between GTK versions. Measure an
    # actually visible row, using the same scrollbar path as pointer tests.
    for _ in range(3):
        control, view = bounds(button), bounds(viewport)
        center = control.y + control.height / 2
        if view.y <= center < view.y + view.height:
            break
        values = [node.get_value_iface() for node in find_all(root, role="scroll bar")]
        value = next(v for v in values if v and v.get_maximum_value() > 200)
        target = value.get_current_value() + center - (view.y + view.height / 2)
        value.set_current_value(max(0, min(target, value.get_maximum_value())))
        time.sleep(0.25)
        button = row_button(root, title, action)
    row = button.get_parent()
    while row and row.get_role_name() != "list item":
        row = row.get_parent()
    check(row is not None, "the action belongs to a game row", root)
    title_label = find_all(row, role="label", name=title)[0]
    details = find_all(row, role="label", name_contains=size)
    check(len(details) == 1 and details[0].get_name().count(size) == 1,
          "size appears once, in the details line", row)
    top, bottom, control, whole = bounds(title_label), bounds(details[0]), bounds(button), bounds(row)
    check(top.height > 0 and bottom.height > 0 and bottom.y >= top.y + top.height - 1,
          "details sit below the game title", row)
    button_center = control.y + control.height / 2
    title_center = top.y + top.height / 2
    details_center = bottom.y + bottom.height / 2
    # Font glyph extents/leading vary with GTK and fonts. The action must sit
    # between the two text lines, not align with the top or bottom of the row.
    check(title_center <= button_center <= details_center,
          f"right-hand actions are centered between both text lines (button={button_center}, title={title_center}, details={details_center}, row={whole.y}/{whole.height})", row)
    check(control.x >= top.x + top.width, "actions stay to the right of the text", row)
    check(any(bounds(image).width >= 40 and bounds(image).height >= 40
              for image in find_all(row, role="image")), "the game icon spans both text lines", row)


def dump(root, why):
    print(f"\n--- accessibility tree ({why}) ---", file=sys.stderr)
    for node, depth in walk(root):
        print("  " * depth + describe(node), file=sys.stderr)
    print("--- end ---\n", file=sys.stderr)


def check(condition, message, root=None):
    if not condition:
        if root is not None:
            dump(root, "at failure")
        raise Failure(message)
    print(f"  ok: {message}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/ferestre-gui")
    parser.add_argument(
        "--keep",
        action="store_true",
        help="keep the throwaway HOME, to look at what the run wrote",
    )
    args = parser.parse_args()

    binary = os.path.abspath(args.binary)
    if not os.path.isfile(binary):
        raise Failure(f"{binary} is not there; cargo build -p ferestre-gui")

    # GApplication is single-instance by design -- opening the launcher twice
    # should raise the window you already have, not give you a second one -- so
    # a window left open from a manual test makes the run under test exit at
    # once, having handed its arguments to that other process. The symptom is
    # "no accessible application with pid N", which describes none of that.
    owner = subprocess.run(
        ["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
         "--object-path", "/org/freedesktop/DBus",
         "--method", "org.freedesktop.DBus.NameHasOwner", "io.github.icex.ferestre"],
        capture_output=True, text=True,
    )
    if "true" in owner.stdout:
        raise Failure(
            "a ferestre-gui is already running and owns io.github.icex.ferestre on this "
            "session bus; close it first, or this run exits into it instead of starting"
        )

    # A throwaway HOME, so the test cannot read a real account's cached library
    # or write into one. It also means the run starts from the state a new user
    # has, which is the state most worth testing.
    home = tempfile.mkdtemp(prefix="ferestre-gui-smoke-")
    recipes_dir = os.path.join(home, "titles")
    shutil.copytree(os.path.join(os.getcwd(), "titles"), recipes_dir)
    # A fixed fixture keeps this warning check independent of live compatibility.
    with open(os.path.join(recipes_dir, "9ZZBROKEN001.toml"), "w") as f:
        f.write('''schema = 1
[title]
product-id = "9ZZBROKEN001"
name = "Smoke Test Broken Game"
slug = "smoke-broken-game"
[launch]
executable = "Game.exe"
[status]
state = "broken"
summary = "Synthetic failure fixture"
stops-at = "Synthetic startup failure"
blocked-by = "unknown"
last-verified = 2026-10-10
''')
    env = dict(os.environ)
    env.update(
        {
            "HOME": home,
            "XDG_CACHE_HOME": os.path.join(home, "cache"),
            "XDG_STATE_HOME": os.path.join(home, "state"),
            "XDG_CONFIG_HOME": os.path.join(home, "config"),
            "GTK_A11Y": "atspi",
            # Startup now refreshes the public catalog. Keep this fixture
            # offline: a refused local proxy exercises cached fallback and
            # prevents real Microsoft data from replacing its seeded rows.
            "HTTPS_PROXY": "http://127.0.0.1:9",
            "https_proxy": "http://127.0.0.1:9",
            "HTTP_PROXY": "http://127.0.0.1:9",
            "http_proxy": "http://127.0.0.1:9",
            "ALL_PROXY": "http://127.0.0.1:9",
            "all_proxy": "http://127.0.0.1:9",
            "NO_PROXY": "",
            "no_proxy": "",
            "FERESTRE_TITLES": recipes_dir,
            # Recipes come from the checkout, but nothing else should: no Steam,
            # no runtime, no client.
            "XODUS_GAMES_DIR": os.path.join(home, "games"),
            "XODUS_STEAM_DIR": os.path.join(home, "steam"),
            "XODUS_PROTON_DIR": os.path.join(home, "runtime"),
            "FERESTRE_PACKAGE_VERSION": "0.1.3-smoke",
            "XODUS_CLI_DIR": os.path.join(home, "client"),
        }
    )

    # A runtime that declares it can satisfy everything the packaged recipes ask
    # for. Without one, every title is blocked on "install the runtime first" and
    # the test can only ever exercise that one path -- which is not the path most
    # of the window is about.
    runtime = os.path.join(home, "runtime")
    os.makedirs(os.path.join(runtime, "files", "share", "ferestre"), exist_ok=True)
    with open(os.path.join(runtime, "proton"), "w") as f:
        f.write("#!/bin/sh\nexit 0\n")
    with open(os.path.join(runtime, "version"), "w") as f:
        f.write("0 smoke-test-runtime\n")
    wanted = set()
    recipe_count = 0
    for name in os.listdir(recipes_dir):
        if not name.endswith(".toml") or name == "capabilities.toml":
            continue
        with open(os.path.join(recipes_dir, name), "rb") as f:
            recipe = tomllib.load(f)
        recipe_count += 1
        wanted |= set(recipe.get("runtime", {}).get("requires", []))
        wanted |= set(recipe.get("runtime", {}).get("wants", []))
    with open(os.path.join(runtime, "files", "share", "ferestre", "capabilities.json"), "w") as f:
        json.dump({"capabilities": sorted(wanted)}, f)

    # A title that is installed but whose build nothing can identify: a manifest,
    # so there is a version, and no package header, so there is no content id.
    # This is the state the window must not draw the same as "up to date" --
    # same row, same button, so only the words can separate them.
    unrecorded = os.path.join(home, "games", "bedrock", "game")
    os.makedirs(unrecorded, exist_ok=True)
    with open(os.path.join(unrecorded, "appxmanifest.xml"), "w") as f:
        f.write('<Package><Identity Name="Microsoft.MinecraftUWP" Version="9.9.9.9" /></Package>')

    # An owned library, cached the way a signed-in run leaves it, because the
    # three things this section checks are all invisible without one: names
    # instead of product ids, the filter that keeps the unrunnable majority out
    # of the way, and the confirmation before a download starts.
    #
    # The proportions are the real ones. Of 102 titles owned on the development
    # account exactly seven ship as MSIXVC; the rest are UWP packages this
    # runtime cannot open. A fixture with one of each would pass a filter that
    # is useless at the size that matters.
    # The cache refuses an entry written by a different build, which is the
    # whole point of the field -- so a fixture that hardcodes a number seeds a
    # cache the window correctly ignores, and every check below times out
    # waiting for a row that will never draw. Read it from the constant instead.
    catalog_rs = os.path.join(os.getcwd(), "crates", "ferestre-core", "src", "catalog.rs")
    schema = re.search(r"const CACHE_SCHEMA: u32 = (\d+);", open(catalog_rs).read())
    if not schema:
        raise Failure(f"no CACHE_SCHEMA in {catalog_rs}; has it been renamed?")
    schema = int(schema.group(1))

    state = os.path.join(home, "state", "ferestre")
    catalog = os.path.join(home, "cache", "ferestre", "catalog")
    os.makedirs(state, exist_ok=True)
    os.makedirs(catalog, exist_ok=True)
    owned = [("9ZZSMOKEGDK1", "Smoke Test Racer", "MSIXVC", 12_500_000_000)]
    owned += [
        (f"9ZZSMOKEUWP{n}", f"Uwp Puzzle {n}", "AppxBundle", 90_000_000) for n in range(1, 26)
    ]
    with open(os.path.join(state, "library.json"), "w") as f:
        json.dump(
            {
                "count": len(owned),
                "items": [
                    {"productId": pid, "productType": "Game", "status": "Active"}
                    for pid, _, _, _ in owned
                ],
            },
            f,
        )
    for pid, name, package_format, size in owned:
        with open(os.path.join(catalog, f"{pid}.json"), "w") as f:
            json.dump(
                {
                    "schema": schema,
                    "market": "US",
                    "language": "en-us",
                    "product": {
                        "product_id": pid,
                        "name": name,
                        "publisher": "Smoke Test Studios",
                        "download_bytes": size,
                        "content_ids": [f"contentid-{pid.lower()}"],
                        "has_packages": True,
                        "has_pc_package": True,
                        "package_format": package_format,
                    },
                },
                f,
            )

    # A Game Pass listing, cached, so the section draws from disk. Seeded rather
    # than fetched: this test must not depend on a Microsoft endpoint being up,
    # and a listing already present is one the window will not go and refetch.
    pass_titles = [
        ("9ZZSMOKEPASS", "Subscription Racer", "MSIXVC", 8_100_000_000),
        ("9ZZSMOKEPUWP", "Subscription Puzzler", "AppxBundle", 120_000_000),
        ("9ZZSMOKETIER", "Higher Tier Only", "MSIXVC", 5_000_000_000),
    ]
    # The tier map is seeded with the listing, and its presence is what stops
    # the window going to the network for one: a section that fetched on every
    # open would make this suite depend on a Microsoft endpoint being up.
    listing_schema = re.search(
        r"const LISTING_SCHEMA: u32 = (\d+);",
        open(os.path.join(os.getcwd(), "crates", "ferestre-core", "src", "gamepass.rs")).read(),
    )
    if not listing_schema:
        raise Failure("no LISTING_SCHEMA in gamepass.rs; has it been renamed?")
    with open(os.path.join(state, "gamepass.json"), "w") as f:
        json.dump(
            {
                "schema": int(listing_schema.group(1)),
                "fetched": 0,
                "market": "US",
                "product_ids": [pid for pid, _, _, _ in pass_titles],
                "tiers": {
                    "included": {
                        # In the tier the seeded account holds, and not.
                        "9ZZSMOKEPASS": ["StandardSubMetadata", "PCSubMetadata"],
                        "9ZZSMOKETIER": ["PCSubMetadata"],
                    }
                },
            },
            f,
        )
    for pid, name, package_format, size in pass_titles:
        with open(os.path.join(catalog, f"{pid}.json"), "w") as f:
            json.dump(
                {
                    "schema": schema,
                    "market": "US",
                    "language": "en-us",
                    "product": {
                        "product_id": pid,
                        "name": name,
                        "publisher": "Smoke Test Studios",
                        "download_bytes": size,
                        "content_ids": [f"contentid-{pid.lower()}"],
                        "has_packages": True,
                        "has_pc_package": True,
                        "package_format": package_format,
                    },
                },
                f,
            )

    # Local PNGs exercise actual Add to Steam downloads, cache reuse and grid
    # installation without reaching an external artwork service.
    art_png = open("crates/ferestre-core/tests/fixtures/steam-art.png", "rb").read()
    class ArtworkServer(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(art_png)))
            self.end_headers()
            self.wfile.write(art_png)
        def log_message(self, *args):
            pass
    art_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ArtworkServer)
    threading.Thread(target=art_server.serve_forever, daemon=True).start()
    art_url = f"http://127.0.0.1:{art_server.server_port}/art.png"
    env["NO_PROXY"] = env["no_proxy"] = "127.0.0.1,localhost"
    # The seeded Game Pass listing is the US one; the market otherwise follows
    # the locale, which is not this test's to choose.
    env["FERESTRE_MARKET"] = "US"
    for filename in glob.glob(os.path.join(recipes_dir, "*.toml")):
        with open(filename, "rb") as f:
            title = tomllib.load(f).get("title", {})
        pid = title.get("product-id")
        if not pid:
            continue
        path = os.path.join(catalog, f"{pid.upper()}.json")
        entry = json.load(open(path)) if os.path.exists(path) else {
            "schema": schema, "market": "US", "language": "en-us",
            "product": {"product_id": pid, "name": title["name"]}}
        entry["product"]["image"] = art_url
        entry["product"]["artwork"] = {slot: art_url for slot in ("portrait", "landscape", "hero", "logo")}
        with open(path, "w") as f:
            json.dump(entry, f)

    # The window is run from a copy, with a stub `ferestre` beside it, because
    # that is how it picks its CLI -- a sibling first, the path second. Two
    # things follow. The window under test can never reach the real client, so
    # no check here can sign in, spend a download or touch an account. And the
    # install path becomes drivable: the stub prints the progress lines the real
    # client prints under XODUS_PROGRESS=json, which is the only way to see the
    # bar without downloading a game to watch it.
    bindir = os.path.join(home, "bin")
    os.makedirs(bindir, exist_ok=True)
    binary_copy = os.path.join(bindir, os.path.basename(binary))
    shutil.copy2(binary, binary_copy)
    stub = os.path.join(bindir, "ferestre")
    with open(stub, "w") as f:
        f.write(
            """#!/usr/bin/env python3
import json, os, select, sys, time
from pathlib import Path
home = Path(os.environ["HOME"])
if sys.argv[1:2] == ["uninstall"]:
    (home / "removal-started").touch()
    while not (home / "release-removal").exists():
        time.sleep(0.05)
    (home / "removal-finished").touch()
    sys.exit(0)
if sys.argv[1:2] == ["run"]:
    # Long enough that starting and exiting are separate redraws, as they are
    # for a real title: each one rebuilds the row the person just clicked.
    (home / "run-started").touch()
    time.sleep(4)
    sys.exit(0)
if sys.argv[1:2] != ["install"]:
    sys.exit(0)
if os.environ.get("XODUS_PROGRESS") != "json":
    # The real client only emits these when asked. A window that forgets to ask
    # would then sit on an empty bar, and this is where that gets noticed.
    sys.exit("progress was not requested")
if (home / "hold-download").exists():
    (home / "download-started").touch()
    while not select.select([sys.stdin], [], [], 0.05)[0]:
        pass
    sys.stdin.buffer.read(1)
    (home / "download-stopping").touch()
    while not (home / "release-cleanup").exists():
        time.sleep(0.05)
    (home / "download-stopped").touch()
    sys.exit(130)
total = 800_000_000
# Ten seconds of it: the estimate deliberately says nothing until it has a
# couple of seconds to average over, and on a loaded machine one walk of the
# accessibility tree can take most of a second, so a four-second run left the
# check only a moment to see the rate before the download ended.
for step in range(1, 101):
    print(json.dumps({"progress": {"done": total * step // 100, "total": total}}), flush=True)
    time.sleep(0.1)
print(":: done", flush=True)
"""
        )
    os.chmod(stub, 0o755)

    # The mount-local CLI is fine for children; a saved Steam target must use
    # APPIMAGE. Spaces exercise Steam's quoted Exe/StartDir fields as well.
    image = os.path.join(home, "My Games", "Ferestre.AppImage")
    os.makedirs(os.path.dirname(image))
    with open(image, "w") as f:
        f.write("#!/usr/bin/env python3\nimport json, sys\nprint(json.dumps(sys.argv[1:]))\n")
    os.chmod(image, 0o755)
    env["APPIMAGE"] = image
    shortcuts = os.path.join(home, "steam", "userdata", "1", "config", "shortcuts.vdf")
    os.makedirs(os.path.dirname(shortcuts))

    gui = subprocess.Popen([binary_copy], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    deadline = time.monotonic() + TIMEOUT
    failures = []
    try:
        Atspi.init()
        app = find_app(gui.pid, deadline)
        frame = wait_for(
            app,
            lambda root: next(iter(find_all(root, role="frame")), None),
            "the window",
            deadline,
        )

        print("sidebar")
        labels = texts(frame)
        for section in ("Library", "Installed", "Updates", "Runtime"):
            check(section in labels, f"{section} is in the sidebar", frame)
        check("Not signed in" in labels, "says plainly that nobody is signed in", frame)

        print("library, with no account")
        # Recipes still show without an account: that is how someone sees a
        # title is supported before buying it.
        labels = wait_for(
            frame,
            lambda root: texts(root) if any("Minecraft" in t for t in texts(root)) else None,
            "the library to draw its rows",
            time.monotonic() + TIMEOUT,
        )
        check(
            any("Minecraft" in label for label in labels),
            "a recipe shows without anyone being signed in",
            frame,
        )
        check(
            any("Smoke Test Broken Game" in label for label in labels),
            "a broken title is listed rather than hidden",
            frame,
        )
        check(
            any("known not to run" in label for label in labels),
            "and it says what is known about it, without refusing to run it",
            frame,
        )

        print("the owned library")
        labels = wait_for(
            frame,
            lambda root: texts(root) if any("Smoke Test Racer" in t for t in texts(root)) else None,
            "the cached library to draw",
            time.monotonic() + TIMEOUT,
        )
        # The bug this replaces: every owned row drew its twelve-character
        # product id, because nothing ever asked the catalog for the names of
        # the rows the pager had put on screen.
        check(
            not any(t.startswith("9ZZSMOKE") for t in labels),
            f"no row is showing a raw product id: {[t for t in labels if t.startswith('9ZZSMOKE')]}",
            frame,
        )
        check(
            not any("Uwp Puzzle" in t for t in labels),
            "the titles this runtime cannot open are kept out of the way",
            frame,
        )
        check(
            any("25 ship as Appx or Msix packages" in t for t in labels),
            "and the count of them is stated, with the reason, rather than left "
            "as a silent filter",
            frame,
        )

        print("showing them anyway")
        # Three nodes carry this name -- the row, its label, and the switch
        # inside it -- and only the last of them does anything, so this goes
        # through the by-name search that skips the ones that cannot act.
        SWITCH = "Show titles you cannot install"
        click_named(frame, SWITCH, "the show-everything switch")
        labels = wait_for(
            frame,
            lambda root: texts(root) if any("Uwp Puzzle" in t for t in texts(root)) else None,
            "the hidden titles to appear",
            time.monotonic() + TIMEOUT,
        )
        check(
            any("AppxBundle package, not MSIXVC" in t for t in labels),
            "each says which container it ships in, not just that it failed",
            frame,
        )
        click_named(frame, SWITCH, "the show-everything switch")
        wait_for(
            frame,
            lambda root: not any("Uwp Puzzle" in t for t in texts(root)),
            "the filter to come back on",
            time.monotonic() + TIMEOUT,
        )
        check(True, "and the switch turns them back off")

        print("scroll position on title actions")
        def scroll_value():
            for node in find_all(frame, role="scroll bar"):
                value = node.get_value_iface()
                if value is not None and value.get_maximum_value() > 200:
                    return value
            return None
        value = scroll_value()
        check(value is not None, "the library has a scrollable title list", frame)
        value.set_current_value(200)
        time.sleep(0.25)
        play = row_button(frame, "Minecraft for Windows", "Play")
        check(play is not None, "the installed fixture can be selected to play", frame)
        # The title list grows with the recipe set. A fixed offset does not
        # guarantee this button is in the viewport; pointer events aimed at an
        # offscreen accessible hit another widget or empty space.
        # Check the target against its viewport before generating the pointer
        # event. This still tests a real focus-changing click, not activation.
        viewport = play.get_parent()
        while viewport is not None and viewport.get_role_name() != "scroll pane":
            viewport = viewport.get_parent()
        check(viewport is not None, "the Play button belongs to the title viewport", frame)
        # GTK4's AT-SPI ScrollTo method is unsupported. Use the same exposed
        # scrollbar Value interface as the rest of this check, aiming from the
        # target's current geometry instead of guessing a catalog-size offset.
        for _ in range(3):
            play = row_button(frame, "Minecraft for Windows", "Play")
            button_box = play.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
            view_box = viewport.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
            center = button_box.y + button_box.height // 2
            if view_box.y <= center < view_box.y + view_box.height:
                break
            value = scroll_value()
            target = value.get_current_value() + center - (view_box.y + view_box.height // 2)
            value.set_current_value(max(0, min(target, value.get_maximum_value())))
            time.sleep(0.25)
        position = scroll_value().get_current_value()
        check(position > 50, f"the library is scrolled before the click ({position})", frame)
        button_box = play.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
        view_box = viewport.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
        check(
            view_box.y <= button_box.y + button_box.height // 2 < view_box.y + view_box.height,
            f"the pointer target is visible (button y={button_box.y}, viewport y={view_box.y}, height={view_box.height})",
            frame,
        )
        # With the pointer, not an action: the click focuses the button, the
        # redraw destroys it, and GTK hands the focus to the first widget left
        # in the list -- scrolling the library to the top to show it.
        press(play, gui.pid)
        wait_for(
            frame,
            lambda root: row_button(root, "Minecraft for Windows", "Stop"),
            "the row to say the title is running",
            time.monotonic() + TIMEOUT,
        )
        time.sleep(0.5)
        after = scroll_value().get_current_value()
        check(abs(after - position) < 2, f"starting a title keeps the scroll position ({position} -> {after})", frame)
        wait_for(
            frame,
            lambda root: row_button(root, "Minecraft for Windows", "Play"),
            "the title to exit",
            time.monotonic() + TIMEOUT,
        )
        time.sleep(0.5)
        after = scroll_value().get_current_value()
        check(abs(after - position) < 2, f"and so does its exit ({position} -> {after})", frame)

        select_in_list(frame, "Runtime")
        wait_for(frame, lambda root: "Ferestre release" in texts(root), "section navigation", time.monotonic() + TIMEOUT)
        time.sleep(0.2)
        check(scroll_value().get_current_value() == 0, "a different section starts at the top", frame)
        select_in_list(frame, "Library")
        wait_for(frame, lambda root: "Minecraft for Windows" in texts(root), "return to library", time.monotonic() + TIMEOUT)

        labels = texts(frame)
        check(any("on disk" in t for t in labels), "installed rows show measured disk usage in Library", frame)
        check(any("Purchased" in t and "12.5 GB" in t for t in labels), "purchased games show their access source", frame)
        check(not any("not in this account's library listing" in t.lower() for t in labels), "missing direct purchase entries never claim missing access", frame)
        check(any("12.5 GB download" in t for t in labels), "catalog rows show download size in their details", frame)
        check_game_row_layout(frame, "Smoke Test Racer", "12.5 GB", "Install")
        check_game_row_layout(frame, "Minecraft for Windows", "on disk", "Play")
        print("installing asks before it downloads")
        install = row_button(frame, "Smoke Test Racer", "Install")
        check(install is not None, "an owned MSIXVC title offers an install", frame)
        click(install)
        labels = wait_for(
            frame,
            lambda root: texts(root) if any("Install Smoke Test Racer?" in t for t in texts(root)) else None,
            "the confirmation to open",
            time.monotonic() + TIMEOUT,
        )
        # 12.5 GB is a long way to get without being asked, and the destination
        # is a decision -- these machines have more than one disk and the
        # default is not always the roomy one.
        check(
            any("12.5 GB to download" in t for t in labels),
            f"the size is shown before the download starts: {[t for t in labels if 'GB' in t]}",
            frame,
        )
        destination = next(iter(find_all(frame, role="text", name="Install to")), None)
        check(destination is not None, "and the destination can be changed", frame)
        check(
            bool(Atspi.Text.get_text(destination.get_text_iface(), 0, -1).strip()),
            "prefilled with somewhere to put it",
            frame,
        )
        click_named(frame, "Cancel")
        wait_for(
            frame,
            lambda root: "Install Smoke Test Racer?" not in texts(root),
            "the confirmation to close",
            time.monotonic() + TIMEOUT,
        )
        check(
            not any("Installing" in t for t in texts(frame)),
            "and saying no starts nothing",
            frame,
        )

        print("an install whose build cannot be identified")
        unknown = [t for t in labels if "9.9.9.9" in t]
        check(bool(unknown), "the installed version is shown", frame)
        check(
            all("cannot be checked" in t for t in unknown),
            f"and it says the build is unknown rather than implying it is current: {unknown}",
            frame,
        )
        check(
            not any("up to date" in t for t in labels),
            "nothing claims to be up to date on no evidence",
            frame,
        )
        check(
            not os.path.exists(os.path.join(home, "state", "ferestre", "installed")),
            "and nothing was recorded, because there was no package header to read",
        )

        print("runtime section")
        select_in_list(frame, "Runtime")
        check(True, "the Runtime section can be selected")
        labels = wait_for(
            frame,
            lambda root: texts(root) if any("Proton" in t for t in texts(root)) else None,
            "the runtime section to draw",
            time.monotonic() + TIMEOUT,
        )
        check(
            any("smoke-test-runtime" in t for t in labels),
            "the runtime it found is named",
            frame,
        )

        check(
            "Ferestre release" in labels and "0.1.3-smoke" in labels,
            "the launcher release uses the packaged version",
            frame,
        )
        check(
            labels.index("Ferestre release") < labels.index("Patched Proton"),
            "the launcher release appears before the Proton runtime",
            frame,
        )

        print("installed section")
        select_in_list(frame, "Installed")
        labels = wait_for(
            frame,
            lambda root: texts(root) if any("9.9.9.9" in t for t in texts(root)) else None,
            "the installed title",
            time.monotonic() + TIMEOUT,
        )
        check(
            not any("Forza" in t for t in labels),
            "the section lists only what is on disk, not every recipe",
            frame,
        )

        print("updates section")
        select_in_list(frame, "Updates")
        labels = wait_for(
            frame,
            lambda root: texts(root) if any("up to date" in t for t in texts(root)) else None,
            "the empty-updates message",
            time.monotonic() + TIMEOUT,
        )
        # A title whose build cannot be identified is not an update, and must not
        # be listed as one: "we cannot tell" is not "there is something newer".
        check(
            not any("9.9.9.9" in t for t in labels),
            "a title of unknown build is not reported as needing an update",
            frame,
        )

        print("game pass")
        select_in_list(frame, "Game Pass")
        labels = wait_for(
            frame,
            lambda root: texts(root)
            if any("Subscription Racer" in t for t in texts(root))
            else None,
            "the Game Pass section",
            time.monotonic() + TIMEOUT,
        )
        # Counted from the rows below it: the AppxBundle is held back, so two.
        check(
            any("2 games in PC Game Pass for the US Store" in t for t in labels),
            "the section says what the list is, where, and how long it is",
            frame,
        )
        # The distinction the whole section rests on: what the catalogue
        # includes is not the same as what this account may install, and saying
        # so is what stops a licence refusal from reading as a bug.
        check(
            any("Sign in to see which of these your Store account can play" in t for t in labels),
            "and says that nobody signed in means nothing is known about access",
            frame,
        )
        check(
            any("Game Pass catalogue" in t and "8.1 GB" in t for t in labels),
            f"a row carries the size before the download: {[t for t in labels if 'GB' in t]}",
            frame,
        )
        check(
            not any("Subscription Puzzler" in t for t in labels),
            "the same filter applies: an AppxBundle is held back here too",
            frame,
        )
        # The seeded account holds nothing, so nothing is known about tiers and
        # nothing may be hidden on that basis -- hiding a title because a lookup
        # has not happened is the same mistake as refusing one.
        check(
            any("Higher Tier Only" in t for t in labels),
            "with no subscription known, no title is hidden for being outside a tier",
            frame,
        )
        check(
            row_button(frame, "Subscription Racer", "Install") is not None,
            "and an included title offers an install",
            frame,
        )

        check(any("8.1 GB download" in t for t in labels), "Game Pass rows use the same size details", frame)
        check_game_row_layout(frame, "Subscription Racer", "8.1 GB", "Install")
        print("a download says how far, how fast and how long")
        select_in_list(frame, "Library")
        wait_for(
            frame,
            lambda root: row_button(root, "Smoke Test Racer", "Install"),
            "the library to come back",
            time.monotonic() + TIMEOUT,
        )
        click(row_button(frame, "Smoke Test Racer", "Install"))
        wait_for(
            frame,
            lambda root: any("Install Smoke Test Racer?" in t for t in texts(root)),
            "the confirmation",
            time.monotonic() + TIMEOUT,
        )
        click_named(frame, "Install", "the confirm button")
        labels = wait_for(
            frame,
            lambda root: texts(root)
            if any("Installing Smoke Test Racer" in t for t in texts(root))
            else None,
            "the download bar",
            time.monotonic() + TIMEOUT,
        )
        check(True, "the bar says what is being installed")
        installing = row_button(frame, "Smoke Test Racer", "Installing..")
        check(
            installing is not None
            and not installing.get_state_set().contains(Atspi.StateType.SENSITIVE),
            "the active install immediately shows a disabled Installing.. button",
            frame,
        )
        for button in find_all(frame, role="button"):
            if any(node.get_role_name() == "label" and node.get_name() in {"Install", "Update"}
                   for node, _ in walk(button)):
                check(
                    not button.get_state_set().contains(Atspi.StateType.SENSITIVE),
                    "other games cannot be installed or updated during a download",
                    frame,
                )
        # The three things a progress bar is for. Waited for rather than read
        # once: the rate and the estimate deliberately say nothing until there
        # is enough history to divide by.
        detail = wait_for(
            frame,
            lambda root: next(
                (t for t in texts(root) if " of 800 MB" in t and "left" in t), None
            ),
            "the byte count, the rate and the time remaining",
            time.monotonic() + TIMEOUT,
        )
        check("MB/s" in detail, f"a transfer rate: {detail!r}", frame)
        check("left" in detail, f"and a time remaining: {detail!r}", frame)
        wait_for(
            frame,
            lambda root: not any("Installing Smoke Test Racer" in t for t in texts(root)),
            "the bar to go away when the download ends",
            time.monotonic() + TIMEOUT,
        )
        check(True, "and it disappears when the download finishes")
        restored = row_button(frame, "Smoke Test Racer", "Install")
        check(
            restored is not None and restored.get_state_set().contains(Atspi.StateType.SENSITIVE),
            "the Install button is enabled again after completion",
            frame,
        )

        print("the editor")
        select_in_list(frame, "Library")
        wait_for(
            frame,
            lambda root: any("Minecraft" in t for t in texts(root)),
            "the library to come back",
            time.monotonic() + TIMEOUT,
        )
        # One per recipe, counted from titles/ rather than written down here.
        # This used to assert a literal 3 and went stale the moment a fourth
        # recipe landed -- it then failed on every commit that added a title,
        # which is exactly the commit whose author has least reason to suspect
        # the GUI suite.
        offered = {}
        for name, menu in row_menus(frame).items():
            offered[name] = set(menu_items(app, menu))
            close_menu(app)
        edit = [name for name, items in offered.items() if "Edit launch settings" in items]
        steam = [name for name, items in offered.items() if "Add to Steam" in items]
        check(
            len(edit) == recipe_count,
            f"every recipe offers an edit ({len(edit)} menus for {recipe_count} recipes)",
            frame,
        )
        check(
            len(steam) == recipe_count,
            f"and an Add to Steam ({len(steam)} for {recipe_count} recipes)",
            frame,
        )
        check(
            all("Open Store page" in items for items in offered.values()),
            "every row's menu links its Store page",
            frame,
        )
        row_action(app, frame, "Add to Steam", steam[0])
        wait_for(frame, lambda root: os.path.isfile(shortcuts),
                 "the saved Steam shortcut", time.monotonic() + TIMEOUT)
        with open(shortcuts, "rb") as f:
            shortcut_bytes = f.read()

        def shortcut_string(key):
            return shortcut_bytes.split(b"\x01" + key.encode() + b"\0", 1)[1].split(b"\0", 1)[0].decode()

        check(shortcut_string("Exe") == f'"{image}"', "Steam target is the persistent AppImage", frame)
        check(shortcut_string("StartDir") == f'"{os.path.dirname(image)}"', "Steam start directory is persistent", frame)
        saved_options = shortcut_string("LaunchOptions")
        check(saved_options.startswith("run "), "Steam shortcut keeps the title launch options", frame)
        stored_id = struct.unpack("<I", shortcut_bytes.split(b"\x02appid\0", 1)[1][:4])[0]
        icon_path = shortcut_string("icon")
        grid = os.path.join(os.path.dirname(shortcuts), "grid")
        check(icon_path == os.path.join(grid, f"{stored_id}_icon.png") and os.path.isfile(icon_path),
              "Steam gets a persistent game icon in its own directory", frame)
        for suffix in ("p", "", "_hero", "_logo"):
            check(open(os.path.join(grid, f"{stored_id}{suffix}.png"), "rb").read() == art_png,
                  f"Steam artwork {suffix or 'landscape'} uses the stored app id", frame)
        # Reproduce an older shortcut, including a renamed title and vanished
        # AppImage mount, then re-add. Keep the id/controller configuration.
        saved_name = shortcut_string("AppName")
        legacy = shortcut_bytes.replace(b"\x01Exe\0" + shortcut_string("Exe").encode() + b"\0",
                                        b'\x01Exe\0"/tmp/.mount_old/usr/bin/ferestre"\0')
        legacy = legacy.replace(b"\x01AppName\0" + saved_name.encode() + b"\0", b"\x01AppName\0Old title name\0")
        legacy = legacy.replace(b"\x01icon\0" + icon_path.encode() + b"\0", b"\x01icon\0\0")
        with open(shortcuts, "wb") as f:
            f.write(legacy)
        os.unlink(icon_path)
        previous_mtime = os.stat(shortcuts).st_mtime_ns
        row_action(app, frame, "Add to Steam")
        wait_for(frame, lambda root: os.stat(shortcuts).st_mtime_ns != previous_mtime,
                 "the existing Steam shortcut to update", time.monotonic() + TIMEOUT)
        shortcut_bytes = open(shortcuts, "rb").read()
        check(shortcut_bytes.count(b"\x01AppName\0") == 1, "re-adding updates instead of duplicating the shortcut", frame)
        check(shortcut_string("AppName") == saved_name and shortcut_string("Exe") == f'"{image}"',
              "re-adding repairs the old title and mount-local executable", frame)
        check(struct.unpack("<I", shortcut_bytes.split(b"\x02appid\0", 1)[1][:4])[0] == stored_id,
              "updating preserves the Steam app id and controller configuration identity", frame)
        check(os.path.isfile(shortcut_string("icon")), "re-adding fills the missing game icon", frame)
        click(edit[0])
        labels = wait_for(
            frame,
            lambda root: texts(root) if any("Executable" in t for t in texts(root)) else None,
            "the editor to open",
            time.monotonic() + TIMEOUT,
        )
        check(
            any("saved to your own titles directory" in t for t in labels),
            "the editor says where an edit goes",
            frame,
        )
        check(
            any("KEY=VALUE" in t for t in labels),
            "and how to write the environment",
            frame,
        )
        for field in (
            "Name",
            "Executable, inside the installed title",
            "Install directory",
            "Wine prefix directory",
        ):
            check(field in labels, f"the editor offers {field!r}", frame)
        check(
            bool(find_all(frame, role="button", name="Cancel")),
            "the editor can be dismissed",
            frame,
        )
        click_named(frame, "Cancel")
        wait_for(
            frame,
            lambda root: not find_all(root, role="dialog"),
            "the editor to close",
            time.monotonic() + TIMEOUT,
        )
        check(True, "and closing it leaves the library where it was")

        print("saving an edit")
        user_titles = os.path.join(home, "config", "ferestre", "titles")
        check(
            not glob.glob(os.path.join(user_titles, "*.toml")),
            "nothing has been written yet",
        )
        row_action(app, frame, "Edit launch settings")
        wait_for(
            frame,
            lambda root: any("Executable" in t for t in texts(root)),
            "the editor to open again",
            time.monotonic() + TIMEOUT,
        )
        # By role as well as name: the row's title string turns up on more than
        # one node, and only one of them is the field.
        executable = next(
            iter(find_all(frame, role="text", name="Executable, inside the installed title")),
            None,
        )
        check(executable is not None, "the executable field is reachable", frame)
        editable = executable.get_editable_text_iface()
        check(editable is not None, "and editable", frame)
        Atspi.EditableText.set_text_contents(editable, "SmokeTest/Changed.exe")
        click_named(frame, "Save")
        wait_for(
            frame,
            lambda root: not find_all(root, role="dialog"),
            "the editor to close after saving",
            time.monotonic() + TIMEOUT,
        )

        written = glob.glob(os.path.join(user_titles, "*.toml"))
        check(len(written) == 1, f"one recipe was written (found {written})")
        body = open(written[0]).read()
        check(
            'executable = "SmokeTest/Changed.exe"' in body,
            "with the change in it",
        )
        check(
            "open an issue" in body,
            "and a note saying what to do with it",
        )
        # The packaged recipe is untouched: an edit is an override, not a fork.
        packaged = os.path.join(os.getcwd(), "titles")
        check(
            "SmokeTest" not in open(os.path.join(packaged, os.path.basename(written[0]))).read(),
            "and the packaged recipe is left alone",
        )

        print("removal status and graceful close")
        # Earlier checks deliberately used an unrecorded manifest-only title.
        # Removal operates on the install record that a real download writes.
        installed_records = os.path.join(state, "installed")
        os.makedirs(installed_records, exist_ok=True)
        with open(os.path.join(installed_records, "9NBLGGH2JHXJ.json"), "w") as f:
            json.dump({"product_id": "9NBLGGH2JHXJ", "dir": unrecorded,
                       "content_ids": ["fixture-content-id"], "package_version": "9.9.9.9"}, f)
        click_named(frame, "Refresh library, subscriptions and catalogue")
        select_in_list(frame, "Installed")
        time.sleep(0.5)
        wait_for(frame, lambda root: any("on disk" in t for t in texts(root)),
                 "measured disk usage in Installed", time.monotonic() + TIMEOUT)
        check(True, "Installed rows show disk usage too")
        check(any(any(source in t for source in ("Purchased", "Game Pass", "Access not checked")) and "on disk" in t for t in texts(frame)),
              "Installed rows show the access source beside their details", frame)
        check(not any("not in this account's library listing" in t.lower() for t in texts(frame)),
              "Installed never confuses purchase-list absence with missing access", frame)
        wait_for(frame, lambda root: not any("Checking for game updates" in t for t in texts(root)),
                 "the automatic update check to finish", time.monotonic() + TIMEOUT)
        wait_for(frame, lambda root: row_menus(root) or None,
                 "the installed game's row menu", time.monotonic() + TIMEOUT)
        row_action(app, frame, "Remove from disk")
        wait_for(frame, lambda root: any("Remove Minecraft for Windows?" in t for t in texts(root)),
                 "the removal confirmation", time.monotonic() + TIMEOUT)
        click_named(frame, "Remove")
        wait_for(frame, lambda root: os.path.exists(os.path.join(home, "removal-started")),
                 "the removal to start", time.monotonic() + TIMEOUT)
        # Let the short starting toast expire: the persistent status must remain.
        time.sleep(6)
        check("Removing Minecraft for Windows" in texts(frame), "removal status remains visible", frame)
        tooltip = wait_for(
            frame, lambda root: next(iter(find_all(root, role="progress bar",
                                                  name_contains="Removing Minecraft for Windows")), None),
            "the spinner's activity tooltip", time.monotonic() + TIMEOUT,
        )
        check("Please wait" in tooltip.get_name(), "the spinner tooltip identifies the operation and game", frame)
        select_in_list(frame, "Library")
        click(row_button(frame, "Smoke Test Racer", "Install"))
        wait_for(frame, lambda root: any("Removing Minecraft for Windows. Please wait" in t for t in texts(root)),
                 "a specific busy message", time.monotonic() + TIMEOUT)
        check(not os.path.exists(os.path.join(home, "download-started")), "busy click did not start another operation")
        click_named(frame, "Close")
        wait_for(frame, lambda root: bool(find_all(root, role="button", name="Finish and Close")),
                 "the close confirmation during removal", time.monotonic() + TIMEOUT)
        click_named(frame, "Keep Open")
        wait_for(frame, lambda root: not find_all(root, role="dialog"),
                 "the close dialog to dismiss", time.monotonic() + TIMEOUT)
        check(gui.poll() is None, "cancelling close keeps the app open")
        click_named(frame, "Close")
        wait_for(frame, lambda root: bool(find_all(root, role="button", name="Finish and Close")),
                 "the second close confirmation", time.monotonic() + TIMEOUT)
        time.sleep(0.3)  # Let the dialog finish its opening animation.
        click_named(frame, "Finish and Close")
        wait_for(frame, lambda root: any("Ferestre will close when it is safe" in t for t in texts(root)),
                 "the pending close status", time.monotonic() + TIMEOUT)
        check(gui.poll() is None, "closing waits for the removal")
        # Pending close is reversible while removal runs.
        click_named(frame, "Keep Window Open")
        click_named(frame, "Close")
        wait_for(frame, lambda root: bool(find_all(root, role="button", name="Finish and Close")),
                 "the close confirmation after keeping the window open", time.monotonic() + TIMEOUT)
        time.sleep(0.3)  # Let the dialog finish its opening animation.
        click_named(frame, "Finish and Close")
        open(os.path.join(home, "release-removal"), "w").close()
        check(gui.wait(timeout=TIMEOUT) == 0, "the app closes after removal finishes")
        check(os.path.exists(os.path.join(home, "removal-finished")), "removal completed before exit")
        stderr_before_restart = gui.stderr.read()
        # Simulate the mount-local CLI disappearing once the GUI exits.
        os.rename(stub, stub + ".unmounted")
        launched = subprocess.run([image, *shlex.split(saved_options)], env=env,
                                  capture_output=True, text=True, check=True)
        check(json.loads(launched.stdout) == shlex.split(saved_options),
              "saved shortcut launches after the GUI and its inner CLI are gone")
        os.rename(stub + ".unmounted", stub)


        print("download cancellation waits for cleanup")
        open(os.path.join(home, "hold-download"), "w").close()
        gui = subprocess.Popen([binary_copy], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        app = find_app(gui.pid, time.monotonic() + TIMEOUT)
        frame = wait_for(app, lambda root: next(iter(find_all(root, role="frame")), None),
                         "the restarted window", time.monotonic() + TIMEOUT)
        select_in_list(frame, "Library")
        wait_for(frame, lambda root: row_button(root, "Smoke Test Racer", "Install"),
                 "the install action", time.monotonic() + TIMEOUT)
        click(row_button(frame, "Smoke Test Racer", "Install"))
        wait_for(frame, lambda root: bool(find_all(root, role="button", name="Install")),
                 "install confirmation", time.monotonic() + TIMEOUT)
        click_named(frame, "Install")
        wait_for(frame, lambda root: os.path.exists(os.path.join(home, "download-started")),
                 "the held download", time.monotonic() + TIMEOUT)
        click_named(frame, "Close")
        wait_for(frame, lambda root: bool(find_all(root, role="button", name="Stop Download and Close")),
                 "download close confirmation", time.monotonic() + TIMEOUT)
        click_named(frame, "Keep Open")
        wait_for(frame, lambda root: not find_all(root, role="dialog"),
                 "the close dialog to dismiss", time.monotonic() + TIMEOUT)
        check(not os.path.exists(os.path.join(home, "download-stopping")), "dismissing close leaves the download running")
        click_named(frame, "Close")
        wait_for(frame, lambda root: bool(find_all(root, role="button", name="Stop Download and Close")),
                 "the next close confirmation", time.monotonic() + TIMEOUT)
        time.sleep(0.3)
        click_named(frame, "Stop Download and Close")
        wait_for(frame, lambda root: os.path.exists(os.path.join(home, "download-stopping")),
                 "cancellation to reach the downloader", time.monotonic() + TIMEOUT)
        check(gui.poll() is None, "the window stays alive during download cleanup")
        check(any("Stopping the download and cleaning up" in t for t in texts(frame)),
              "cleanup is explained while waiting", frame)
        open(os.path.join(home, "release-cleanup"), "w").close()
        check(gui.wait(timeout=TIMEOUT) == 0, "the app closes after download cleanup")
        check(os.path.exists(os.path.join(home, "download-stopped")), "download cleanup completed before exit")
        print("cached lists survive restarts; refreshing lists never blocks close")
        # Count requests at a refused local proxy: a fresh cached catalogue must
        # not be fetched at startup, even when account metadata is refreshed.
        class CountingProxy(http.server.BaseHTTPRequestHandler):
            requests = []
            def do_CONNECT(self):
                self.requests.append(self.path)
                self.send_error(502)
            def log_message(self, *args):
                pass
        proxy = http.server.ThreadingHTTPServer(("127.0.0.1", 0), CountingProxy)
        threading.Thread(target=proxy.serve_forever, daemon=True).start()
        proxy_url = f"http://127.0.0.1:{proxy.server_port}"
        for variable in ("HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"):
            env[variable] = proxy_url
        client_dir = os.path.join(home, "client")
        os.makedirs(client_dir, exist_ok=True)
        fixture_account = {"gamertag": "Fixture Player", "xuid": "fixture-player"}
        for filename in (os.path.join(state, "account.json"), os.path.join(home, "client-account.json")):
            with open(filename, "w") as f:
                json.dump(fixture_account, f)
        client = os.path.join(client_dir, "xodus-cli")
        with open(client, "w") as f:
            f.write("""#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
home = Path(os.environ['HOME'])
with (home / 'client-calls').open('a') as log:
    log.write(sys.argv[1] + '\\n')
if sys.argv[1] == 'account':
    print((home / 'client-account.json').read_text())
elif sys.argv[1] == 'library':
    (home / 'refresh-started').touch()
    (home / 'refresh-child-pid').write_text(str(os.getpid()))
    while (home / 'hold-refresh').exists():
        time.sleep(.05)
    print((home / 'client-library.json').read_text())
elif sys.argv[1] == 'update':
    print(json.dumps({'version': '9.9.9.9'}))
""")
        os.chmod(client, 0o755)
        # Six-hour-old lists and update checks must survive app restarts.
        for filename in ("library.json", "gamepass.json"):
            os.utime(os.path.join(state, filename), (time.time() - 21600, time.time() - 21600))
        update_cache = os.path.join(state, "updates", "9NBLGGH2JHXJ.json")
        os.makedirs(os.path.dirname(update_cache), exist_ok=True)
        with open(update_cache, "w") as f:
            json.dump({"schema": 1, "record": json.load(open(os.path.join(installed_records, "9NBLGGH2JHXJ.json"))),
                       "available": "9.9.9.9"}, f)
        os.utime(update_cache, (time.time() - 21600, time.time() - 21600))
        # Fill recipe metadata too, so a fresh startup has no legitimate misses.
        for filename in glob.glob(os.path.join(recipes_dir, "*.toml")):
            with open(filename, "rb") as f:
                title = tomllib.load(f).get("title", {})
            pid = title.get("product-id")
            if pid:
                path = os.path.join(catalog, f"{pid.upper()}.json")
                if not os.path.exists(path):
                    with open(path, "w") as f:
                        json.dump({"schema": schema, "market": "US", "language": "en-us",
                                   "product": {"product_id": pid, "name": title["name"]}}, f)
        usage_paths = glob.glob(os.path.join(home, "cache", "ferestre", "disk-usage", "*.json"))
        check(bool(usage_paths), "disk usage measurements are persisted for future app loads")
        usage_snapshots = {path: (os.stat(path).st_mtime_ns, open(path, "rb").read()) for path in usage_paths}
        cached_library = open(os.path.join(state, "library.json"), "rb").read()
        with open(os.path.join(home, "client-library.json"), "wb") as f:
            f.write(cached_library)
        gui = subprocess.Popen([binary_copy], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        app = find_app(gui.pid, time.monotonic() + TIMEOUT)
        frame = wait_for(app, lambda root: next(iter(find_all(root, role="frame")), None),
                         "cached restart window", time.monotonic() + TIMEOUT)
        wait_for(frame, lambda root: "Fixture Player" in texts(root), "cached account identity", time.monotonic() + TIMEOUT)
        time.sleep(1)
        calls = open(os.path.join(home, "client-calls")).read().splitlines()
        check("account" in calls and "library" not in calls, "six-hour-old ownership is reused across app loads")
        check("update" not in calls, "six-hour-old game-update checks are reused at startup")
        for section in ("Runtime", "Installed", "Library"):
            select_in_list(frame, section)
        time.sleep(.3)
        check("update" not in open(os.path.join(home, "client-calls")).read().splitlines(),
              "switching views does not repeat cached update checks")
        check(not any("catalog.gamepass.com" in request for request in CountingProxy.requests),
              "fresh Game Pass listing is not fetched again on startup")
        check(open(os.path.join(state, "library.json"), "rb").read() == cached_library,
              "unchanged startup preserves the cached game list")
        check(not any("displaycatalog.mp.microsoft.com" in request for request in CountingProxy.requests),
              "fresh product metadata is not fetched again on startup")
        # A failed forced refresh still attempts the network and keeps rows.
        cached_product = os.path.join(catalog, "9ZZSMOKEGDK1.json")
        previous_product = open(cached_product, "rb").read()
        click_named(frame, "Refresh library, subscriptions and catalogue")
        wait_for(frame, lambda root: any("displaycatalog.mp.microsoft.com" in request for request in CountingProxy.requests),
                 "explicit product metadata revalidation", time.monotonic() + TIMEOUT)
        check(open(cached_product, "rb").read() == previous_product,
              "failed metadata revalidation preserves the last good product")
        check(any("Smoke Test Racer" in t for t in texts(frame)),
              "cached rows remain visible during failed revalidation", frame)
        click_named(frame, "Close")
        check(gui.wait(timeout=5) == 0, "catalogue revalidation never blocks closing")
        calls = open(os.path.join(home, "client-calls")).read().splitlines()
        check("library" in calls and "update" in calls,
              "top-right Refresh bypasses both daily library and game-update caches")
        update_calls = calls.count("update")
        os.utime(update_cache, (time.time() - 172800, time.time() - 172800))
        # Expired metadata revalidates automatically while cached names draw.
        for path in glob.glob(os.path.join(catalog, "*.json")):
            os.utime(path, (time.time() - 172800, time.time() - 172800))
        CountingProxy.requests.clear()
        gui = subprocess.Popen([binary_copy], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        app = find_app(gui.pid, time.monotonic() + TIMEOUT)
        frame = wait_for(app, lambda root: next(iter(find_all(root, role="frame")), None),
                         "stale catalogue window", time.monotonic() + TIMEOUT)
        wait_for(frame, lambda root: any("displaycatalog.mp.microsoft.com" in request for request in CountingProxy.requests),
                 "automatic stale metadata revalidation", time.monotonic() + TIMEOUT)
        check(any("Smoke Test Racer" in t for t in texts(frame)),
              "stale product names remain visible while revalidation fails", frame)
        check(open(cached_product, "rb").read() == previous_product,
              "automatic failed revalidation keeps the previous snapshot")
        wait_for(frame, lambda root: open(os.path.join(home, "client-calls")).read().splitlines().count("update") > update_calls,
                 "expired game-update checks to run", time.monotonic() + TIMEOUT)
        wait_for(frame, lambda root: time.time() - os.stat(update_cache).st_mtime < 60,
                 "the daily game-update result to persist", time.monotonic() + TIMEOUT)
        check(True, "expired game updates refresh after 24 hours and persist for the next load")
        # Restore ownership after the forced refresh's synthetic client answer.
        with open(os.path.join(state, "library.json"), "wb") as f:
            f.write(cached_library)
        refresh_marker = os.path.join(home, "refresh-started")
        if os.path.exists(refresh_marker):
            os.unlink(refresh_marker)
        check(all((os.stat(path).st_mtime_ns, open(path, "rb").read()) == snapshot
                  for path, snapshot in usage_snapshots.items()),
              "restarts and catalogue refreshes reuse fresh disk usage measurements")
        # Request an actual library refresh and hold its child, then close.
        open(os.path.join(home, "hold-refresh"), "w").close()
        click(find_all(frame, role="button", name_contains="Fixture Player")[0])
        wait_for(frame, lambda root: bool(find_all(root, role="button", name="Reload library")),
                 "the account menu", time.monotonic() + TIMEOUT)
        click_named(frame, "Reload library")
        wait_for(frame, lambda root: os.path.exists(os.path.join(home, "refresh-started")),
                 "a list refresh to start", time.monotonic() + TIMEOUT)
        click_named(frame, "Close")
        check(gui.wait(timeout=5) == 0, "closing during list refresh exits immediately without Finish and Close")
        check(open(os.path.join(state, "library.json"), "rb").read() == cached_library,
              "closing an unfinished refresh retains the last complete cache")
        os.unlink(os.path.join(home, "hold-refresh"))
        # An external account change must bypass a still-fresh ownership
        # cache. Its public catalogue remains reusable across accounts.
        with open(os.path.join(home, "client-account.json"), "w") as f:
            json.dump({"gamertag": "Other Fixture Player", "xuid": "other-fixture-player"}, f)
        with open(os.path.join(home, "client-library.json"), "w") as f:
            json.dump({"items": [{"productId": "9ZZSMOKEGAME", "productType": "Game", "status": "Active"}]}, f)
        gui = subprocess.Popen([binary_copy], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        app = find_app(gui.pid, time.monotonic() + TIMEOUT)
        frame = wait_for(app, lambda root: next(iter(find_all(root, role="frame")), None),
                         "account change window", time.monotonic() + TIMEOUT)
        wait_for(frame, lambda root: any("Other Fixture Player" in t for t in texts(root)),
                 "changed account metadata", time.monotonic() + TIMEOUT)
        wait_for(frame, lambda root: json.load(open(os.path.join(state, "library.json"))).get("count") == 1,
                 "ownership to refresh for the new account", time.monotonic() + TIMEOUT)
        check(True, "switching accounts invalidates fresh cached ownership")
        click_named(frame, "Close")
        check(gui.wait(timeout=5) == 0, "the account-change window closes normally")
        proxy.shutdown()
        if b"panicked" in stderr_before_restart:
            failures.append("the first window panicked")

    except Failure as e:
        if "frame" in locals():
            dump(frame, str(e))
        failures.append(str(e))
    finally:
        art_server.shutdown()
        # Release only this fixture's workers, even if an assertion failed.
        for gate in ("release-removal", "release-cleanup"):
            open(os.path.join(home, gate), "w").close()
        try:
            os.unlink(os.path.join(home, "hold-refresh"))
        except FileNotFoundError:
            pass
        gui.terminate()
        try:
            gui.wait(timeout=5)
        except subprocess.TimeoutExpired:
            gui.kill()
        stderr = (gui.stderr.read() or b"").decode(errors="replace")
        # GTK's theme parser complains about the system theme on every start and
        # it says nothing about this launcher.
        noise = ("Theme parser error", "prefer-dark-theme", "radv is not a conformant")
        interesting = [
            line
            for line in stderr.splitlines()
            if line.strip() and not any(n in line for n in noise)
        ]
        if interesting:
            print("\nstderr:", file=sys.stderr)
            print("\n".join(interesting), file=sys.stderr)
        if any("panicked" in line for line in interesting):
            failures.append("the window panicked; see stderr above")
        if args.keep:
            print(f"\nkept {home}")
        else:
            shutil.rmtree(home, ignore_errors=True)

    if failures:
        print("\nFAILED:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    print("\nall good")
    return 0


if __name__ == "__main__":
    sys.exit(main())

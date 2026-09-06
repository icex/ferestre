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
import glob
import json
import os
import re
import shutil
import subprocess
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
        last = predicate(root)
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
    env = dict(os.environ)
    env.update(
        {
            "HOME": home,
            "XDG_CACHE_HOME": os.path.join(home, "cache"),
            "XDG_STATE_HOME": os.path.join(home, "state"),
            "XDG_CONFIG_HOME": os.path.join(home, "config"),
            "GTK_A11Y": "atspi",
            "FERESTRE_TITLES": os.path.join(os.getcwd(), "titles"),
            # Recipes come from the checkout, but nothing else should: no Steam,
            # no runtime, no client.
            "XODUS_GAMES_DIR": os.path.join(home, "games"),
            "XODUS_STEAM_DIR": os.path.join(home, "steam"),
            "XODUS_PROTON_DIR": os.path.join(home, "runtime"),
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
    for name in os.listdir(os.path.join(os.getcwd(), "titles")):
        if not name.endswith(".toml") or name == "capabilities.toml":
            continue
        with open(os.path.join(os.getcwd(), "titles", name), "rb") as f:
            recipe = tomllib.load(f)
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
import json, os, sys, time
if sys.argv[1:2] != ["install"]:
    sys.exit(0)
if os.environ.get("XODUS_PROGRESS") != "json":
    # The real client only emits these when asked. A window that forgets to ask
    # would then sit on an empty bar, and this is where that gets noticed.
    sys.exit("progress was not requested")
total = 800_000_000
# Four seconds of it: the estimate deliberately says nothing until it has a
# couple of seconds to average over, so a shorter run would prove less.
for step in range(1, 41):
    print(json.dumps({"progress": {"done": total * step // 40, "total": total}}), flush=True)
    time.sleep(0.1)
print(":: done", flush=True)
"""
        )
    os.chmod(stub, 0o755)

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
            any("Forza Horizon 5" in label for label in labels),
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
        check(
            any("3 titles are included with PC Game Pass" in t for t in labels),
            "the section says what the list is and how long it is",
            frame,
        )
        # The distinction the whole section rests on: what the catalogue
        # includes is not the same as what this account may install, and saying
        # so is what stops a licence refusal from reading as a bug.
        check(
            any("No active subscription was found" in t for t in labels),
            "and says plainly that no subscription was found on this account",
            frame,
        )
        check(
            any("Included with PC Game Pass" in t and "8.1 GB" in t for t in labels),
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

        print("the editor")
        select_in_list(frame, "Library")
        wait_for(
            frame,
            lambda root: any("Minecraft" in t for t in texts(root)),
            "the library to come back",
            time.monotonic() + TIMEOUT,
        )
        edit = find_all(frame, role="button", name_contains="Edit how this title launches")
        check(len(edit) == 3, f"each of the three recipes offers an edit (found {len(edit)})", frame)
        check(
            len(find_all(frame, role="button", name_contains="Add to Steam")) == 3,
            "and an Add to Steam",
            frame,
        )
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
        click(find_all(frame, role="button", name_contains="Edit how this title launches")[0])
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

    except Failure as e:
        failures.append(str(e))
    finally:
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

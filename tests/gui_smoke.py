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
from gi.repository import Atspi  # noqa: E402

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
    """Every node in the tree, breadth of the whole window."""
    yield node, depth
    if depth >= limit:
        return
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None:
            yield from walk(child, depth + 1, limit)


def describe(node):
    role = node.get_role_name()
    name = node.get_name() or ""
    return f"{role}:{name!r}"


def texts(root):
    """Every label and name in the tree, for substring assertions."""
    found = []
    for node, _ in walk(root):
        name = node.get_name()
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


def find_all(root, role=None, name=None, name_contains=None):
    """Matching nodes, each once.

    The tree reaches some objects by more than one path, so a raw walk reports
    one dialog as three and one button as two. `path` is the object's identity
    on the bus, which is the only thing that deduplicates correctly.
    """
    out = []
    seen = set()
    for node, _ in walk(root):
        if node.path in seen:
            continue
        seen.add(node.path)
        if role and node.get_role_name() != role:
            continue
        node_name = node.get_name() or ""
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

    gui = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
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

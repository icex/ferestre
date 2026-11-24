#!/usr/bin/env python3
"""Notifications stack: several show at once, and each can be dismissed.

libadwaita's toast overlay showed one toast at a time and queued the rest, so
three results arriving together took fifteen seconds to read and a High one
pushed an older one back to reappear after the fact. The window now stacks
them. On the launch fixture (`gui_launch.prepare`: three installed titles, a
fake `ferestre run` that waits for a file), this checks that:

- three notifications fired together ("Starting Launch Game 0", "Starting
  Launch Game 1", "Launch Game 2 added to Downloads") are all on screen at
  once in a 1280x900 window;
- "Dismiss notification" removes that one and leaves the others;
- a launch that fails replaces its own "Starting ..." with the failure;
- the keyboard reaches them: F6 from the navigation puts the focus on the
  newest, Escape dismisses it and puts the focus back on the content.

    cargo build -p ferestre-gui -p ferestre-cli
    xvfb-run -a --server-args='-screen 0 1280x900x24 -noreset' \\
        dbus-run-session -- python3 tests/gui_notifications.py
    # or, without Xvfb, in a private KWin session:
    python3 tests/run_wayland_gui.py tests/gui_notifications.py \\
        --binary target/debug/ferestre-gui
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time

import gui_smoke as s
from gui_launch import prepare
from gui_updater import deadline

F6, ESCAPE = 0xffc3, 0xff1b


def alerts(root):
    """The notifications on screen, by text."""
    found = {}
    for node, _ in s.walk(root):
        try:
            if node.get_role_name() != "alert":
                continue
            if not node.get_state_set().contains(s.Atspi.StateType.SHOWING):
                continue
            found[node.get_name()] = node
        except s.GLib.GError:
            continue
    return found


def focused(root):
    return next((n for n, _ in s.walk(root)
                 if n.get_state_set().contains(s.Atspi.StateType.FOCUSED)), None)


def key(sym):
    s.Atspi.generate_keyboard_event(sym, None, s.Atspi.KeySynthType.SYM)
    time.sleep(0.15)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/ferestre-gui")
    args = parser.parse_args()
    s.Atspi.init()
    process = wm = frame = None
    with tempfile.TemporaryDirectory(prefix="ferestre-notifications-", dir="/var/tmp") as tmp:
        h = Path(tmp)
        ids, _, _, env = prepare(h, args.binary)
        if os.environ.get("FERESTRE_TEST_WM") != "1":
            wm = subprocess.Popen(["openbox", "--sm-disable"],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            time.sleep(0.3)
        try:
            with (h / "gui.log").open("wb") as log:
                process = subprocess.Popen([h / "bin/ferestre-gui"], env=env, stdout=log, stderr=log)
            app = s.find_app(process.pid, deadline())
            frame = s.wait_for(app, lambda r: next(iter(s.find_all(r, role="frame")), None),
                               "the window", deadline())

            def wait(test, what):
                return s.wait_for(frame, test, what, deadline())

            s.select_in_list(frame, "My Library")
            wait(lambda _: s.row_button(frame, "Launch Game 0", "Play"), "the first Play")
            wait(lambda _: s.row_button(frame, "Launch Game 2", "Update"), "the Update")
            # Clear whatever startup said, so only these three are counted.
            for node in list(alerts(frame).values()):
                for button in s.find_all(node, name="Dismiss notification"):
                    s.click(button)

            three = ("Starting Launch Game 0", "Starting Launch Game 1",
                     "Launch Game 2 added to Downloads")
            # One after another, each as soon as the last one shows: a redraw
            # between a lookup and its click can swallow the click.
            for control, text in zip(("Launch Game 0 · Play", "Launch Game 1 · Play",
                                      "Launch Game 2 · Update"), three):
                until = deadline()
                while True:
                    s.click_named(frame, control)
                    try:
                        s.wait_for(frame, lambda r: text in alerts(r), text,
                                   min(until, time.monotonic() + 2))
                        break
                    except s.Failure:
                        if time.monotonic() >= until:
                            raise
            shown = wait(lambda r: (lambda a: a if all(t in a for t in three) else None)(alerts(r)),
                         "all three notifications at once")
            s.check(True, "three notifications fired together are all on screen at once")
            ys = [shown[t].get_component_iface().get_extents(s.Atspi.CoordType.WINDOW).y for t in three]
            s.check(ys == sorted(ys) and len(set(ys)) == 3,
                    f"they stack, newest at the bottom ({ys})", frame)

            s.click(s.find_all(shown["Starting Launch Game 1"], name="Dismiss notification")[0])
            wait(lambda r: "Starting Launch Game 1" not in alerts(r), "the dismissed one to go")
            left = alerts(frame)
            s.check("Launch Game 2 added to Downloads" in left,
                    "dismissing one leaves the others", frame)

            # A failed launch replaces its own "Starting ..." at once.
            (h / f"fail-{ids[0]}").write_text("")
            wait(lambda r: any(t.startswith("Launch Game 0 stopped") or t.startswith("Launch Game 0 exited")
                               for t in alerts(r)), "the failure notification")
            s.check("Starting Launch Game 0" not in alerts(frame),
                    "the failure replaces the stale Starting notification", frame)

            # Keyboard: F6 from the navigation reaches the notifications, and
            # Escape dismisses the focused one and returns to the content.
            before = set(alerts(frame))
            s.select_in_list(frame, "My Library")
            # Three steps make a cycle; a redraw on the way may cost one.
            for _ in range(6):
                key(F6)
                node = focused(frame)
                if node is not None and node.get_name() == "Dismiss notification":
                    break
            node = focused(frame)
            s.check(node is not None and node.get_name() == "Dismiss notification",
                    "F6 cycles to the notifications", frame)
            key(ESCAPE)
            wait(lambda r: len(set(alerts(r))) < len(before), "Escape to dismiss the focused one")
            wait(lambda r: (n := focused(r)) is not None and " · " in (n.get_name() or ""),
                 "the focus back on a game")
            s.check(True, "Escape dismisses it and the focus returns to the content")

            for name in ids:
                (h / f"fail-{name}").write_text("")
                (h / f"end-{name}").write_text("")
            wait(lambda r: s.row_button(frame, "Launch Game 1", "Play"), "the fake games to end")
            s.click_named(frame, "Close")
            process.wait(timeout=20)
            s.check("panicked at" not in (h / "gui.log").read_text(), "no panic")
            print("PASS: notifications stack, dismiss and take keyboard focus", flush=True)
        except Exception:
            print((h / "gui.log").read_text()[-4000:], flush=True)
            if frame is not None:
                s.dump(frame, "notifications failure")
            raise
        finally:
            for name in ids:
                (h / f"end-{name}").write_text("")
            for p in (process, wm):
                if p is not None and p.poll() is None:
                    p.terminate()
                    p.wait(timeout=5)
            for f in h.glob("launch-*"):
                try:
                    pid = int(f.read_text())
                    environ = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
                    if ("HOME=" + str(h)).encode() in environ:
                        os.killpg(pid, 15)
                except (OSError, ValueError):
                    pass


if __name__ == "__main__":
    main()

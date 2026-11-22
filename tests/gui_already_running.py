#!/usr/bin/env python3
"""Play on a title that is already running, and "Reset to defaults" in the editor.

Already running: a fake game -- `sleep`, under the Windows image name Wine
would give it, with WINEPREFIX set to the fixture title's prefix as the window
works it out (`<games>/<slug>-proton/pfx`) -- stands in for a copy started
outside the window, by a Steam shortcut, say. Play must not start a second
copy. It asks: Leave It Running starts nothing and leaves the game alone;
Close It ends it; Close and Start Again ends it, then launches.

The fixture runtime's `wineserver` only writes down that it was asked to `-k`
the prefix, so it ends nothing, as a real one of another Wine build would not
either. With no `wine` beside it there is nothing to ask the game with, so
Close It goes straight to `wineserver -k` and then ends what is left in the
prefix itself. For Close and Start Again a fake `wine` is added whose
`taskkill` ends the fake game, as a game does when asked to close.

Reset to defaults: with a user override for a title, the editor's button is
sensitive; pressing it and confirming sets the override aside as
`<ID>.toml.bak`, and the button is insensitive when the editor opens again.

Everything is in a throwaway HOME under /var/tmp, so the fake game can never
be in a real prefix, and only processes this test started are ended.

    cargo build -p ferestre-gui -p ferestre-cli
    xvfb-run -a --server-args='-screen 0 1280x900x24 -noreset' \\
        dbus-run-session -- python3 tests/gui_already_running.py
    # or, without Xvfb, in a private KWin session:
    python3 tests/run_wayland_gui.py tests/gui_already_running.py \\
        --binary target/debug/ferestre-gui
"""
import argparse
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

import gui_smoke as s
from gui_launch import prepare
from gui_updater import write, deadline

FAKE_IMAGE = "FakeGame.exe"
RESPONSES = ("Leave It Running", "Close It", "Close and Start Again")


def start_fake_game(home, prefix):
    """A process as Wine starts one: argv[0] is the Windows executable."""
    game = subprocess.Popen(["C:\\Game\\" + FAKE_IMAGE, "600"], executable="sleep",
                            env={"WINEPREFIX": f"{prefix}/", "PATH": os.environ["PATH"]})
    until = time.monotonic() + 10
    while not Path(f"/proc/{game.pid}/cmdline").read_bytes().startswith(b"C:\\Game\\"):
        if time.monotonic() > until:
            raise s.Failure("the fake game never started")
        time.sleep(0.02)
    (home / "fake-game-pid").write_text(str(game.pid))
    return game


def sensitive(node):
    return node.get_state_set().contains(s.Atspi.StateType.SENSITIVE)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/ferestre-gui")
    args = parser.parse_args()
    s.Atspi.init()
    process = app = None
    games = []
    # Under /var/tmp: /tmp is RAM on many desktops, and no real prefix is here.
    with tempfile.TemporaryDirectory(prefix="ferestre-already-running-", dir="/var/tmp") as tmp:
        h = Path(tmp)
        ids, _, pad, env = prepare(h, args.binary)
        product, name = ids[0], "Launch Game 0"
        prefix = h / "games" / "launch-0-proton" / "pfx"
        prefix.mkdir(parents=True)
        bin_dir = h / "runtime" / "files" / "bin"
        calls = h / "wineserver-calls"
        write(bin_dir / "wineserver", f'#!/bin/sh\necho "$WINEPREFIX $*" >> {calls}\nexit 0\n', True)
        launch_receipt = h / f"launch-{product}"
        try:
            with (h / "gui.log").open("wb") as log:
                process = subprocess.Popen([h / "bin/ferestre-gui"], env=env, stdout=log, stderr=log)
            app = s.find_app(process.pid, deadline())
            frame = s.wait_for(app, lambda r: next(iter(s.find_all(r, role="frame")), None),
                               "window", deadline())

            def wait(test, what, limit=None):
                return s.wait_for(app, test, what, time.monotonic() + (limit or s.TIMEOUT))

            s.select_in_list(frame, "My Library")
            wait(lambda _: s.row_button(frame, name, "Play"), "Play")

            print("already running")
            game = start_fake_game(h, prefix)
            games.append(game)

            def ask():
                s.click_named(frame, f"{name} · Play")
                return wait(lambda root: any(t == f"{name} is already running" for t in s.texts(root)),
                            "the already-running dialog")

            ask()
            s.check(any(FAKE_IMAGE in t for t in s.texts(app)), "the dialog names what is running")
            for response in RESPONSES:
                s.check(bool(s.find_all(app, role="button", name=response)), f"it offers {response!r}")
            s.click_named(app, "Leave It Running")
            wait(lambda root: not s.find_all(root, role="button", name="Leave It Running"),
                 "the dialog to close")
            time.sleep(1.5)
            s.check(not launch_receipt.exists(), "Leave It Running starts nothing")
            s.check(game.poll() is None and not calls.exists(), "and leaves the running copy alone")
            s.check(s.row_button(frame, name, "Play") is not None, "the title shows Play again")

            ask()
            s.click_named(app, "Close It")
            wait(lambda root: any(t == f"{name} was closed" for t in s.texts(root)),
                 "Close It to report the title closed", 40)
            s.check(game.wait(timeout=5) == -signal.SIGKILL,
                    "the copy wineserver -k could not reach was ended directly")
            asked = calls.read_text().splitlines()
            s.check(asked == [f"{prefix} -k"], f"wineserver -k ran on the title's prefix only ({asked})")
            time.sleep(1.5)
            s.check(not launch_receipt.exists(), "Close It starts no second copy")

            print("close and start again")
            # A wine whose taskkill closes the fake game, as a game closes
            # when asked: it writes down what it was asked, then ends it.
            taskkill = h / "taskkill-calls"
            write(bin_dir / "wine",
                  f'#!/bin/sh\necho "$WINEPREFIX $*" >> {taskkill}\n'
                  f'kill "$(cat {h / "fake-game-pid"})"\nexit 0\n', True)
            game = start_fake_game(h, prefix)
            games.append(game)
            ask()
            s.click_named(app, "Close and Start Again")
            wait(lambda _: launch_receipt.exists(), "the title to start again once closed", 40)
            s.check(game.wait(timeout=5) == -signal.SIGTERM, "the running copy was asked, and closed")
            told = taskkill.read_text().splitlines()
            s.check(told == [f"{prefix}/ taskkill /IM {FAKE_IMAGE}"],
                    f"taskkill asked the title in its own prefix, without /F ({told})")
            s.check(calls.read_text().splitlines() == asked,
                    "a title that closed when asked is not killed")
            (h / f"fail-{product}").touch()
            wait(lambda _: s.row_button(frame, name, "Play"), "the fixture launch to end")

            print("reset to defaults")
            other, other_name = ids[1], "Launch Game 1"
            override = h / "config" / "ferestre" / "titles" / f"{other}.toml"
            write(override, (h / "titles" / f"{other}.toml").read_text().replace(
                '[launch]\n    executable="Game.exe"',
                '[launch]\n    executable="Game.exe"\n    env={FERESTRE_NATIVE_WAYLAND="1"}'))

            def open_editor():
                s.row_action(app, frame, "Edit launch settings", other_name)
                return wait(lambda root: next(iter(s.find_all(root, role="button", name="Reset to defaults")), None),
                            "the editor's Reset to defaults")

            reset = open_editor()
            s.check(sensitive(reset), "Reset to defaults is available with an override")
            s.click_named(app, "Reset to defaults")
            wait(lambda root: any(t == f"Reset {other_name} to its defaults?" for t in s.texts(root)),
                 "the reset confirmation")
            s.click_named(app, "Reset")
            # By the files: the toast saying so can be queued behind others.
            backup = override.with_name(f"{other}.toml.bak")
            wait(lambda _: backup.is_file() and not override.exists(), "the reset to finish")
            s.check(True, "the override was set aside as .bak")
            s.check("FERESTRE_NATIVE_WAYLAND" in backup.read_text(), "with the edit in it")
            wait(lambda root: not s.find_all(root, role="button", name="Reset to defaults"),
                 "the editor to close")
            reset = open_editor()
            s.check(not sensitive(reset), "Reset to defaults is unavailable with nothing to reset")
            s.click_named(app, "Cancel")
            wait(lambda root: not s.find_all(root, role="button", name="Reset to defaults"),
                 "the editor to close again")

            s.click_named(frame, "Close")
            s.check(process.wait(timeout=10) == 0, "the window closes cleanly")
            s.check("panicked at" not in (h / "gui.log").read_text(), "and nothing panicked")
            print("all already-running and reset checks passed", flush=True)
        except Exception:
            print((h / "gui.log").read_text(), flush=True)
            if app is not None:
                s.dump(app, "already-running failure")
            raise
        finally:
            pad()
            for game in games:
                if game.poll() is None:
                    game.kill()
                game.wait()
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            # Launches the window started own their sessions; end only ours.
            for receipt in h.glob("launch-*"):
                try:
                    pid = int(receipt.read_text())
                    environ = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
                    if ("HOME=" + str(h)).encode() in environ:
                        os.killpg(pid, signal.SIGTERM)
                except (OSError, ValueError):
                    pass


if __name__ == "__main__":
    main()

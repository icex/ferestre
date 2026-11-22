#!/usr/bin/env python3
"""`ferestre run` refuses to start a title already running in its prefix.

A second copy in the same prefix exits at once with a code that says nothing
about why, and `run` is often started by a Steam shortcut with nobody to ask.
So it refuses, says what is running and what to do, and exits with its own
code (4). `--force`, which is about runtime capabilities, does not get past
it; `--dry-run`, which starts nothing, does not check. Because a Steam shortcut
shows neither stderr nor the exit code, the refusal is also appended to the
title's launch log and, for a launch the window did not start, sent as a
desktop notification -- here to a fake `gdbus` that writes down what it was
asked, on a session bus address that leads nowhere.

Two launches a moment apart cannot both get past the scan: a launch holds a
lock beside the prefix's `pfx` until its title ends, and a second one refuses
while it is held.

The "game" is `sleep`, run under a Windows image name the way Wine names a
process, with WINEPREFIX set to the fixture title's prefix. Everything lives in
a throwaway HOME under /var/tmp, so the fake can never be in a real prefix, and
the test ends only the process it started.

    FERESTRE_BINARY=target/debug/ferestre python3 tests/test_run_already_running.py
"""

import fcntl
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

BINARY = Path(os.environ.get("FERESTRE_BINARY", "target/debug/ferestre")).resolve()
PRODUCT = "9ZZRUNNING01"
NAME = "Already Running Fixture"
SLUG = "already-running"
EXIT_ALREADY_RUNNING = 4


class RunAlreadyRunning(unittest.TestCase):
    def setUp(self):
        if not BINARY.is_file():
            self.fail(f"{BINARY} is not there; cargo build -p ferestre-cli")
        scratch = tempfile.TemporaryDirectory(prefix="ferestre-run-guard-", dir="/var/tmp")
        self.addCleanup(scratch.cleanup)
        home = self.home = Path(scratch.name)
        runtime = home / "runtime"
        (runtime / "files/share/ferestre").mkdir(parents=True)
        # Never reached: every run here stops at the missing Xbox service,
        # before Proton, if the guard lets it get that far.
        (runtime / "proton").write_text("#!/bin/sh\nexit 0\n")
        (runtime / "proton").chmod(0o755)
        (runtime / "version").write_text("0 guard-fixture-runtime\n")
        (runtime / "files/share/ferestre/capabilities.json").write_text('{"capabilities": []}')
        titles = home / "titles"
        titles.mkdir()
        (titles / f"{PRODUCT}.toml").write_text(f'''schema = 1
[title]
product-id = "{PRODUCT}"
name = "{NAME}"
slug = "{SLUG}"
[launch]
executable = "FakeGame.exe"
[status]
state = "untested"
summary = "Private fixture"
''')
        games = home / "games"
        (games / SLUG).mkdir(parents=True)
        (games / SLUG / "appxmanifest.xml").write_text(
            '<Package><Identity Name="Guard.Fixture" Version="1.0.0.0" /></Package>')
        # A client that answers nothing, and a private service directory with no
        # service in it: nothing here can reach a real account or a real
        # service socket.
        client = home / "client"
        client.mkdir()
        (client / "xodus-cli").write_text("#!/bin/sh\nexit 1\n")
        (client / "xodus-cli").chmod(0o755)
        runtime_dir = home / "xdg-runtime"
        runtime_dir.mkdir(mode=0o700)
        self.prefix = games / f"{SLUG}-proton"
        # The notification goes to this fake, never to the desktop.
        fake_bin = home / "bin"
        fake_bin.mkdir()
        self.notified = home / "notified"
        (fake_bin / "gdbus").write_text(
            f'#!/bin/sh\nprintf "%s\\n" "$@" > {self.notified}\nexit 0\n')
        (fake_bin / "gdbus").chmod(0o755)
        self.env = {k: v for k, v in os.environ.items()
                    if not k.startswith(("XODUS_", "FERESTRE_", "STEAM_", "WINE"))}
        self.env.update(
            HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
            XDG_STATE_HOME=str(home / "state"), XDG_CACHE_HOME=str(home / "cache"),
            XDG_DATA_HOME=str(home / "data"), XODUS_PROTON_DIR=str(runtime),
            XODUS_GAMES_DIR=str(games), XODUS_STEAM_DIR=str(home / "steam"),
            XODUS_CLI_DIR=str(client), FERESTRE_TITLES=str(titles),
            XODUS_REPO_DIR=str(Path(__file__).resolve().parents[1]),
            XDG_RUNTIME_DIR=str(runtime_dir), XODUS_RUNTIME_DIR=str(runtime_dir),
            HTTPS_PROXY="http://127.0.0.1:9", HTTP_PROXY="http://127.0.0.1:9",
            https_proxy="http://127.0.0.1:9", http_proxy="http://127.0.0.1:9",
            PATH=f"{fake_bin}:{os.environ['PATH']}",
            DBUS_SESSION_BUS_ADDRESS=f"unix:path={home}/no-bus")

    def start_fake_game(self, prefix):
        """A process Wine would have started: argv[0] the Windows path."""
        game = subprocess.Popen(["C:\\Game\\FakeGame.exe", "120"], executable="sleep",
                                env={"WINEPREFIX": f"{prefix}/", "PATH": os.environ["PATH"]})
        self.addCleanup(lambda: (game.kill(), game.wait()))
        # Until it has exec'd, its command line is still Python's.
        deadline = time.monotonic() + 10
        cmdline = Path(f"/proc/{game.pid}/cmdline")
        while not cmdline.read_bytes().startswith(b"C:\\Game\\FakeGame.exe"):
            self.assertLess(time.monotonic(), deadline, "the fake game never started")
            time.sleep(0.02)
        return game

    def run_cli(self, *args, env=None):
        return subprocess.run([BINARY, "run", PRODUCT, *args], env={**self.env, **(env or {})},
                              capture_output=True, text=True, timeout=60)

    def test_a_second_copy_is_refused_and_force_does_not_get_past_it(self):
        game = self.start_fake_game(self.prefix / "pfx")
        refusal = (f"{NAME} is already running in its prefix (FakeGame.exe). Close the game, "
                   "or press Play in Ferestre and choose Close It, then start it again.")
        log = self.home / "games" / f"{SLUG}-launch.log"
        for extra in ((), ("--force",)):
            result = self.run_cli(*extra)
            self.assertEqual(result.returncode, EXIT_ALREADY_RUNNING, result.stderr)
            self.assertIn(f"!! {refusal}", result.stderr)
        # Refused before the prefix is touched: the scan in prepare_launch,
        # not the one after the prefix lock is taken.
        self.assertFalse(self.prefix.exists(), "the prefix was created")
        self.assertIsNone(game.poll(), "the running title was touched")
        # Where someone who started it from Steam can find out why.
        self.assertEqual(log.read_text().count(f"!! {refusal}\n"), 2, "not in the launch log")
        notified = self.notified.read_text().splitlines()
        self.assertIn("org.freedesktop.Notifications.Notify", notified)
        self.assertIn(f"'{NAME} did not start'", notified)
        self.assertIn(f"'{refusal}'", notified)

        # The window says why itself: no notification for its launches.
        self.notified.unlink()
        windowed = self.run_cli(env={"FERESTRE_LAUNCH_ID": "test"})
        self.assertEqual(windowed.returncode, EXIT_ALREADY_RUNNING, windowed.stderr)
        self.assertFalse(self.notified.exists(), "the window's own launch sent a notification")

        # A dry run starts nothing, so it does not check.
        dry = self.run_cli("--dry-run")
        self.assertEqual(dry.returncode, 0, dry.stderr)
        self.assertNotIn("already running", dry.stderr)

        # Once it has gone, run gets past the check, as far as the Xbox
        # service this fixture deliberately does not have.
        game.kill()
        game.wait()
        after = self.run_cli()
        self.assertNotEqual(after.returncode, EXIT_ALREADY_RUNNING, after.stderr)
        self.assertNotIn("already running", after.stderr)
        self.assertIn("xodus-service is missing", after.stderr)

    def test_a_launch_still_starting_holds_the_prefix(self):
        """Another `run` holds the prefix's lock, with nothing of the title in
        it yet: this one refuses too."""
        self.prefix.mkdir(parents=True)
        with open(self.prefix / ".ferestre-run.lock", "w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.run_cli()
        self.assertEqual(result.returncode, EXIT_ALREADY_RUNNING, result.stderr)
        self.assertIn(f"!! {NAME} is already starting in its prefix", result.stderr)
        # Let go, the next launch gets past it, as far as the service.
        after = self.run_cli()
        self.assertNotEqual(after.returncode, EXIT_ALREADY_RUNNING, after.stderr)
        self.assertIn("xodus-service is missing", after.stderr)

    def test_a_title_in_another_prefix_does_not_count(self):
        self.start_fake_game(self.home / "games" / "other-proton" / "pfx")
        result = self.run_cli()
        self.assertNotEqual(result.returncode, EXIT_ALREADY_RUNNING, result.stderr)
        self.assertNotIn("already running", result.stderr)
        self.assertIn("xodus-service is missing", result.stderr)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""`ferestre run` refuses to start a title already running in its prefix.

A second copy in the same prefix exits at once with a code that says nothing
about why, and `run` is often started by a Steam shortcut with nobody to ask.
So it refuses, says what is running and what to do, and exits with its own
code (4). `--force`, which is about runtime capabilities, does not get past
it; `--dry-run`, which starts nothing, does not check.

The "game" is `sleep`, run under a Windows image name the way Wine names a
process, with WINEPREFIX set to the fixture title's prefix. Everything lives in
a throwaway HOME under /var/tmp, so the fake can never be in a real prefix, and
the test ends only the process it started.

    FERESTRE_BINARY=target/debug/ferestre python3 tests/test_run_already_running.py
"""

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
        # A Proton that would say it was started: nothing may start it while
        # the title runs.
        self.proton_ran = home / "proton-ran"
        (runtime / "proton").write_text(f"#!/bin/sh\ntouch {self.proton_ran}\nexit 0\n")
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
            https_proxy="http://127.0.0.1:9", http_proxy="http://127.0.0.1:9")

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

    def run_cli(self, *args):
        return subprocess.run([BINARY, "run", PRODUCT, *args], env=self.env,
                              capture_output=True, text=True, timeout=60)

    def test_a_second_copy_is_refused_and_force_does_not_get_past_it(self):
        game = self.start_fake_game(self.prefix / "pfx")
        for extra in ((), ("--force",)):
            result = self.run_cli(*extra)
            self.assertEqual(result.returncode, EXIT_ALREADY_RUNNING, result.stderr)
            self.assertIn(
                f"!! {NAME} is already running in its prefix (FakeGame.exe). Close it, "
                "or stop it from Ferestre, then start it again.", result.stderr)
        self.assertFalse(self.proton_ran.exists(), "Proton was started beside the running title")
        self.assertFalse((self.home / "games" / f"{SLUG}-launch.log").exists(),
                         "the launch log was written")
        self.assertFalse((self.prefix / "pfx").exists(), "the prefix was created")
        self.assertIsNone(game.poll(), "the running title was touched")

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

    def test_a_title_in_another_prefix_does_not_count(self):
        self.start_fake_game(self.home / "games" / "other-proton" / "pfx")
        result = self.run_cli()
        self.assertNotEqual(result.returncode, EXIT_ALREADY_RUNNING, result.stderr)
        self.assertNotIn("already running", result.stderr)
        self.assertIn("xodus-service is missing", result.stderr)


if __name__ == "__main__":
    unittest.main()

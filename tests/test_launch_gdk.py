#!/usr/bin/env python3
"""scripts/launch-gdk.sh starting (or reusing) the Xbox service, with stubs.

The client and the service are small stand-ins: the service binds the socket
the real one would (XODUS_RUNTIME_DIR, else XDG_RUNTIME_DIR), fails the same
way on a socket left behind, and records where it listened.
"""

import os
from pathlib import Path
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/launch-gdk.sh"

# Run by its own path with an absolute interpreter, so its process name is
# xodus-service, as pgrep would see the real one.
SERVICE = f"""#!{sys.executable}
import os, socket, sys, time
directory = os.environ.get("XODUS_RUNTIME_DIR") or os.environ["XDG_RUNTIME_DIR"]
path = os.path.join(directory, "xodus.sock")
s = socket.socket(socket.AF_UNIX)
try:
    s.bind(path)
except OSError as error:
    print("Unable to bind to socket:", error, flush=True)
    sys.exit(101)
s.listen()
with open(os.environ["SERVICE_RECORD"], "a") as f:
    f.write(f"{{os.getpid()}} {{path}} {{os.environ.get('XODUS_RUNTIME_DIR', '-')}}\\n")
print("service listening", flush=True)
while True:
    connection, _ = s.accept()
    connection.close()
"""


class LaunchGdkServiceTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="ferestre-launch-gdk-test-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.cli = self.root / "cli"
        self.cli.mkdir()
        service = self.cli / "xodus-service"
        service.write_text(SERVICE)
        service.chmod(0o755)
        client = self.cli / "xodus-cli"
        client.write_text('#!/bin/sh\necho "$@" >> "$CLIENT_RECORD"\n')
        client.chmod(0o755)
        self.record = self.root / "service.record"
        self.xdg = self.root / "xdg"
        self.xdg.mkdir(mode=0o700)
        self.tmp = self.root / "tmp"
        self.tmp.mkdir()
        for name in ("proton", "game", "prefix"):
            (self.root / name).mkdir()
        self.addCleanup(self.stop_services)

    def stop_services(self):
        if not self.record.exists():
            return
        for line in self.record.read_text().splitlines():
            try:
                os.kill(int(line.split()[0]), signal.SIGKILL)
            except ProcessLookupError:
                pass

    def env(self, **extra):
        env = {k: v for k, v in os.environ.items()
               if k not in ("XODUS_RUNTIME_DIR", "XODUS_CLI_DIR", "XODUS_PROTON_DIR")}
        env.update(HOME=str(self.root), XDG_RUNTIME_DIR=str(self.xdg), TMPDIR=str(self.tmp),
                   XODUS_CLI_DIR=str(self.cli), XODUS_PROTON_DIR=str(self.root / "proton"),
                   XODUS_GAMES_DIR=str(self.root / "games"), SERVICE_RECORD=str(self.record),
                   CLIENT_RECORD=str(self.root / "client.record"))
        env.update(extra)
        return env

    def launch(self, **extra):
        p = subprocess.run([str(SCRIPT), str(self.root / "game"), "Game.exe",
                            str(self.root / "prefix"), str(self.root / "launch.log")],
                           env=self.env(**extra), stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertTrue((self.root / "client.record").exists(), "the title was not started")
        return p

    def services(self):
        if not self.record.exists():
            return []
        return [line.split()[1:] for line in self.record.read_text().splitlines()]

    def listening(self, path):
        s = socket.socket(socket.AF_UNIX)
        try:
            s.connect(str(path))
            return True
        except OSError:
            return False
        finally:
            s.close()

    def stale_socket(self, path):
        s = socket.socket(socket.AF_UNIX)
        s.bind(str(path))
        s.close()
        self.assertTrue(stat.S_ISSOCK(os.lstat(path).st_mode))

    def start_service(self, directory):
        directory.mkdir(exist_ok=True)
        service = subprocess.Popen([str(self.cli / "xodus-service")],
                                   env=self.env(XODUS_RUNTIME_DIR=str(directory)),
                                   stdout=subprocess.DEVNULL, start_new_session=True)
        self.addCleanup(lambda: (service.kill(), service.wait()))
        deadline = time.monotonic() + 10
        while not self.listening(directory / "xodus.sock"):
            self.assertLess(time.monotonic(), deadline, "stub service did not start")
            time.sleep(0.05)

    def test_a_socket_left_in_the_override_directory_is_replaced(self):
        custom = self.root / "custom"
        custom.mkdir(mode=0o700)
        self.stale_socket(custom / "xodus.sock")
        self.launch(XODUS_RUNTIME_DIR=str(custom))
        self.assertEqual(self.services(), [[str(custom / "xodus.sock"), str(custom)]])
        self.assertTrue(self.listening(custom / "xodus.sock"))

    def test_a_service_on_another_socket_does_not_stand_in_for_this_one(self):
        # What `ferestre run` leaves running for another title.
        self.start_service(self.root / "scoped")
        self.launch()
        self.assertIn([str(self.xdg / "xodus.sock"), str(self.xdg)], self.services())
        self.assertTrue(self.listening(self.xdg / "xodus.sock"))

    def test_a_listening_service_is_reused(self):
        self.start_service(self.xdg)
        self.launch()
        self.assertEqual(len(self.services()), 1)

    def test_the_service_log_is_private_and_kept_in_the_runtime_directory(self):
        self.launch()
        log = self.xdg / "xodus-service.log"
        self.assertTrue(log.exists())
        self.assertEqual(stat.S_IMODE(log.stat().st_mode), 0o600)
        self.assertFalse((self.tmp / "xodus-service.log").exists())

    def test_an_old_world_readable_log_is_made_private(self):
        log = self.xdg / "xodus-service.log"
        log.write_text("old\n")
        log.chmod(0o644)
        self.launch()
        self.assertEqual(stat.S_IMODE(log.stat().st_mode), 0o600)


if __name__ == "__main__":
    unittest.main()

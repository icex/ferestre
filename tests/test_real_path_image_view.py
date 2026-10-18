#!/usr/bin/env python3
"""Exercise the actual private mount, image mapping, and cancellation path."""

import importlib.util
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HELPER = Path(__file__).resolve().parents[1] / "scripts/real-path-image-view.py"
spec = importlib.util.spec_from_file_location("image_view", HELPER)
view = importlib.util.module_from_spec(spec)
spec.loader.exec_module(view)


class ImageViewTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="ferestre-view-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.image = self.root / "Game.exe"
        self.image.write_bytes(b"encrypted package bytes")
        self.image.chmod(0o644)
        self.fd = os.memfd_create("Game.exe", flags=0)
        self.addCleanup(os.close, self.fd)
        os.write(self.fd, b"MZ" + b"licensed fixture image" * 200)
        self.mapping = f"{self.fd}:\\??\\Z:{str(self.image).replace('/', chr(92))}"

    def require_namespace(self):
        p = subprocess.run(["unshare", "--user", "--map-root-user", "--mount", "true"],
                           capture_output=True)
        if p.returncode:
            why = "host does not allow unprivileged mount namespaces"
            # These are the only tests of the private mounts, so where they
            # are meant to run (CI sets this) a skip would hide a regression
            # behind a green run.
            if os.environ.get("FERESTRE_REQUIRE_USERNS") == "1":
                self.fail(f"{why}, and FERESTRE_REQUIRE_USERNS=1 requires them: "
                          f"{p.stderr.decode(errors='replace').strip()}")
            self.skipTest(why)

    def launch(self, code):
        env = dict(os.environ, WINE_DLL_FILE_MAP=self.mapping, FERESTRE_IMAGE_VIEW="real-path")
        return subprocess.Popen([sys.executable, str(HELPER), "--", sys.executable, "-c", code],
                                cwd=self.root, env=env, pass_fds=(self.fd,),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def launch_with_channel(self, title):
        """Start `title` (a shell command) through the view as the window
        does: with the launch's end of a stop channel. Returns the view and
        the window's end."""
        ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(ours.close)
        prefix = self.root / "pfx"
        prefix.mkdir(exist_ok=True)
        env = dict(os.environ, WINE_DLL_FILE_MAP=self.mapping, FERESTRE_IMAGE_VIEW="real-path",
                   WINEPREFIX=str(prefix), FERESTRE_STOP_FD=str(theirs.fileno()))
        env.pop("STEAM_COMPAT_DATA_PATH", None)
        p = subprocess.Popen([sys.executable, str(HELPER), "--", "bash", "-c", title],
                             cwd=self.root, env=env, pass_fds=(self.fd, theirs.fileno()),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        theirs.close()

        def stop():
            if p.poll() is None:
                p.send_signal(signal.SIGTERM)
            p.communicate(timeout=15)
        self.addCleanup(stop)
        return p, ours

    def read_line(self, channel, limit=15):
        channel.settimeout(limit)
        line = b""
        while not line.endswith(b"\n"):
            chunk = channel.recv(1)
            if not chunk:
                break
            line += chunk
        return line

    def wait_for(self, path, p, limit=15):
        deadline = time.monotonic() + limit
        while not path.exists():
            if p.poll() is not None:
                self.fail(p.communicate()[1])
            self.assertLess(time.monotonic(), deadline, f"{path.name} never appeared")
            time.sleep(0.05)

    def fake_wine(self, server_folder):
        """A `wine` that writes down where it ran and what it was asked."""
        seen = self.root / "seen"
        wine = self.root / "bin/wine"
        wine.parent.mkdir()
        wine.write_text("#!/bin/sh\n"
                        f"{{ id -u; cat /tmp/.wine-$(id -u)/{server_folder}/marker; "
                        f'echo "$@"; echo "${{FERESTRE_STOP_FD:-unset}}"; }} > {seen}\n')
        wine.chmod(0o755)
        return wine, seen

    def test_a_close_request_runs_taskkill_beside_the_title_inside_the_view(self):
        self.require_namespace()
        server_folder = f"server-view-stop-{os.getpid()}"
        p, channel = self.launch_with_channel(
            f"mkdir -p /tmp/.wine-$(id -u)/{server_folder} && "
            f"echo inside > /tmp/.wine-$(id -u)/{server_folder}/marker && "
            # The title sees neither the channel nor its variable.
            "{ [ -z \"${FERESTRE_STOP_FD+set}\" ] && echo clean || echo leaked; } > channel && "
            "ls /proc/$$/fd > fds && "
            "touch ready && exec -a 'C:\\Games\\Halo\\HaloInfinite.exe' sleep 60")
        self.assertEqual(self.read_line(channel), b"ready\n")
        self.wait_for(self.root / "ready", p)
        self.assertEqual((self.root / "channel").read_text(), "clean\n")
        self.assertEqual(sorted((self.root / "fds").read_text().split()), ["0", "1", "2"],
                         "the title inherited more than its standard streams")
        self.assertFalse(Path(f"/tmp/.wine-{os.getuid()}/{server_folder}").exists(),
                         "the title's server folder is private to the view")

        wine, seen = self.fake_wine(server_folder)
        channel.sendall(b"close " + os.fsencode(wine) + b"\n")
        self.assertEqual(self.read_line(channel), b"asked\n")
        self.assertEqual(seen.read_text(),
                         f"{os.getuid()}\ninside\ntaskkill /IM HaloInfinite.exe\nunset\n",
                         "taskkill ran as the title's user, beside its server folder")
        # Asking is not ending: the window waits, and ends it itself.
        self.assertIsNone(p.poll())

    def test_a_view_with_nothing_to_ask_says_so(self):
        self.require_namespace()
        p, channel = self.launch_with_channel("touch ready && exec sleep 60")
        self.assertEqual(self.read_line(channel), b"ready\n")
        self.wait_for(self.root / "ready", p)
        wine, seen = self.fake_wine("unused")
        channel.sendall(b"close " + os.fsencode(wine) + b"\n")
        self.assertEqual(self.read_line(channel), b"none\n")
        self.assertFalse(seen.exists(), "taskkill ran with nothing to ask")
        # A wine the window did not name as a program is not run either.
        channel.sendall(b"close relative/wine\n")
        self.assertEqual(self.read_line(channel), b"none\n")

    def test_the_window_letting_go_of_the_channel_leaves_the_title_running(self):
        self.require_namespace()
        p, channel = self.launch_with_channel("touch ready && exec sleep 60")
        self.assertEqual(self.read_line(channel), b"ready\n")
        self.wait_for(self.root / "ready", p)
        channel.close()
        time.sleep(0.5)
        self.assertIsNone(p.poll(), "the view ended with the window")

    def test_native_reads_and_mappings_retain_the_real_path(self):
        self.require_namespace()
        p = self.launch("""
import json, mmap, os
from pathlib import Path
path=Path('Game.exe').resolve()
with path.open('rb') as f:
    image=mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    assert image[:2] == b'MZ'
    assert str(path) in Path('/proc/self/maps').read_text()
try:
    path.open('wb')
except OSError:
    pass
else:
    raise AssertionError('image bind is writable')
assert 'WINE_DLL_FILE_MAP' not in os.environ
assert 'FERESTRE_IMAGE_VIEW' not in os.environ
assert path.stat().st_mode & 0o777 == 0o644
print('mapped PE at its installed path')
""")
        out, err = p.communicate(timeout=15)
        self.assertEqual(p.returncode, 0, err)
        self.assertIn("mapped PE at its installed path", out)
        self.assertEqual(self.image.read_bytes(), b"encrypted package bytes")

    def test_rejects_paths_outside_the_installation(self):
        directory = tempfile.TemporaryDirectory(prefix="ferestre-view-outside-")
        self.addCleanup(directory.cleanup)
        outside = Path(directory.name) / "outside.exe"
        mapping = f"{self.fd}:\\??\\Z:{str(outside).replace('/', chr(92))}"
        outside.write_bytes(b"fixture")
        with self.assertRaises(ValueError):
            view.image_entries(mapping, self.root)
        alias = self.root / "alias.exe"
        alias.symlink_to(outside)
        mapping = f"{self.fd}:\\??\\Z:{str(alias).replace('/', chr(92))}"
        with self.assertRaises(ValueError):
            view.image_entries(mapping, self.root)

    def test_rejects_duplicate_targets_and_non_pe_descriptors(self):
        with self.assertRaises(ValueError):
            view.image_entries(self.mapping + "|" + self.mapping, self.root)
        os.pwrite(self.fd, b"XX", 0)
        with self.assertRaises(ValueError):
            view.image_entries(self.mapping, self.root)

    def test_cancellation_stops_detached_children_and_restores_the_view(self):
        self.require_namespace()
        p = self.launch("""
import os, subprocess, sys, time
from pathlib import Path
child=subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(60)'], start_new_session=True)
Path('child.pid').write_text(str(child.pid))
print('ready',flush=True)
time.sleep(60)
""")
        try:
            deadline = time.monotonic() + 10
            while not (self.root / "child.pid").exists() and time.monotonic() < deadline:
                if p.poll() is not None:
                    self.fail(p.communicate()[1])
                time.sleep(0.05)
            self.assertTrue((self.root / "child.pid").exists())
            child = int((self.root / "child.pid").read_text())
            p.send_signal(signal.SIGTERM)
            _, err = p.communicate(timeout=10)
            self.assertEqual(p.returncode, 130, err)
            self.assertFalse(Path(f"/proc/{child}").exists(), "detached child survived cleanup")
            self.assertEqual(self.image.read_bytes(), b"encrypted package bytes")
        finally:
            if p.poll() is None:
                p.send_signal(signal.SIGTERM)
                p.communicate(timeout=10)

    def test_normal_launcher_exit_keeps_its_detached_game_alive(self):
        self.require_namespace()
        p = self.launch("""
import subprocess, sys
subprocess.Popen([sys.executable, '-c',
    "import time;from pathlib import Path;time.sleep(0.2);assert Path('Game.exe').read_bytes()[:2]==b'MZ';Path('handoff.finished').write_text('done')"],
    start_new_session=True)
""")
        _, err = p.communicate(timeout=10)
        self.assertEqual(p.returncode, 0, err)
        self.assertTrue((self.root / "handoff.finished").exists(), "launcher cleanup killed its game")
        self.assertEqual(self.image.read_bytes(), b"encrypted package bytes")


class StopChannelTests(unittest.TestCase):
    """The view's half of Stop, without namespaces."""

    def test_images_are_named_as_taskkill_matches_them(self):
        self.assertEqual(view.image_name(b"C:\\Games\\Halo\\HaloInfinite.exe\0-x\0"),
                         "HaloInfinite.exe")
        self.assertEqual(view.image_name(b"Z:\\g\\Game.EXE\0"), "Game.EXE")
        for other in (b"C:\\windows\\system32\\services.exe\0", b"C:\\windows\\Steam.exe\0",
                      b"/opt/proton/files/bin/wineserver\0", b"/usr/bin/bash\0", b""):
            self.assertIsNone(view.image_name(other), other)

    def test_the_request_runs_as_the_first_process_without_loader_variables(self):
        env = view.environment(b"WINEPREFIX=/p/pfx/\0WINEFSYNC=1\0WINESERVERSOCKET=16\0"
                               b"WINEPRELOADRESERVE=1-2\0WINELOADERNOEXEC=1\0FERESTRE_STOP_FD=5\0"
                               b"BYTES=\xff\0junk\0")
        self.assertEqual(env, {"WINEPREFIX": "/p/pfx/", "WINEFSYNC": "1",
                               "BYTES": os.fsdecode(b"\xff")})
        argv, request_env = view.close_command("/opt/wine", [
            (10, "Game.exe", env), (11, "game.EXE", {}), (12, "Helper.exe", {})])
        self.assertEqual(argv, ["/opt/wine", "taskkill", "/IM", "Game.exe", "/IM", "Helper.exe"])
        self.assertNotIn("/F", argv)
        self.assertEqual(request_env, env)
        self.assertIsNone(view.close_command("/opt/wine", []))

    def test_only_wine_processes_are_found(self):
        with tempfile.TemporaryDirectory(prefix="ferestre-view-proc-") as directory:
            proc = Path(directory)
            for pid, cmdline, environ in (
                    (30, b"C:\\Game\\Game.exe\0", b"WINEPREFIX=/p\0"),
                    (20, b"C:\\windows\\system32\\explorer.exe\0", b"WINEPREFIX=/p\0"),
                    (40, b"C:\\Game\\Game.exe\0", b"PATH=/bin\0"),
                    (50, b"/usr/bin/sleep\0", b"WINEPREFIX=/p\0")):
                (proc / str(pid)).mkdir()
                (proc / str(pid) / "cmdline").write_bytes(cmdline)
                (proc / str(pid) / "environ").write_bytes(environ)
            found = view.wine_processes([50, 40, 30, 20, 60, 30], proc)
            self.assertEqual(found, [(30, "Game.exe", {"WINEPREFIX": "/p"})])

    def test_requests_are_answered_until_the_window_lets_go(self):
        ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        asked = []
        server = threading.Thread(target=view.serve_stop,
                                  args=(theirs, lambda wine: asked.append(wine) or len(asked) == 1))
        server.start()
        stream = ours.makefile("rb")
        self.assertEqual(stream.readline(), b"ready\n")
        ours.sendall(b"close /opt/wine\n")
        self.assertEqual(stream.readline(), b"asked\n")
        ours.sendall(b"something else\nclose /opt/other wine\n")
        self.assertEqual(stream.readline(), b"none\n")
        stream.close()
        ours.close()
        server.join(5)
        self.assertFalse(server.is_alive(), "the view kept serving a closed channel")
        self.assertEqual(asked, ["/opt/wine", "/opt/other wine"])
        theirs.close()

    def test_only_a_unix_stream_socket_is_taken_as_the_channel(self):
        for value in (None, "", "x", "1", "-4"):
            self.assertIsNone(view.stop_channel(value), value)
        read, write = os.pipe()
        self.addCleanup(os.close, read)
        self.addCleanup(os.close, write)
        self.assertIsNone(view.stop_channel(str(read)))
        datagram = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        self.addCleanup(datagram.close)
        self.assertIsNone(view.stop_channel(str(datagram.fileno())))
        os.fstat(datagram.fileno())  # left open for its owner
        ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(ours.close)
        os.set_inheritable(theirs.fileno(), True)
        channel = view.stop_channel(str(theirs.detach()))
        self.addCleanup(channel.close)
        self.assertIsNotNone(channel)
        self.assertFalse(channel.get_inheritable(), "the title would inherit the channel")


if __name__ == "__main__":
    unittest.main()

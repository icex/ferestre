#!/usr/bin/env python3
"""The real-path view runs the launch as the caller, not as namespace root.

Wine names its server directory /tmp/.wine-<getuid()> and refuses one it does
not own, so a launch that sees uid 0 splits from the rest of the session's
wineservers. The mounts that need root must still be visible to it.
"""

import fcntl
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import unittest

HELPER = Path(__file__).resolve().parents[1] / "scripts/real-path-image-view.py"


class CallerIdsTests(unittest.TestCase):
    def setUp(self):
        p = subprocess.run(["unshare", "--user", "--map-root-user", "--mount", "true"],
                           capture_output=True)
        if p.returncode:
            self.skipTest("host does not allow unprivileged mount namespaces")
        directory = tempfile.TemporaryDirectory(prefix="ferestre-view-ids-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.image = self.root / "Game.exe"
        self.image.write_bytes(b"encrypted package bytes")
        self.fd = os.memfd_create("Game.exe", flags=0)
        self.addCleanup(os.close, self.fd)
        os.write(self.fd, b"MZ" + b"licensed fixture image" * 50)
        self.mapping = f"{self.fd}:\\??\\Z:{str(self.image).replace('/', chr(92))}"
        self.server_base = Path(f"/tmp/.wine-{os.getuid()}")

    def helper(self, *launch, **extra):
        # No prefix of the developer's own: the launch would wait for its
        # sessions.
        env = {k: v for k, v in os.environ.items()
               if k not in ("WINEPREFIX", "STEAM_COMPAT_DATA_PATH")}
        env.update(WINE_DLL_FILE_MAP=self.mapping, FERESTRE_IMAGE_VIEW="real-path", **extra)
        return [sys.executable, str(HELPER), "--", *launch], env

    def outside_server(self):
        """A lock held the way a wineserver for the fixture prefix holds it."""
        prefix = self.root / "compat" / "pfx"
        prefix.mkdir(parents=True)
        self.server_base.mkdir(mode=0o700, exist_ok=True)
        info = prefix.stat()
        server = self.server_base / f"server-{info.st_dev:x}-{info.st_ino:x}"
        server.mkdir(mode=0o700)
        self.addCleanup(shutil.rmtree, server, True)
        fd = os.open(server / "lock", os.O_RDWR | os.O_CREAT, 0o600)
        held = [fd]
        self.addCleanup(lambda: os.close(fd) if held else None)
        # An open file description lock lasts exactly as long as this
        # descriptor, which release() closes.
        fcntl.fcntl(fd, fcntl.F_OFD_SETLK, struct.pack("hhqqi4x", fcntl.F_WRLCK, os.SEEK_SET, 0, 1, 0))

        def release():
            held.remove(fd)
            os.close(fd)
        return str(self.root / "compat"), release

    def test_the_launch_sees_the_callers_ids_and_the_private_view(self):
        owned = self.root / "owned"
        owned.mkdir()
        env = dict(os.environ, WINE_DLL_FILE_MAP=self.mapping, FERESTRE_IMAGE_VIEW="real-path")
        code = """
import os
from pathlib import Path
print(os.getuid(), os.getgid(), os.geteuid())
# What wineserver checks before using its directory.
print(os.stat('owned').st_uid == os.getuid())
print(Path('Game.exe').read_bytes()[:2] == b'MZ')
"""
        p = subprocess.run([sys.executable, str(HELPER), "--", sys.executable, "-c", code],
                           cwd=self.root, env=env, pass_fds=(self.fd,),
                           capture_output=True, text=True, timeout=30)
        self.assertEqual(p.returncode, 0, p.stderr)
        ids, owns, mapped = p.stdout.splitlines()
        self.assertEqual(ids, f"{os.getuid()} {os.getgid()} {os.getuid()}")
        self.assertEqual(owns, "True", "the caller's own directory looks foreign")
        self.assertEqual(mapped, "True", "the private image view is not visible")
        self.assertEqual(self.image.read_bytes(), b"encrypted package bytes")

    def test_the_launch_cannot_undo_the_private_view(self):
        env = dict(os.environ, WINE_DLL_FILE_MAP=self.mapping, FERESTRE_IMAGE_VIEW="real-path")
        p = subprocess.run([sys.executable, str(HELPER), "--", "umount", str(self.image)],
                           cwd=self.root, env=env, pass_fds=(self.fd,),
                           capture_output=True, text=True, timeout=30)
        self.assertNotEqual(p.returncode, 0, "the launch could unmount its licensed images")

    def test_the_launch_cannot_reach_a_wine_server_outside_the_view(self):
        # Wine's server opens files by the path its client sends. One started
        # outside the view resolves those paths to the encrypted package, so
        # the launch must not see, and join, the servers under the caller's
        # real /tmp/.wine-<uid>.
        self.server_base.mkdir(mode=0o700, exist_ok=True)
        marker = self.server_base / f"ferestre-view-marker-{os.getpid()}"
        marker.mkdir()
        self.addCleanup(marker.rmdir)
        code = f"""
import os, stat
base = '/tmp/.wine-{os.getuid()}'
info = os.lstat(base)
print({marker.name!r} in os.listdir(base))
print(stat.S_ISDIR(info.st_mode), info.st_uid == os.getuid(), oct(stat.S_IMODE(info.st_mode)))
os.mkdir(base + '/server-test')
"""
        argv, env = self.helper(sys.executable, "-c", code)
        p = subprocess.run(argv, cwd=self.root, env=env, pass_fds=(self.fd,),
                           capture_output=True, text=True, timeout=30)
        self.assertEqual(p.returncode, 0, p.stderr)
        seen, usable = p.stdout.splitlines()
        self.assertEqual(seen, "False", "the launch can reach the caller's outside Wine servers")
        self.assertEqual(usable, "True True 0o700", "Wine would refuse the launch's server directory")
        self.assertTrue(marker.is_dir())
        self.assertFalse((self.server_base / "server-test").exists(),
                         "the launch's server directory leaked outside the view")

    def test_the_launch_waits_for_a_session_outside_the_view_to_end(self):
        compat, release = self.outside_server()
        started = self.root / "started"
        argv, env = self.helper(sys.executable, "-c", f"open({str(started)!r}, 'w').close()",
                                STEAM_COMPAT_DATA_PATH=compat)
        p = subprocess.Popen(argv, cwd=self.root, env=env, pass_fds=(self.fd,),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            time.sleep(1.5)
            self.assertFalse(started.exists(),
                             "the launch started while another server held the prefix")
            release()
            _, err = p.communicate(timeout=30)
        finally:
            if p.poll() is None:
                p.kill()
                p.wait()
        self.assertEqual(p.returncode, 0, err)
        self.assertTrue(started.exists())
        self.assertNotIn("still running", err)

    def test_a_session_that_does_not_end_does_not_hold_the_launch_forever(self):
        compat, _ = self.outside_server()
        argv, env = self.helper(sys.executable, "-c", "print('launched')",
                                STEAM_COMPAT_DATA_PATH=compat)
        begun = time.monotonic()
        p = subprocess.run(argv, cwd=self.root, env=env, pass_fds=(self.fd,),
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(p.stdout, "launched\n")
        self.assertIn("still running outside the private image view", p.stderr)
        self.assertLess(time.monotonic() - begun, 30)


if __name__ == "__main__":
    unittest.main()

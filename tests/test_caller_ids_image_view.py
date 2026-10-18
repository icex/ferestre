#!/usr/bin/env python3
"""The real-path view runs the launch as the caller, not as namespace root.

Wine names its server directory /tmp/.wine-<getuid()> and refuses one it does
not own, so a launch that sees uid 0 splits from the rest of the session's
wineservers. The mounts that need root must still be visible to it.
"""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
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


if __name__ == "__main__":
    unittest.main()

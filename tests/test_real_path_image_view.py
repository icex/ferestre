#!/usr/bin/env python3
"""Exercise the actual private mount, image mapping, and cancellation path."""

import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
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
            self.skipTest("host does not allow unprivileged mount namespaces")

    def launch(self, code):
        env = dict(os.environ, WINE_DLL_FILE_MAP=self.mapping, FERESTRE_IMAGE_VIEW="real-path")
        return subprocess.Popen([sys.executable, str(HELPER), "--", sys.executable, "-c", code],
                                cwd=self.root, env=env, pass_fds=(self.fd,),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

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


if __name__ == "__main__":
    unittest.main()

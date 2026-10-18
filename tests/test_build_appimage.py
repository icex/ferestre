#!/usr/bin/env python3
"""The AppImage build refuses to produce a package that cannot run.

release.yml publishes whatever packaging/appimage/build-appimage.sh produces, and
the script's self-test is the only check in between that the packaged command
starts at all. These build real AppImages around stand-in launchers, so they
need the pinned appimagetool and type-2 runtime: from the cache when they are
there, otherwise downloaded and checked against the hashes the script pins.
"""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / 'packaging/appimage/build-appimage.sh'


def executable(path, text):
    path.write_text(text)
    path.chmod(0o755)
    return path


class BuildAppImageTests(unittest.TestCase):
    def build(self, root, launcher, *extra):
        env = {k: v for k, v in os.environ.items()
               if not k.startswith('FERESTRE_') or k in
               ('FERESTRE_CACHE_DIR', 'FERESTRE_APPIMAGETOOL', 'FERESTRE_APPIMAGE_RUNTIME')}
        env.update(NO_COLOR='1', FERESTRE_BUILD_DIR=str(root / 'build'),
                   # An empty string is "autodetect"; a GUI from somebody's own
                   # target/ would change what is being tested.
                   FERESTRE_GUI_BIN=str(executable(root / 'no-gui', '#!/bin/sh\nexit 0\n')))
        return subprocess.run(
            [str(SCRIPT), '--bin', str(launcher), '--version', '9.9.9-test',
             '--out', str(root / 'out'), *extra],
            env=env, cwd=root, capture_output=True, text=True, timeout=600)

    def test_a_launcher_that_answers_version_builds(self):
        with tempfile.TemporaryDirectory(prefix='ferestre appimage ') as tmp:
            root = Path(tmp)
            launcher = executable(root / 'ferestre',
                                  '#!/bin/sh\n[ "$1" = version ] && echo "ferestre 9.9.9-test"\n')
            result = self.build(root, launcher)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((root / 'out/ferestre-9.9.9-test-x86_64.AppImage').is_file())

    def test_a_launcher_that_cannot_start_fails_the_build(self):
        with tempfile.TemporaryDirectory(prefix='ferestre appimage ') as tmp:
            root = Path(tmp)
            launcher = executable(root / 'ferestre',
                                  '#!/bin/sh\necho "error while loading shared libraries" >&2\nexit 127\n')
            result = self.build(root, launcher)
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertIn("does not run 'version'", result.stderr)

    def test_a_client_without_its_service_fails_the_build(self):
        with tempfile.TemporaryDirectory(prefix='ferestre appimage ') as tmp:
            root = Path(tmp)
            launcher = executable(root / 'ferestre', '#!/bin/sh\necho ok\n')
            client_dir = root / 'client'
            client_dir.mkdir()
            client = executable(client_dir / 'xodus-cli', '#!/bin/sh\nexit 0\n')
            result = self.build(root, launcher, '--client', str(client), '--skip-self-test')
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertIn('no executable xodus-service beside it', result.stderr)
            self.assertFalse((root / 'out/ferestre-9.9.9-test-x86_64.AppImage').exists())

            executable(client_dir / 'xodus-service', '#!/bin/sh\nexit 0\n')
            result = self.build(root, launcher, '--client', str(client), '--skip-self-test')
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()

#!/usr/bin/env python3
"""No URL of an NVIDIA file is ever committed.

The only place a download URL for nvngx_dlssnr.dll may come from is the
person's own settings or environment, never this repository. The guard's own
source holds no such URL: its self-check assembles one at run time.
"""
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]
PATTERN = re.compile(r'https?://\S*(nvngx|dlssnr\.dll|nvidia\.com/\S*dlss)', re.I)


def tracked_files():
    out = subprocess.check_output(['git', '-C', str(ROOT), 'ls-files', '-z'])
    return [ROOT / name for name in out.decode().split('\0') if name]


class NoNvidiaUrlTests(unittest.TestCase):
    def test_the_pattern_catches_a_dll_url(self):
        self.assertRegex('see https://example.invalid/' + 'nvngx_dlssnr.dll here', PATTERN)
        self.assertRegex('https://www.' + 'nvidia.com/x/' + 'DLSS', PATTERN)
        self.assertNotRegex('https://github.com/TheAutomatic/DLSSNR-AMD.git', PATTERN)

    def test_no_tracked_file_has_a_url_of_an_nvidia_file(self):
        found = []
        for path in tracked_files():
            if not path.is_file() or path.is_symlink():
                continue
            try:
                text = path.read_text(encoding='utf-8')
            except (UnicodeDecodeError, OSError):
                continue
            for number, line in enumerate(text.splitlines(), 1):
                if PATTERN.search(line):
                    found.append(f'{path.relative_to(ROOT)}:{number}')
        self.assertEqual(found, [])


if __name__ == '__main__':
    unittest.main()

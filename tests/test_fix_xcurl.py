#!/usr/bin/env python3
"""scripts/fix-xcurl.sh against small PE fixtures, offline.

The DLLs here are built in the test: a header, one section, and the export or
import tables the script reads. That is enough to tell Microsoft's XCurl (it
exports xcurl_*) from a plain libcurl, and to check what a title imports.
"""

import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
import zipfile

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/fix-xcurl.sh"


def pe(exports=(), imports=(), filler=b""):
    """A PE32+ image exporting `exports` and importing `imports` from XCurl.dll."""
    base = 0x1000
    sec = bytearray()

    def put(data):
        offset = len(sec)
        sec.extend(data)
        return base + offset

    export_rva = import_rva = 0
    if exports:
        names = [put(n.encode() + b"\0") for n in exports]
        table = put(b"".join(struct.pack("<I", n) for n in names))
        export_rva = put(struct.pack("<IIHHIIIIIII", 0, 0, 0, 0, 0, 1,
                                     len(exports), len(exports), 0, table, 0))
    if imports:
        hints = [put(b"\0\0" + n.encode() + b"\0") for n in imports]
        thunks = put(b"".join(struct.pack("<Q", h) for h in hints) + b"\0" * 8)
        dll = put(b"XCurl.dll\0")
        import_rva = put(struct.pack("<IIIII", thunks, 0, 0, dll, thunks) + b"\0" * 20)
    put(filler)
    sec.extend(b"\0" * (-len(sec) % 0x200))

    header = bytearray(0x200)
    header[:2] = b"MZ"
    struct.pack_into("<I", header, 0x3c, 0x40)
    header[0x40:0x44] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", header, 0x44, 0x8664, 1, 0, 0, 0, 240, 0x2022)
    opt = 0x58
    struct.pack_into("<H", header, opt, 0x20b)
    struct.pack_into("<II", header, opt + 112, export_rva, 40 if exports else 0)
    struct.pack_into("<II", header, opt + 120, import_rva, 40 if imports else 0)
    section = opt + 240
    header[section:section + 8] = b".data\0\0\0"
    struct.pack_into("<IIII", header, section + 8, len(sec), base, len(sec), 0x200)
    return bytes(header) + bytes(sec)


MICROSOFT_V1 = pe(["curl_easy_init", "curl_easy_perform", "xcurl_global_init_mem"], filler=b"v1")
MICROSOFT_V2 = pe(["curl_easy_init", "curl_easy_perform", "xcurl_global_init_mem"], filler=b"v2")
LIBCURL = pe(["curl_easy_init", "curl_easy_perform", "curl_easy_setopt"], filler=b"libcurl")
GAME = pe(imports=["curl_easy_init", "curl_easy_perform"])


class FixXcurlTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="ferestre-xcurl-test-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.game = self.root / "game"
        self.game.mkdir()
        (self.game / "Game.exe").write_bytes(GAME)
        self.dll = self.game / "XCurl.dll"
        self.backup = self.game / "XCurl.dll.microsoft"
        self.fetched = self.root / "fetched/bin"
        self.fetched.mkdir(parents=True)
        (self.fetched / "libcurl-x64.dll").write_bytes(LIBCURL)
        (self.fetched / "curl-ca-bundle.crt").write_text("fixture bundle\n")
        # A curl that fails, so a test that should stay offline cannot pass by
        # fetching, and one that records what it was asked for.
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.curl_calls = self.root / "curl.calls"
        stub = self.bin / "curl"
        stub.write_text('#!/bin/sh\necho "$@" >> "$CURL_CALLS"\n'
                        'if [ -n "${CURL_SERVES:-}" ]; then\n'
                        '  while [ $# -gt 1 ]; do [ "$1" = -o ] && cp "$CURL_SERVES" "$2"; shift; done\n'
                        '  exit 0\nfi\nexit 22\n')
        stub.chmod(0o755)

    def run_script(self, *args, libcurl=True, **extra):
        env = {k: v for k, v in os.environ.items() if k not in ("LIBCURL", "BEDROCK_DIR")}
        env.update(HOME=str(self.root), XODUS_GAMES_DIR=str(self.root / "games"),
                   PATH=f"{self.bin}:{os.environ['PATH']}", CURL_CALLS=str(self.curl_calls))
        if libcurl:
            env["LIBCURL"] = str(self.fetched / "libcurl-x64.dll")
        env.update(extra)
        return subprocess.run([str(SCRIPT), *args, str(self.game)], env=env,
                              capture_output=True, text=True)

    def test_a_first_run_substitutes_and_keeps_microsofts_build(self):
        self.dll.write_bytes(MICROSOFT_V1)
        p = self.run_script()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(self.dll.read_bytes(), LIBCURL)
        self.assertEqual(self.backup.read_bytes(), MICROSOFT_V1)
        self.assertEqual((self.game / "curl-ca-bundle.crt").read_text(), "fixture bundle\n")

    def test_an_update_that_brings_back_a_new_microsoft_build_is_substituted_again(self):
        self.dll.write_bytes(MICROSOFT_V2)
        self.backup.write_bytes(MICROSOFT_V1)
        p = self.run_script()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(self.dll.read_bytes(), LIBCURL, "the new Microsoft build was left in place")
        self.assertEqual(self.backup.read_bytes(), MICROSOFT_V2, "the backup is the outdated build")
        p = self.run_script("--restore")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(self.dll.read_bytes(), MICROSOFT_V2)

    def test_an_already_substituted_dll_is_left_alone(self):
        self.dll.write_bytes(LIBCURL)
        self.backup.write_bytes(MICROSOFT_V1)
        p = self.run_script()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("already substituted", p.stdout)
        self.assertEqual(self.backup.read_bytes(), MICROSOFT_V1)

    def test_a_truncated_dll_from_an_interrupted_run_is_repaired(self):
        self.dll.write_bytes(LIBCURL[:0x210])
        self.backup.write_bytes(MICROSOFT_V1)
        p = self.run_script()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(self.dll.read_bytes(), LIBCURL)
        self.assertEqual(self.backup.read_bytes(), MICROSOFT_V1, "the good backup was overwritten")

    def test_a_failed_copy_is_reported_and_leaves_the_game_as_it_was(self):
        if os.geteuid() == 0:
            self.skipTest("root ignores directory permissions")
        self.dll.write_bytes(MICROSOFT_V1)
        self.game.chmod(0o555)
        self.addCleanup(self.game.chmod, 0o755)
        p = self.run_script()
        self.assertNotEqual(p.returncode, 0, p.stdout)
        self.assertEqual(self.dll.read_bytes(), MICROSOFT_V1)
        self.assertFalse(self.backup.exists())

    def test_a_given_libcurl_is_used_without_any_download(self):
        self.dll.write_bytes(MICROSOFT_V1)
        p = self.run_script()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertFalse(self.curl_calls.exists(), "downloaded although LIBCURL was given")

    def test_a_download_that_does_not_match_the_pinned_checksum_is_refused(self):
        self.dll.write_bytes(MICROSOFT_V1)
        archive = self.root / "served.zip"
        with zipfile.ZipFile(archive, "w") as z:
            z.writestr("curl-x-win64-mingw/bin/libcurl-x64.dll", LIBCURL)
        p = self.run_script(libcurl=False, CURL_SERVES=str(archive))
        self.assertNotEqual(p.returncode, 0, p.stdout)
        self.assertIn("checksum", p.stderr)
        self.assertEqual(self.dll.read_bytes(), MICROSOFT_V1)
        url = self.curl_calls.read_text()
        self.assertRegex(url, r"https://curl\.se/windows/dl-[0-9._]+/curl-[0-9._]+-win64-mingw\.zip")
        self.assertNotIn("latest", url)

    def test_a_missing_import_refuses_the_swap(self):
        self.dll.write_bytes(MICROSOFT_V1)
        (self.fetched / "libcurl-x64.dll").write_bytes(pe(["curl_easy_init"], filler=b"short"))
        p = self.run_script()
        self.assertNotEqual(p.returncode, 0, p.stdout)
        self.assertIn("curl_easy_perform", p.stdout)
        self.assertEqual(self.dll.read_bytes(), MICROSOFT_V1)

    def test_restore_refuses_a_backup_that_is_not_microsofts(self):
        self.dll.write_bytes(LIBCURL)
        self.backup.write_bytes(MICROSOFT_V1[:0x210])
        p = self.run_script("--restore")
        self.assertNotEqual(p.returncode, 0, p.stdout)
        self.assertEqual(self.dll.read_bytes(), LIBCURL)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Exercise doctor with a client binary the loader cannot start, the way the
real one fails without WebKitGTK (issue 8), and with the library in place."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

WEBKIT = "libwebkit2gtk-4.1.so.0"
FIXTURE = "libferestre_client_fixture.so.1"


def system_libc():
    """Where this system's C library is, as the loader finds it."""
    probe = subprocess.run(["ldd", "/bin/sh"], capture_output=True, text=True,
                           env=dict(os.environ, LC_ALL="C"), check=True)
    for line in probe.stdout.splitlines():
        name, _, rest = line.strip().partition(" => ")
        if name == "libc.so.6":
            return Path(rest.split()[0]).resolve()
    raise RuntimeError("could not find libc.so.6")


def build_client(home, soname):
    """An ELF named xodus-cli that needs `soname`, with the library built
    beside it in lib/ and not on any search path.

    The client is linked with -z nodefaultlib and a run path holding only a
    link to libc, so the loader never looks in the system's own directories
    or its cache for it. Its libraries are found exactly where this test puts
    them, whatever the machine running it has installed: borrowing WebKit's
    soname then does not mean finding the real WebKit."""
    lib = home / "lib"
    client = home / "client"
    libc = home / "libc"
    lib.mkdir()
    client.mkdir()
    libc.mkdir()
    (libc / "libc.so.6").symlink_to(system_libc())
    (home / "dependency.c").write_text("int ferestre_client_fixture(void) { return 0; }\n")
    (home / "main.c").write_text("extern int ferestre_client_fixture(void);\n"
                                 "int main(void) { return ferestre_client_fixture(); }\n")
    subprocess.run(["cc", "-shared", "-fPIC", f"-Wl,-soname,{soname}",
                    "-o", lib / soname, home / "dependency.c"], check=True)
    subprocess.run(["cc", "-o", client / "xodus-cli", home / "main.c", lib / soname,
                    "-Wl,-z,nodefaultlib", f"-Wl,-rpath,{libc}"], check=True)
    return client / "xodus-cli", lib


class ClientDependencies(unittest.TestCase):
    def setUp(self):
        self.binary = Path(os.environ.get("FERESTRE_BINARY", "target/debug/ferestre")).resolve()
        self.temporary = tempfile.TemporaryDirectory(prefix="ferestre-client-deps-")
        self.home = Path(self.temporary.name)
        self.env = dict(os.environ, HOME=str(self.home),
                        XODUS_GAMES_DIR=str(self.home / "games"),
                        XODUS_CLI_DIR=str(self.home / "client"),
                        XODUS_PROTON_DIR=str(self.home / "runtime"),
                        XODUS_STEAM_DIR=str(self.home / "steam"),
                        XDG_CONFIG_HOME=str(self.home / "config"),
                        XDG_STATE_HOME=str(self.home / "state"),
                        XDG_CACHE_HOME=str(self.home / "cache"),
                        FERESTRE_OS_RELEASE=str(self.home / "os-release"),
                        FERESTRE_OSTREE_MARKER=str(self.home / "ostree-booted"))
        self.env.pop("LD_LIBRARY_PATH", None)

    def tearDown(self):
        self.temporary.cleanup()

    def os_release(self, text):
        (self.home / "os-release").write_text(text)

    def client_check(self, env=None):
        result = subprocess.run([self.binary, "doctor", "--json"], env=env or self.env,
                                capture_output=True, text=True)
        checks = json.loads(result.stdout)["checks"]
        return next(c for c in checks if c["section"] == "client" and "libraries" in c["name"])

    def test_missing_webkit_names_the_package_for_the_distribution(self):
        client, lib = build_client(self.home, WEBKIT)
        (lib / WEBKIT).rename(self.home / WEBKIT)
        # The fixture borrows WebKit's soname, and the real library may well be
        # installed; the client must still not find it.
        ldd = subprocess.run(["ldd", client], capture_output=True, text=True,
                             env=dict(self.env, LC_ALL="C"))
        self.assertIn(f"{WEBKIT} => not found", ldd.stdout)

        # Linux Mint names Ubuntu and Debian as its parents.
        self.os_release('NAME="Linux Mint"\nID=linuxmint\nID_LIKE="ubuntu debian"\n')
        missing = self.client_check()
        self.assertEqual(missing["status"], "fail")
        self.assertIn("cannot start", missing["name"])
        self.assertEqual(missing["detail"], WEBKIT)
        self.assertIn("sudo apt install libwebkit2gtk-4.1-0", missing["hint"])

        self.os_release("ID=nobara\nID_LIKE=\"rhel centos fedora\"\n")
        self.assertIn("sudo dnf install webkit2gtk4.1", self.client_check()["hint"])
        self.os_release("ID=cachyos\nID_LIKE=arch\n")
        self.assertIn("sudo pacman -S webkit2gtk-4.1", self.client_check()["hint"])

        # An image-based Fedora layers it with rpm-ostree, and SteamOS gets
        # no command, because nothing installed into it lasts.
        (self.home / "ostree-booted").write_text("")
        self.assertNotIn("pacman", self.client_check()["hint"])
        self.os_release("ID=fedora\nVARIANT_ID=kinoite\n")
        atomic = self.client_check()
        self.assertIn("rpm-ostree install webkit2gtk4.1", atomic["hint"])
        self.assertNotIn("dnf", atomic["hint"])
        (self.home / "ostree-booted").unlink()
        self.os_release("ID=steamos\nID_LIKE=arch\n")
        steamos = self.client_check()
        self.assertNotIn("pacman", steamos["hint"])
        self.assertIn("read-only", steamos["hint"])

        # A distribution with no known package still names the library.
        self.os_release("ID=gentoo\n")
        unknown = self.client_check()
        self.assertEqual(unknown["status"], "fail")
        self.assertNotIn("sudo", unknown["hint"])
        self.assertIn(WEBKIT, unknown["hint"])

        # The library installed: the client's own environment finds it.
        (self.home / WEBKIT).rename(lib / WEBKIT)
        found = self.client_check(dict(self.env, LD_LIBRARY_PATH=str(lib)))
        self.assertEqual(found["status"], "ok")

    def test_an_unknown_library_is_named_without_a_package(self):
        client, lib = build_client(self.home, FIXTURE)
        (lib / FIXTURE).unlink()
        self.os_release("ID=debian\n")
        missing = self.client_check()
        self.assertEqual(missing["status"], "fail")
        self.assertEqual(missing["detail"], FIXTURE)
        self.assertNotIn("apt install", missing["hint"])

    def test_a_script_client_is_not_reported(self):
        client = self.home / "client/xodus-cli"
        client.parent.mkdir()
        client.write_text("#!/bin/sh\nexit 0\n")
        client.chmod(0o755)
        self.assertEqual(self.client_check()["status"], "ok")


if __name__ == "__main__":
    unittest.main()

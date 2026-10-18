#!/usr/bin/env python3
"""The window with a client the loader cannot start, as without WebKitGTK.

The client is a real ELF that needs libwebkit2gtk-4.1.so.0, built against a
throwaway library of that name which is kept off every search path. PackageKit
is tests/fake_packagekit.py on a private bus standing in for the system one;
its install moves the throwaway library onto the window's LD_LIBRARY_PATH,
which is what an installed package looks like to the loader.

    dbus-run-session -- tests/gui_client_deps.py [--binary target/debug/ferestre-gui] [--keep]

Three runs of the window: PackageKit refusing the install, then the library
arriving from elsewhere; no PackageKit at all; and an install started from
Sign in that carries on signing in. Fails at once on a system that has the
real library, where the fixture cannot be missing it.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

import gui_smoke as s
import fake_packagekit

WEBKIT = "libwebkit2gtk-4.1.so.0"
PACKAGE = "libwebkit2gtk-4.1-0"
BANNER = "The Xbox client cannot start: WebKitGTK is missing"

# Writes how it was run before it needs the library, so a run the loader
# allowed leaves a line behind and one it refused leaves nothing.
CLIENT = r'''#include <stdio.h>
#include <stdlib.h>
extern int ferestre_webkit_fixture(void);
int main(int argc, char **argv) {
    char path[4096];
    snprintf(path, sizeof path, "%s/client.log", getenv("HOME"));
    FILE *log = fopen(path, "a");
    for (int i = 1; log && i < argc; i++) fprintf(log, "%s%s", argv[i], i + 1 < argc ? " " : "");
    if (log) { fputc('\n', log); fclose(log); }
    return ferestre_webkit_fixture() + 1;
}
'''


def write(path, text, executable=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    if executable:
        path.chmod(0o755)


def calls(log):
    if not log.exists():
        return []
    return [json.loads(line) for line in log.read_text().splitlines()]


def deadline():
    return time.monotonic() + 20


class Window:
    """One window over its own HOME, fixture client and private system bus.
    `packagekit` is "install", "refuse", or None for a bus without it."""

    def __init__(self, binary, home, packagekit):
        self.home = home
        self.stage, self.lib, client = home / "stage", home / "lib", home / "client"
        for directory in (self.stage, self.lib, client):
            directory.mkdir(parents=True)
        write(home / "webkit.c", "int ferestre_webkit_fixture(void) { return 0; }\n")
        write(home / "client.c", CLIENT)
        subprocess.run(["cc", "-shared", "-fPIC", f"-Wl,-soname,{WEBKIT}", "-o", self.stage / WEBKIT,
                        home / "webkit.c"], check=True)
        subprocess.run(["cc", "-o", client / "xodus-cli", home / "client.c", self.stage / WEBKIT],
                       check=True)
        ldd = subprocess.run(["ldd", client / "xodus-cli"], capture_output=True, text=True,
                             env=dict(os.environ, LC_ALL="C", LD_LIBRARY_PATH=str(self.lib)))
        s.check(f"{WEBKIT} => not found" in ldd.stdout,
                f"the fixture client lacks {WEBKIT} (a system that has it cannot run this test)")

        # A runtime, so the window does not start by downloading one.
        write(home / "runtime/proton", "#!/bin/sh\nexit 0\n", True)
        write(home / "runtime/version", "0 ferestre-client-deps-fixture\n")
        write(home / "os-release", 'NAME="Debian GNU/Linux"\nID=debian\nVERSION_ID="13"\n')

        self.log = home / "packagekit.jsonl"
        if packagekit is None:
            address, daemon = fake_packagekit.start_bus(home)
            self.services = [daemon]
        else:
            address, self.services = fake_packagekit.start(
                home, self.log, install_move=(self.stage / WEBKIT, self.lib / WEBKIT),
                refuse=packagekit == "refuse")

        env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
                   XDG_STATE_HOME=str(home / "state"), XDG_CACHE_HOME=str(home / "cache"),
                   XODUS_CLI_DIR=str(client), XODUS_GAMES_DIR=str(home / "games"),
                   XODUS_PROTON_DIR=str(home / "runtime"), XODUS_STEAM_DIR=str(home / "steam"),
                   FERESTRE_OS_RELEASE=str(home / "os-release"),
                   FERESTRE_OSTREE_MARKER=str(home / "ostree-booted"),
                   LD_LIBRARY_PATH=str(self.lib), DBUS_SYSTEM_BUS_ADDRESS=address,
                   GTK_A11Y="atspi", HTTPS_PROXY="http://127.0.0.1:9",
                   HTTP_PROXY="http://127.0.0.1:9", ALL_PROXY="http://127.0.0.1:9",
                   FERESTRE_APPLICATION_ID=f"io.github.icex.ferestre.ClientDeps{packagekit or 'None'}")
        env.pop("APPIMAGE", None)
        self.stderr = home / "gui.log"
        self.gui = None
        try:
            with open(self.stderr, "wb") as stderr:
                self.gui = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL,
                                            stderr=stderr)
            self.app = s.find_app(self.gui.pid, deadline())
            self.frame = s.wait_for(
                self.app, lambda root: next(iter(s.find_all(root, role="frame", name="Ferestre")), None),
                "main window", deadline())
        except BaseException:
            self.stop(True)
            raise

    def has_text(self, text):
        return any(text in t for t in s.texts(self.frame))

    def wait_text(self, text, what):
        s.wait_for(self.frame, lambda root: any(text in t for t in s.texts(root)), what, deadline())

    def wait_idle(self, what):
        s.wait_for(self.frame, lambda root: any(n.get_name() == "No activity"
                                                for n in s.find_all(root, role="progress bar")),
                   what, deadline())

    def account_button(self):
        return s.wait_for(self.frame, lambda root: next(iter(
            s.find_all(root, role="button", name_contains="Not signed in")), None),
            "account button", deadline())

    def library_appears(self):
        """What an install from a terminal or a software centre does."""
        os.replace(self.stage / WEBKIT, self.lib / WEBKIT)

    def signed_in(self, root):
        log = self.home / "client.log"
        return log.exists() and any(line.split(" ")[0] in ("account-profiles", "login")
                                    for line in log.read_text().splitlines())

    def close(self):
        s.click_named(self.frame, "Close")
        s.check(self.gui.wait(timeout=10) == 0, "the window closes")

    def stop(self, failed):
        if self.gui is not None and self.gui.poll() is None:
            self.gui.terminate()
            self.gui.wait(timeout=10)
        for service in self.services:
            service.terminate()
            service.wait(timeout=10)
        if failed and self.stderr.exists():
            print(f"\n--- {self.stderr} ---\n{self.stderr.read_text(errors='replace')}--- end ---")


def refused_then_installed_elsewhere(w):
    print("PackageKit refuses, then the library arrives some other way")
    w.wait_text(BANNER, "the banner saying the client cannot start")
    w.wait_idle("startup work to finish")
    s.click_named(w.frame, "Install…")
    s.wait_for(w.frame, lambda root: any(c["method"] == "InstallPackages" for c in calls(w.log)),
               "an install request", deadline())
    made = calls(w.log)
    resolve = next(c for c in made if c["method"] == "Resolve")
    install = next(c for c in made if c["method"] == "InstallPackages")
    s.check(resolve["args"] == [1 << 3, [PACKAGE]],
            f"the Debian package is resolved, not installed ones ({resolve['args']})")
    s.check(install["args"] == [1 << 1, [f"{PACKAGE};2.54.0-1;amd64;fixture"]],
            f"the 64-bit package is installed once, trusted only ({install['args']})")
    hints = [c["args"][0] for c in made if c["method"] == "SetHints"]
    s.check(len(hints) == 2 and all("interactive=true" in h for h in hints),
            "both transactions may ask for a password")
    w.wait_text(f"Could not install {PACKAGE}: the installation was not authorised",
                "the refusal toast")
    w.wait_idle("the refused install to finish")
    s.check(w.has_text(BANNER), "the banner stays after a refusal", w.frame)

    # Installed from a terminal: Sign in checks again instead of refusing.
    w.library_appears()
    s.click(w.account_button())
    s.wait_for(w.frame, w.signed_in, "sign-in to run the client", deadline())
    s.check(not s.find_all(w.frame, role="dialog"), "no explanation once the client can start", w.frame)
    s.wait_for(w.frame, lambda root: not any(BANNER in t for t in s.texts(root)),
               "the banner to go", deadline())
    s.check(True, "a library installed elsewhere is noticed before refusing")
    w.wait_idle("sign-in to finish")
    w.close()


def without_packagekit(w):
    print("no PackageKit on the system bus")
    w.wait_text(BANNER, "the banner saying the client cannot start")
    w.wait_idle("startup work to finish")
    s.check(bool(s.find_all(w.frame, role="button", name="Details…")), "the banner offers details", w.frame)
    s.check(not s.find_all(w.frame, role="button", name="Install…"), "and no install", w.frame)
    s.click_named(w.frame, "Details…")
    w.wait_text("sudo apt install libwebkit2gtk-4.1-0", "the command in the explanation")
    s.check(bool(s.find_all(w.frame, role="button", name="Copy Command")), "the command can be copied", w.frame)
    s.check(not s.find_all(w.frame, role="button", name="Install"), "the dialog offers no install", w.frame)
    s.click_named(w.frame, "Cancel")
    s.wait_for(w.frame, lambda root: not s.find_all(root, role="dialog"), "the explanation to close", deadline())

    # The banner's button checks again before explaining.
    w.library_appears()
    s.click_named(w.frame, "Details…")
    w.wait_text("The Xbox client can start now", "the client to be found able to start")
    s.wait_for(w.frame, lambda root: not any(BANNER in t for t in s.texts(root)),
               "the banner to go", deadline())
    s.check(not s.find_all(w.frame, role="dialog"), "and nothing left to explain", w.frame)
    w.close()


def installed_from_sign_in(w):
    print("installing from Sign in, which then carries on")
    w.wait_text(BANNER, "the banner saying the client cannot start")
    w.wait_idle("startup work to finish")
    s.click(w.account_button())
    w.wait_text("Install the libwebkit2gtk-4.1-0 package now?", "the sign-in explanation")
    s.check(bool(s.find_all(w.frame, role="button", name="Copy Command")), "the command can be copied", w.frame)
    s.click_named(w.frame, "Install")
    s.wait_for(w.frame, lambda root: any(c["method"] == "InstallPackages" for c in calls(w.log)),
               "an install request", deadline())
    w.wait_text(f"Installed {PACKAGE}", "the installed toast")
    s.wait_for(w.frame, lambda root: not any(BANNER in t for t in s.texts(root)),
               "the banner to go once the library is there", deadline())
    # No second press: the sign-in that asked for the install continues.
    s.wait_for(w.frame, w.signed_in, "sign-in to carry on by itself", deadline())
    s.check(True, "sign-in carries on after the install")
    w.wait_idle("sign-in to finish")
    w.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/ferestre-gui")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    root = Path(tempfile.mkdtemp(prefix="fcd-"))
    s.Atspi.init()
    try:
        for name, packagekit, scenario in [
            ("refuse", "refuse", refused_then_installed_elsewhere),
            ("absent", None, without_packagekit),
            ("install", "install", installed_from_sign_in),
        ]:
            window = Window(binary, root / name, packagekit)
            failed = True
            try:
                scenario(window)
                failed = False
            finally:
                window.stop(failed)
        print("all client dependency GUI checks passed")
    finally:
        if args.keep:
            print("kept", root)
        else:
            shutil.rmtree(root)


if __name__ == "__main__":
    main()

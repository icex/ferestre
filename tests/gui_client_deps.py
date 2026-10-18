#!/usr/bin/env python3
"""The window with a client the loader cannot start, as without WebKitGTK.

The client is a real ELF that needs libwebkit2gtk-4.1.so.0, built against a
throwaway library of that name which is kept off every search path. PackageKit
is tests/fake_packagekit.py on a private bus standing in for the system one;
its install moves the throwaway library onto the window's LD_LIBRARY_PATH,
which is what an installed package looks like to the loader.

    dbus-run-session -- tests/gui_client_deps.py [--binary target/debug/ferestre-gui] [--keep]

Fails at once on a system that has the real library, where the fixture cannot
be missing it.
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/ferestre-gui")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    home = Path(tempfile.mkdtemp(prefix="fcd-"))
    gui = None
    services = []
    try:
        stage, lib, client = home / "stage", home / "lib", home / "client"
        for directory in (stage, lib, client):
            directory.mkdir()
        write(home / "webkit.c", "int ferestre_webkit_fixture(void) { return 0; }\n")
        write(home / "client.c", CLIENT)
        subprocess.run(["cc", "-shared", "-fPIC", f"-Wl,-soname,{WEBKIT}", "-o", stage / WEBKIT,
                        home / "webkit.c"], check=True)
        subprocess.run(["cc", "-o", client / "xodus-cli", home / "client.c", stage / WEBKIT], check=True)
        ldd = subprocess.run(["ldd", client / "xodus-cli"], capture_output=True, text=True,
                             env=dict(os.environ, LC_ALL="C", LD_LIBRARY_PATH=str(lib)))
        s.check(f"{WEBKIT} => not found" in ldd.stdout,
                f"the fixture client lacks {WEBKIT} (a system that has it cannot run this test)")

        # A runtime, so the window does not start by downloading one.
        write(home / "runtime/proton", "#!/bin/sh\nexit 0\n", True)
        write(home / "runtime/version", "0 ferestre-client-deps-fixture\n")
        write(home / "os-release", 'NAME="Debian GNU/Linux"\nID=debian\nVERSION_ID="13"\n')

        packagekit_log = home / "packagekit.jsonl"
        address, services = fake_packagekit.start(
            home, packagekit_log, install_move=(stage / WEBKIT, lib / WEBKIT))

        env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
                   XDG_STATE_HOME=str(home / "state"), XDG_CACHE_HOME=str(home / "cache"),
                   XODUS_CLI_DIR=str(client), XODUS_GAMES_DIR=str(home / "games"),
                   XODUS_PROTON_DIR=str(home / "runtime"), XODUS_STEAM_DIR=str(home / "steam"),
                   FERESTRE_OS_RELEASE=str(home / "os-release"), LD_LIBRARY_PATH=str(lib),
                   DBUS_SYSTEM_BUS_ADDRESS=address, GTK_A11Y="atspi",
                   HTTPS_PROXY="http://127.0.0.1:9", HTTP_PROXY="http://127.0.0.1:9",
                   ALL_PROXY="http://127.0.0.1:9",
                   FERESTRE_APPLICATION_ID="io.github.icex.ferestre.ClientDepsTest")
        env.pop("APPIMAGE", None)
        gui = subprocess.Popen([binary], env=env, stderr=subprocess.PIPE)
        s.Atspi.init()
        app = s.find_app(gui.pid, time.monotonic() + 20)
        frame = s.wait_for(app, lambda root: next(iter(s.find_all(root, role="frame", name="Ferestre")), None),
                           "main window", time.monotonic() + 20)
        s.wait_for(frame, lambda root: any(BANNER in t for t in s.texts(root)),
                   "the banner saying the client cannot start", time.monotonic() + 20)
        s.check(True, "the window says the client cannot start")
        s.wait_for(frame, lambda root: any(n.get_name() == "No activity" for n in s.find_all(root, role="progress bar")),
                   "startup work to finish", time.monotonic() + 20)

        # Signing in explains and offers the fix instead of running the client.
        button = s.wait_for(frame, lambda root: next(iter(s.find_all(root, role="button", name_contains="Not signed in")), None),
                            "account button", time.monotonic() + 20)
        s.click(button)
        s.wait_for(frame, lambda root: any("Install the libwebkit2gtk-4.1-0 package now?" in t for t in s.texts(root)),
                   "the sign-in explanation", time.monotonic() + 20)
        s.check(bool(s.find_all(frame, role="button", name="Copy Command")), "the command can be copied", frame)
        s.check(bool(s.find_all(frame, role="button", name="Install")), "PackageKit is offered", frame)
        s.click_named(frame, "Cancel")
        s.wait_for(frame, lambda root: not s.find_all(root, role="dialog"), "the explanation to close",
                   time.monotonic() + 20)
        s.check(not any(c["method"] == "InstallPackages" for c in calls(packagekit_log)),
                "nothing is installed without being asked")

        # The banner's button installs through PackageKit.
        s.click_named(frame, "Install…")
        s.wait_for(frame, lambda root: any(c["method"] == "InstallPackages" for c in calls(packagekit_log)),
                   "an install request", time.monotonic() + 20)
        made = calls(packagekit_log)
        resolve = next(c for c in made if c["method"] == "Resolve")
        install = next(c for c in made if c["method"] == "InstallPackages")
        s.check(resolve["args"] == [1 << 3, [PACKAGE]], f"the Debian package is resolved, not installed ones ({resolve['args']})")
        s.check(install["args"] == [1 << 1, [f"{PACKAGE};2.54.0-1;amd64;fixture"]],
                f"the 64-bit package is installed once, trusted only ({install['args']})")
        hints = [c["args"][0] for c in made if c["method"] == "SetHints"]
        s.check(len(hints) == 2 and all("interactive=true" in h for h in hints),
                "both transactions may ask for a password")

        s.wait_for(frame, lambda root: any(f"Installed {PACKAGE}" in t for t in s.texts(root)),
                   "the installed toast", time.monotonic() + 20)
        s.wait_for(frame, lambda root: not any(BANNER in t for t in s.texts(root)),
                   "the banner to go once the library is there", time.monotonic() + 20)
        s.check(True, "the banner goes once the client can start")

        # And signing in now runs the client.
        s.wait_for(frame, lambda root: any(n.get_name() == "No activity" for n in s.find_all(root, role="progress bar")),
                   "the install to finish", time.monotonic() + 20)
        s.click(button)
        signing_in = lambda root: (home / "client.log").exists() and any(
            line.split(" ")[0] in ("account-profiles", "login")
            for line in (home / "client.log").read_text().splitlines())
        s.wait_for(frame, signing_in, "the client to run for sign-in", time.monotonic() + 20)
        s.check(not s.find_all(frame, role="dialog"), "no explanation once the client can start", frame)

        s.click_named(frame, "Close")
        s.check(gui.wait(timeout=10) == 0, "the window closes")
        print("all client dependency GUI checks passed")
    finally:
        if gui is not None and gui.poll() is None:
            gui.terminate()
            gui.wait(timeout=10)
        for service in services:
            service.terminate()
            service.wait(timeout=10)
        if args.keep:
            print("kept", home)
        else:
            shutil.rmtree(home)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Isolation harness for WINE_DLL_FILE_MAP.

Asks one question with the game taken out of the picture: can Wine run a MAIN
EXECUTABLE that exists only behind a file descriptor?

Setup: garbage on disk at the target path, a real PE in a memfd, and
WINE_DLL_FILE_MAP pointing at the fd. If Wine runs the real program, the
mechanism works; if it falls back to start.exe hunting file associations, it
does not.

Runs in ~30s against a 330KB notepad instead of a 45GB game, which is what made
iterating on the loader patch practical.
"""
import os, subprocess, sys, time

PROTON = os.path.expanduser("~/.steam/steam/compatibilitytools.d/xodus/files")
WINE = f"{PROTON}/bin/wine"
REAL_PE = f"{PROTON}/lib/wine/x86_64-windows/notepad.exe"
PREFIX = os.path.expanduser("~/xbox-games/exp33-prefix")
FAKE_DIR = "/tmp/mapfd/win"
FAKE = f"{FAKE_DIR}/notepad.exe"

os.makedirs(FAKE_DIR, exist_ok=True)
real = open(REAL_PE, "rb").read()

# Decoy: same length, not a PE. Without the map, Wine must refuse this.
with open(FAKE, "wb") as f:
    f.write(b"\x00\xff" * (len(real) // 2))

# The real image, in memory only. No MFD_CLOEXEC: it has to survive exec.
fd = os.memfd_create("mapfd_test", 0)
os.write(fd, real)
os.set_inheritable(fd, True)

env = dict(os.environ)
env["WINE_DLL_FILE_MAP"] = f"{fd}:" + "\\??\\Z:" + FAKE.replace("/", "\\")
env["WINEPREFIX"] = PREFIX
env.pop("WINEDEBUG", None)

# argv must be the DOS form; Wine cannot resolve \??\ paths from the command line.
proc = subprocess.Popen([WINE, "Z:" + FAKE.replace("/", "\\")], env=env,
                        pass_fds=(fd,), stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL)
time.sleep(25)

ps = subprocess.run(["ps", "-eo", "comm"], capture_output=True, text=True).stdout
running = [line for line in ps.splitlines() if "notepad" in line.lower()]

print(f"wine alive        : {proc.poll() is None}")
print(f"notepad processes : {running}")
print("PASS" if running else f"FAIL (wine exit={proc.poll()})")

proc.kill()
subprocess.run([f"{PROTON}/bin/wineserver", "-k"], env=env, capture_output=True)
sys.exit(0 if running else 1)

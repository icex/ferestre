#!/usr/bin/env python3
"""Start Forza Horizon 5 and take it through its welcome screen, without a person.

    drive-fh5.py            launch, press Start Game, report what happened
    drive-fh5.py shot NAME  capture the game window now
    drive-fh5.py start      press Start Game on an already-running game
    drive-fh5.py kill       stop the game

Sibling of tools/drive-aoe.py and tools/auto-join.py, and it works the same
way: input goes through tools/wine_input_bot.c running inside the game's own
Wine prefix, so SendInput lands in the queue the game reads, and the window is
raised through KWin before a screenshot because Wine's SetForegroundWindow
tells the compositor nothing.

Why it exists: Forza's failure is thirty to sixty seconds into a launch that
takes another forty just to decrypt and start, and it is not always the same
failure. Watching for it by hand costs two minutes a look and gets the timing
wrong -- pressing Start Game a few seconds late made the crash look as though
Start Game had caused it, when it happens on its own just as often.
"""

import os
import re
import subprocess
import sys
import time
from pathlib import Path

HOME = Path.home()
GAMES = Path(os.environ.get("XODUS_GAMES_DIR", HOME / "xbox-games"))
SLUG = "fh5"
GAME_DIR = Path(os.environ.get("FH5_DIR", GAMES / SLUG))
PREFIX = Path(os.environ.get("WINEPREFIX", GAMES / f"{SLUG}-proton" / "pfx"))
TOOL = Path(
    os.environ.get(
        "XODUS_PROTON_DIR", HOME / ".local/share/Steam/compatibilitytools.d/xodus"
    )
) / "files"
WINE = TOOL / "bin" / "wine"
BOT = Path(os.environ.get("XODUS_BOT_EXE", GAME_DIR / "_bot.exe"))
OUT = Path(os.environ.get("FH5_OUT", "/tmp/ferestre-drive-fh5"))
LOG = GAMES / f"{SLUG}-launch.log"
REPO = Path(__file__).resolve().parent.parent

WINDOW = "Forza Horizon 5"
EXE = "ForzaHorizon5.exe"
VK_RETURN = "0D"

# The welcome screen is not up for another half-minute after the process is,
# and the prompt itself only appears once the profile load finishes.
WINDOW_TIMEOUT = 240
PRESS_AFTER = 45
PRESS_EVERY = 15
PATIENCE = 300

NOISE = re.compile(r"^(ntsync|radv|wineserver|wine:|WARNING|Proton)")


def die(message, *detail):
    print(f"!! {message}", file=sys.stderr)
    for line in detail:
        print(f"   {line}", file=sys.stderr)
    raise SystemExit(1)


def bot(args, timeout=120):
    """Run the input bot inside the game's prefix."""
    if not BOT.exists():
        die(
            f"no input bot at {BOT}",
            "build tools/wine_input_bot.c with a mingw compiler and point",
            "XODUS_BOT_EXE at the result:",
            "  x86_64-w64-mingw32-gcc -O2 -o bot.exe tools/wine_input_bot.c -luser32",
        )
    env = dict(os.environ, WINEPREFIX=str(PREFIX), WINEDEBUG="-all")
    try:
        p = subprocess.run(
            [str(WINE), str(BOT)] + args,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(GAME_DIR),
        )
    except subprocess.TimeoutExpired:
        return "TIMEOUT"
    return "\n".join(l for l in p.stdout.splitlines() if not NOISE.match(l))


def game_pid():
    out = subprocess.run(
        ["pgrep", "-f", EXE], capture_output=True, text=True
    ).stdout.split()
    return int(out[0]) if out else None


def uptime(pid):
    out = subprocess.run(
        ["ps", "-o", "etimes=", "-p", str(pid)], capture_output=True, text=True
    ).stdout.strip()
    return int(out) if out.isdigit() else 0


def window_rect():
    m = re.search(r"rect (-?\d+),(-?\d+) (\d+)x(\d+)", bot(["windows"]) or "")
    return tuple(int(g) for g in m.groups()) if m else None


def raise_window():
    """Put the game in front in the compositor's opinion, not just Wine's."""
    OUT.mkdir(parents=True, exist_ok=True)
    script_path = OUT / "raise.js"
    script_path.write_text(
        "for (const w of workspace.windowList()) {\n"
        f'    if (w.caption && w.caption.indexOf("{WINDOW}") !== -1) {{\n'
        "        w.minimized = false;\n"
        "        workspace.activeWindow = w;\n"
        "    }\n"
        "}\n"
    )
    dest = ["--session", "--dest", "org.kde.KWin", "--object-path", "/Scripting"]
    call = ["gdbus", "call"] + dest + ["--method"]
    subprocess.run(
        call + ["org.kde.kwin.Scripting.loadScript", str(script_path), "drivefh5"],
        capture_output=True,
    )
    subprocess.run(call + ["org.kde.kwin.Scripting.start"], capture_output=True)
    time.sleep(1)
    subprocess.run(
        call + ["org.kde.kwin.Scripting.unloadScript", "drivefh5"], capture_output=True
    )


def shot(tag="fh5"):
    """Capture the desktop and crop to the game window."""
    OUT.mkdir(parents=True, exist_ok=True)
    raise_window()
    full = OUT / f"{tag}_screen.png"
    full.unlink(missing_ok=True)
    subprocess.run(
        ["spectacle", "-b", "-n", "-f", "-o", str(full)], capture_output=True, timeout=90
    )
    for _ in range(30):
        if full.exists() and full.stat().st_size > 0:
            break
        time.sleep(0.5)
    if not full.exists():
        print("   (no screenshot: spectacle produced nothing)")
        return None
    try:
        from PIL import Image
    except ImportError:
        print(f"   {full} (uncropped: python-pillow is not installed)")
        return full
    im = Image.open(full).convert("RGB")
    rect = window_rect()
    if rect:
        x, y, w, h = rect
        im = im.crop((x, y, min(x + w, im.width), min(y + h, im.height)))
    out = OUT / f"{tag}.png"
    im.save(out)
    full.unlink(missing_ok=True)
    print(f"   {out}")
    return out


def press_start():
    """Tap Return, which is what the welcome screen offers as START GAME."""
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / "script.txt"
    path.write_text(f"focus {WINDOW}\nwait 800\nkey {VK_RETURN}\nwait 2000\n")
    return bot(["script", "Z:" + str(path).replace("/", "\\")])


def launch():
    if game_pid():
        print(":: already running")
        return
    print(":: launching")
    LOG.unlink(missing_ok=True)
    env = dict(os.environ, WINEDEBUG=os.environ.get("XODUS_WINEDEBUG", "+timestamp,+gdkc"))
    subprocess.Popen(
        ["setsid", str(REPO / "target/release/ferestre"), "run", SLUG],
        cwd=str(GAMES),
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    deadline = time.time() + WINDOW_TIMEOUT
    while time.time() < deadline:
        if window_rect():
            print(f"   window up after {int(WINDOW_TIMEOUT - (deadline - time.time()))}s")
            return
        time.sleep(5)
    die(f"no game window appeared within {WINDOW_TIMEOUT}s", f"log: {LOG}")


def watch():
    """Press Start Game once the welcome screen is up, and wait for the end.

    The presses are timed from the *game's* uptime, not the launcher's: the
    launcher spends about forty seconds decrypting and starting Proton, and a
    press before the window exists goes to whatever else has focus.
    """
    presses = 0
    while True:
        pid = game_pid()
        if not pid:
            return f"exited after {presses} Start Game press(es)"
        up = uptime(pid)
        if up >= PATIENCE:
            return f"still running after {up}s and {presses} Start Game press(es)"
        if up >= PRESS_AFTER and (up - PRESS_AFTER) % PRESS_EVERY < 3:
            presses += 1
            print(f":: Start Game (press {presses}, {up}s in)")
            shot(f"{presses}-before-start")
            press_start()
            time.sleep(5)
            shot(f"{presses}-after-start")
        time.sleep(3)


def report(verdict):
    text = LOG.read_text(errors="replace") if LOG.exists() else ""
    faults = re.findall(
        r"Unhandled page fault on \w+ access to ([0-9A-F]+) at address ([0-9A-F]+)", text
    )
    print()
    print(f":: {verdict}")
    if faults:
        where, addr = faults[-1]
        print(f"   faulted reading {where} at {addr}")
    if "no MSAAppId" in text:
        print("   tokens carried no title claim (MicrosoftGame.config has no MSAAppId)")
    if "title named no service configuration" in text:
        print("   the save provider fell back to the title's own SCID")
    crash = (
        PREFIX
        / "drive_c/users/steamuser/AppData/Local/Packages"
        / "Microsoft.624F8B84B80_8wekyb3d8bbwe/PersistentLocalStorage/CrashReport.xml"
    )
    if crash.exists():
        report_text = crash.read_text(errors="replace")
        for field in ("UPTIME", "UI_SCENE"):
            m = re.search(rf'<{field} Value="([^"]*)"', report_text)
            if m:
                print(f"   {field}: {m.group(1)}")
    return 0 if faults == [] else 1


def kill():
    subprocess.run([str(REPO / "scripts/kill-gdk.sh")], capture_output=True)


def main(argv):
    cmd = argv[1] if len(argv) > 1 else "run"
    if cmd == "kill":
        kill()
        return 0
    if cmd == "shot":
        shot(argv[2] if len(argv) > 2 else "fh5")
        return 0
    if cmd == "start":
        press_start()
        return 0
    if cmd != "run":
        die(f"unknown command {cmd!r}", __doc__.strip().splitlines()[2])
    launch()
    verdict = watch()
    rc = report(verdict)
    kill()
    return rc


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

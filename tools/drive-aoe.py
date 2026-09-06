#!/usr/bin/env python3
"""Start Age of Empires: Definitive Edition and play it, without a person.

    drive-aoe.py            launch, walk the menus into a match, report
    drive-aoe.py shot NAME  capture the game window now
    drive-aoe.py menus      drive the menus of an already-running game
    drive-aoe.py kill       stop the game

This is the evidence behind the recipe's `playable`, and it is here so the
claim can be rechecked rather than believed. The title's failure mode was a
crash on the way *into* a match, which a launch-and-look test never reaches:
the main menu drew fine for weeks while starting a game faulted.

Sibling of tools/auto-join.py, which does the same for Minecraft Bedrock, and
it borrows that script's two halves:

  Input   goes through tools/wine_input_bot.c, running inside the game's own
          Wine prefix, so SendInput lands in the queue the game reads. This is
          a KDE Wayland session: the X root is empty and there is no
          xdotool/ydotool to fall back on.
  Vision  comes from spectacle on the host side.

The one thing that is not shared: on Wayland, `SetForegroundWindow` inside Wine
moves Wine's own focus and tells the compositor nothing. A game window sitting
behind another application is still captured as that other application's
pixels. So the window is raised through KWin's scripting interface first, and
the capture is cropped to the rectangle Wine reports.
"""

import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

HOME = Path.home()
GAMES = Path(os.environ.get("XODUS_GAMES_DIR", HOME / "xbox-games"))
SLUG = "age-of-empires-definitive-edition"
GAME_DIR = GAMES / SLUG
PREFIX = Path(os.environ.get("WINEPREFIX", GAMES / f"{SLUG}-proton" / "pfx"))
TOOL = Path(
    os.environ.get(
        "XODUS_PROTON_DIR", HOME / ".local/share/Steam/compatibilitytools.d/xodus"
    )
) / "files"
WINE = TOOL / "bin" / "wine"
BOT = Path(os.environ.get("XODUS_BOT_EXE", GAME_DIR / "_bot.exe"))
OUT = Path(os.environ.get("AOE_OUT", "/tmp/ferestre-drive-aoe"))
LOG = GAMES / f"{SLUG}-launch.log"
REPO = Path(__file__).resolve().parent.parent

# The game window's title carries its build, so match on the stable part.
WINDOW = "Age of Empires"

# Client coordinates, measured against the 1280x720 window the title opens in.
# Percentages would survive a resize; these do not, and are honest about it --
# the menu is a fixed-size panel in the corner, and a run that finds the wrong
# button reports a wrong screenshot rather than a wrong number.
SINGLE_PLAYER = (270, 311)
CUSTOM_GAME = (270, 342)
START_GAME = (164, 648)

NOISE = re.compile(r"^(ntsync|radv|wineserver|wine:|WARNING|Proton)")


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


def die(message, *detail):
    print(f"!! {message}", file=sys.stderr)
    for line in detail:
        print(f"   {line}", file=sys.stderr)
    raise SystemExit(1)


def game_pid():
    out = subprocess.run(
        ["pgrep", "-f", "AoEDE.exe"], capture_output=True, text=True
    ).stdout.split()
    return int(out[0]) if out else None


def window_rect():
    """Where the game window is, as Wine sees it."""
    m = re.search(r"rect (-?\d+),(-?\d+) (\d+)x(\d+)", bot(["windows"]) or "")
    return tuple(int(g) for g in m.groups()) if m else None


def raise_window():
    """Put the game in front, in the compositor's opinion and not just Wine's.

    Wine's SetForegroundWindow reorders Wine's own windows; the Wayland
    compositor is not told. Without this the screenshot below is of whatever
    happens to be on top -- which, the first time, was a QEMU window, and the
    resulting picture of somebody else's virtual machine took a while to
    explain.
    """
    script = OUT / "raise.js"
    script.write_text(
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
        call + ["org.kde.kwin.Scripting.loadScript", str(script), "driveaoe"],
        capture_output=True,
    )
    subprocess.run(call + ["org.kde.kwin.Scripting.start"], capture_output=True)
    time.sleep(1)
    subprocess.run(
        call + ["org.kde.kwin.Scripting.unloadScript", "driveaoe"], capture_output=True
    )


def shot(tag="aoe"):
    """Capture the desktop and crop to the game window."""
    try:
        from PIL import Image
    except ImportError:
        die("python-pillow is needed to crop the screenshot")
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
        die("spectacle produced nothing")
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


def script(lines):
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / "script.txt"
    path.write_text("\n".join(lines) + "\n")
    # The bot is a Windows binary and wants a Windows path.
    return bot(["script", "Z:" + str(path).replace("/", "\\")])


def launch():
    if game_pid():
        print(":: already running")
        return
    print(":: launching")
    LOG.unlink(missing_ok=True)
    env = dict(os.environ, WINEDEBUG=os.environ.get("XODUS_WINEDEBUG", "+gdkc,+seh"))
    subprocess.Popen(
        ["setsid", str(REPO / "target/release/ferestre"), "run", SLUG],
        cwd=str(GAMES),
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    deadline = time.time() + 180
    while time.time() < deadline:
        if window_rect():
            print(f"   window up after {int(180 - (deadline - time.time()))}s")
            return
        time.sleep(5)
    die("no game window appeared within three minutes", f"log: {LOG}")


def menus():
    """Main menu -> Single Player -> Custom Game -> Start Game."""
    print(":: main menu")
    shot("1-menu")
    print(":: Single Player")
    script([f"focus {WINDOW}", "wait 800", "click %d %d" % SINGLE_PLAYER, "wait 3000"])
    shot("2-single-player")
    print(":: Custom Game")
    script([f"focus {WINDOW}", "wait 500", "click %d %d" % CUSTOM_GAME, "wait 6000"])
    shot("3-setup")
    print(":: Start Game")
    script([f"focus {WINDOW}", "wait 500", "click %d %d" % START_GAME, "wait 25000"])
    shot("4-in-game")


def report():
    """What the run proved, and what the log says about it."""
    alive = game_pid()
    faults = 0
    refused = 0
    if LOG.exists():
        text = LOG.read_text(errors="replace")
        faults = text.count("Unhandled page fault")
        refused = text.count("a block this runtime never began")
    print()
    print(f":: game running after the match started: {'yes' if alive else 'NO'}")
    print(f":: unhandled page faults in the log:     {faults}")
    print(f":: async blocks refused (expected, >0):  {refused}")
    print(f":: screenshots: {OUT}")
    if not alive or faults:
        die("the title did not survive starting a game", f"log: {LOG}")
    print(":: playable")


def kill():
    subprocess.run([str(GAMES / "kill-gdk.sh")], capture_output=True)
    print(":: stopped")


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "all"
    if cmd == "shot":
        shot(sys.argv[2] if len(sys.argv) > 2 else "aoe")
    elif cmd == "menus":
        menus()
        report()
    elif cmd == "kill":
        kill()
    elif cmd == "all":
        launch()
        menus()
        report()
    else:
        die(f"unknown command: {cmd}", __doc__.strip().splitlines()[2])

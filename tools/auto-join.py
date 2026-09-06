#!/usr/bin/env python3
"""Reproduce the Minecraft Bedrock server-join freeze without a person.

Joining a third-party server from the in-game server list freezes the game;
joining the same server from the history, or by IP, does not. Reproducing it by
hand takes minutes of clicking, which makes it useless as a test loop. This
drives the whole thing: launch, navigate the menus, detect the freeze, collect
the evidence, kill, repeat.

Vision comes from spectacle, because this is a KDE Wayland session where the X
root is empty and no xdotool/ydotool is installed. Input goes through
tools/wine_input_bot.c, which runs inside the game's own Wine prefix and so
shares its input queue.

  auto-join.py shot                 capture the game window
  auto-join.py windows              list the game's windows
  auto-join.py click <x> <y>        click at window-client coordinates
  auto-join.py run <script>         run an input script
  auto-join.py launch               start the game
  auto-join.py state                report running / frozen / progress
  auto-join.py collect <tag>        capture stacks + logs for a frozen run
  auto-join.py kill                 stop the game
"""
import os, re, subprocess, sys, time
from pathlib import Path

HOME = Path.home()
REPO = Path(__file__).resolve().parent.parent
GAMES = Path(os.environ.get("XODUS_GAMES_DIR", HOME / "xbox-games"))
GAME_DIR = GAMES / "bedrock" / "game"
PREFIX = Path(os.environ.get("WINEPREFIX", GAMES / "bedrock-proton" / "pfx"))
TOOL = Path(os.environ.get("XODUS_PROTON_DIR",
                           HOME / ".local/share/Steam/compatibilitytools.d/xodus")) / "files"
WINE = TOOL / "bin" / "wine"
def _find_bot():
    """The input bot only needs user32, so it can live anywhere; prefer a copy
    beside the outputs and fall back to one staged in the game directory."""
    for cand in (os.environ.get("XODUS_BOT_EXE"), OUT / "bot.exe", GAME_DIR / "_bot.exe"):
        if cand and Path(cand).exists():
            return Path(cand)
    return OUT / "bot.exe"
LOG = GAMES / "bedrock-launch.log"
OUT = Path(os.environ.get("XODUS_AUTOJOIN_OUT", "/tmp/xodus-autojoin"))
OUT.mkdir(parents=True, exist_ok=True)
BOT = None

NOISE = re.compile(r"^(ntsync|radv|wineserver|wine: created)")


def wine(args, timeout=60):
    global BOT
    if BOT is None:
        BOT = _find_bot()
    env = dict(os.environ, WINEPREFIX=str(PREFIX), WINEDEBUG="-all")
    try:
        p = subprocess.run([str(WINE), str(BOT)] + args, capture_output=True,
                           text=True, timeout=timeout, env=env, cwd=str(GAME_DIR))
    except subprocess.TimeoutExpired:
        return "TIMEOUT"
    return "\n".join(l for l in p.stdout.splitlines() if not NOISE.match(l))


def game_pid():
    """The PE process, i.e. the one with many threads."""
    try:
        pids = subprocess.run(["pgrep", "-f", "Minecraft.Windows.exe"],
                              capture_output=True, text=True).stdout.split()
    except FileNotFoundError:
        return None
    for pid in pids:
        try:
            if len(list((Path("/proc") / pid / "task").iterdir())) > 20:
                return int(pid)
        except OSError:
            continue
    return None


def window_rect():
    out = wine(["windows"], timeout=45)
    m = re.search(r"rect (-?\d+),(-?\d+) (\d+)x(\d+).*class 'Bedrock'", out or "")
    if not m:
        return None
    x, y, w, h = (int(g) for g in m.groups())
    return x, y, w, h


def shot(tag="game"):
    """Capture the screen and crop to the game window."""
    from PIL import Image
    full = OUT / f"{tag}_screen.png"
    full.unlink(missing_ok=True)
    subprocess.run(["spectacle", "-b", "-n", "-f", "-o", str(full)],
                   capture_output=True, timeout=60)
    for _ in range(20):
        if full.exists() and full.stat().st_size > 0:
            break
        time.sleep(0.5)
    if not full.exists():
        return None
    im = Image.open(full).convert("RGB")
    rect = window_rect()
    if rect:
        x, y, w, h = rect
        im = im.crop((x, y, x + w, y + h))
    out = OUT / f"{tag}.png"
    im.save(out)
    half = OUT / f"{tag}_half.png"
    im.resize((im.width // 2, im.height // 2)).save(half)
    return out


def launch():
    if game_pid():
        print("already running")
        return
    LOG.unlink(missing_ok=True)
    env = dict(os.environ, WINEDEBUG=os.environ.get("XODUS_WINEDEBUG", "+gdkc"))
    subprocess.Popen(["setsid", "nohup", str(GAMES / "launch-bedrock-debug.sh")],
                     cwd=str(GAMES), env=env,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     start_new_session=True)
    print("launched")


def cpu_ticks(pid):
    total = 0
    try:
        for t in (Path("/proc") / str(pid) / "task").iterdir():
            try:
                # the comm field can contain spaces, so cut past the last ')'
                s = (t / "stat").read_text().rsplit(")", 1)[1].split()
                total += int(s[11]) + int(s[12])
            except (OSError, IndexError, ValueError):
                pass
    except OSError:
        pass
    return total


def state(seconds=6):
    pid = game_pid()
    if not pid:
        return {"running": False}
    size0 = LOG.stat().st_size if LOG.exists() else 0
    c0 = cpu_ticks(pid)
    time.sleep(seconds)
    size1 = LOG.stat().st_size if LOG.exists() else 0
    c1 = cpu_ticks(pid)
    return {"running": True, "pid": pid, "log_growth": size1 - size0,
            "cpu_ticks": c1 - c0, "frozen": (size1 == size0 and c1 - c0 < 5)}


def kill():
    subprocess.run("pgrep -f '[M]inecraft.Windows.exe' | xargs -r kill -9",
                   shell=True, capture_output=True)
    subprocess.run("pgrep -f '[l]aunch-gdk.sh' | xargs -r kill -9",
                   shell=True, capture_output=True)
    print("killed")


def collect(tag):
    pid = game_pid()
    stamp = OUT / tag
    stamp.mkdir(parents=True, exist_ok=True)
    if pid:
        ws = REPO / "tools" / "winestack"
        if ws.exists():
            with open(stamp / "stacks.txt", "w") as f:
                subprocess.run([str(ws), str(pid)], stdout=f, stderr=subprocess.STDOUT,
                               timeout=300)
    if LOG.exists():
        subprocess.run(f"tail -c 4000000 '{LOG}' > '{stamp / 'tail.log'}'", shell=True)
    shot(f"{tag}/frozen")
    print(f"collected into {stamp}")


# Where the menu items sit, as fractions of the game window, so the sequence
# survives the window being a different size between runs:
#   Play -> Servers tab -> The Hive -> PLAY
JOIN_STEPS = [
    ("play",    0.499, 0.460, 5),
    ("servers", 0.830, 0.176, 6),
    ("hive",    0.129, 0.845, 5),
    ("join",    0.797, 0.643, 0),
]


def click_at(x, y):
    script = OUT / "_click.txt"
    script.write_text(f"click {x} {y}\n")
    return wine(["script", str(script)], timeout=60)


# Pin the window geometry, so a run is identical whether the desk monitor or a
# 1080p remote session is driving the display.
CANON_RECT = (60, 36, 1864, 1048)


def pin_window(attempts=6):
    """Force the canonical geometry and verify it took.

    The game opens at whatever the current display offers, and a title that
    re-asserts fullscreen can undo a single SetWindowPos -- so pin, check, and
    retry. Menu positions are fractions of the window, but Minecraft also
    changes its UI scale with resolution, so a wildly different window is not
    merely a scaled one: better to fail loudly than click blind.
    """
    x, y, w, h = CANON_RECT
    for i in range(attempts):
        wine(["resize", "Minecraft", str(x), str(y), str(w), str(h)], timeout=60)
        time.sleep(1.5)
        rect = window_rect()
        if rect and abs(rect[2] - w) <= 8 and abs(rect[3] - h) <= 8:
            print(f"  pin: {rect}")
            return rect
        print(f"  pin attempt {i + 1}: got {rect}, want {CANON_RECT}")
    print("  pin: FAILED - refusing to click at unverified coordinates")
    return None


def repro(tag="run", settle=45, patience=40):
    """Launch, join The Hive from the server list, and wait for the freeze."""
    kill()
    time.sleep(3)
    launch()

    rect = None
    for _ in range(60):
        rect = window_rect()
        if rect:
            break
        time.sleep(2)
    if not rect:
        print("FAIL - the game window never appeared")
        return 1
    rect = pin_window()
    if not rect:
        print("FAIL - could not pin the window geometry")
        return 1
    print(f"window {rect}; letting the menu settle {settle}s")
    time.sleep(settle)

    for name, fx, fy, pause in JOIN_STEPS:
        rect = window_rect()
        if not rect:
            print(f"FAIL - lost the window before '{name}'")
            return 1
        x, y, w, h = rect
        sx, sy = int(x + fx * w), int(y + fy * h)
        print(f"  {name}: click {sx},{sy}")
        click_at(sx, sy)
        shot(f"{tag}_{name}")
        if pause:
            time.sleep(pause)

    print("  joining; waiting for the freeze")
    for i in range(patience):
        st = state()
        if not st["running"]:
            print(f"  the game exited after {i * 6}s")
            return 2
        if st["frozen"]:
            print(f"FROZE after {i * 6}s: {st}")
            collect(tag)
            return 0
    print("no freeze seen; the join may have succeeded")
    shot(f"{tag}_end")
    return 3


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "state"
    if cmd == "shot":
        print(shot(sys.argv[2] if len(sys.argv) > 2 else "game"))
    elif cmd == "pin":
        print(pin_window())
    elif cmd == "windows":
        print(wine(["windows"], timeout=45))
    elif cmd == "run":
        print(wine(["script", sys.argv[2]], timeout=int(os.environ.get("BOT_TIMEOUT", "180"))))
    elif cmd == "click":
        script = OUT / "_click.txt"
        script.write_text(f"click {sys.argv[2]} {sys.argv[3]}\n")
        print(wine(["script", str(script)], timeout=60))
    elif cmd == "launch":
        launch()
    elif cmd == "state":
        print(state())
    elif cmd == "collect":
        collect(sys.argv[2] if len(sys.argv) > 2 else "run")
    elif cmd == "kill":
        kill()
    elif cmd == "repro":
        sys.exit(repro(sys.argv[2] if len(sys.argv) > 2 else "run"))
    else:
        print(__doc__)


if __name__ == "__main__":
    main()

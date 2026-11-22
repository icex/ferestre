#!/usr/bin/env python3
"""Regenerate the README screenshots from a demo library and a synthetic account.

What it does
    Builds a throwaway demo HOME (a handful of installed titles as 4 kB fake
    folders, an owned library, a PC Game Pass list, one pending update and the
    signed-in account "Player One"), starts the release GUI in a private,
    virtual KWin session for each picture, drives it through AT-SPI and the
    tests' fake SDL controller, and captures the virtual screen through KWin's
    ScreenShot2 interface. The pictures land in docs/screenshots/:

        home.png               Home: hero carousel, quick links, shelves
        library-handheld.png   fullscreen My Library, a focused card, update badge
        running-game.png       a running title's Resume / Stop overlay
        install-game.png       the install dialog over the blurred library
        stop-confirmation.png  the Stop confirmation, Cancel selected by the controller
        notifications.png      three stacked "Starting ..." notifications
        compact-sidebar.png    the icon rail with the account menu open
        home-ultrawide.png     Home at 3440x1000 (a 32:9-style wide window)
        already-running.png    the "<title> is already running" dialog

    All are 1280x800 except home-ultrawide.png.

How to run it
    export PATH=$HOME/.cargo/bin:$PATH
    CARGO_TARGET_DIR=/var/tmp/ferestre-readme-target \\
        nice cargo build --release --offline -p ferestre-gui
    nice python3 tools/readme-screenshots.py \\
        --binary /var/tmp/ferestre-readme-target/release/ferestre-gui
    # one or two pictures, kept in a scratch folder for a look first:
    python3 tools/readme-screenshots.py --only home,install-game --out /var/tmp/shots

    Needs util-linux `unshare` with unprivileged user namespaces, kwin_wayland,
    dbus-daemon, the AT-SPI registry, python3-gobject with Atspi, Pillow and a
    C compiler (for the fake controller library). Scratch space is a private
    folder under /var/tmp/ferestre-readme-shots, removed afterwards unless
    --keep is given. The renderer is GTK's default, so the dialog blur shows
    as users see it; --renderer cairo is the fallback where GL does not work.

Privacy guarantees
    - The GUI never sees the real HOME, ~/.config/ferestre, the keyring or a
      real account. Each picture runs in its own user, mount, UTS and PID
      namespace in which /home, /run/user, /run/media, /media and /mnt are
      empty tmpfs mounts. The demo HOME is /home/player on that tmpfs, the
      user is "player" (a bind-mounted passwd), the hostname is
      "ferestre-demo", and the free space the install dialog reports is the
      tmpfs's, not a real disk's. The stage that starts the GUI refuses to
      run unless HOME is /home/player, the real home folder is not reachable,
      and it is in a mount namespace of its own.
    - The D-Bus session is private and has no service activation, so no
      keyring, portal or volume monitor daemon is reached. Network access goes
      to a refused local proxy.
    - The account is the fake client's "Player One" with a zero XUID and no
      gamerpic. The xodus-cli, ferestre CLI and the games are fakes; a
      "running game" is `sleep` under a Windows image name.
    - Artwork is public Microsoft Store catalogue art for well-known titles,
      copied from the local catalogue cache (~/.cache/ferestre/catalog). Only
      product metadata is kept from each entry (id, name, publisher, kind);
      the market is set to US, image URLs point at an unroutable address, and
      download sizes are synthetic.
"""
import argparse
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import zlib

DEMO_HOME = Path("/home/player")
SCRATCH_ROOT = Path("/var/tmp/ferestre-readme-shots")
APP_ID = "io.github.icex.ferestre.ReadmeShots"
SOCKET = "ferestre-readme-wayland"
ACCOUNT = {"gamertag": "Player One", "xuid": "0000000000000000"}

# (product id, catalogue entry to take name/publisher from, art donor or None)
# A donor is the same game's other Store listing, for a banner the cached
# listing lacks.
OWNED = [
    ("9NJWTJSVGVLJ", None),  # Age of Empires: Definitive Edition
    ("9MSVPJCHHRPR", None),  # Avowed
    ("BSLX1RNXR6H7", None),  # Batman: Arkham Knight
    ("9PL1J8PJKH29", None),  # Control
    ("9MSVVM5NS9L6", None),  # DREDGE
    ("9NNX1VVR3KNQ", "9NKX70BBCDRN"),  # Forza Horizon 5
    ("9MT5NJ5W7B8Z", None),  # Hogwarts Legacy
    ("9NBLGGH2JHXJ", None),  # Minecraft for Windows
    ("9MWR1NC6VQ6L", None),  # Stardew Valley
]
# Installed, most recently played first. Each has a banner, so no tile in
# "Jump back in" is blank, even at 2560 pixels wide.
INSTALLED = ["9PL1J8PJKH29", "9MWR1NC6VQ6L", "9NNX1VVR3KNQ", "9NJWTJSVGVLJ", "BSLX1RNXR6H7"]
UPDATE = "BSLX1RNXR6H7"
GAMEPASS = ["9MSVPJCHHRPR", "9PJTHRNVH62H", "C2KDNLT2H7DM", "9PP5G1F0C2B6", "9N6VPDLQDVT5",
            "9NMS4SFNBGBH", "9NCJSXWZTP88", "9N8FQ28Z6QX3", "9PMBB7NGHV95", "9NKV34XDW014",
            "9NJ2FD5HJ7SB", "9P5786PJB9RP", "9NBLGGH2JHXJ"]
DOWNLOAD_GB = {"9MSVPJCHHRPR": 75, "9MT5NJ5W7B8Z": 82, "9NNX1VVR3KNQ": 118}
RUNNING = "9PL1J8PJKH29"  # Control, for the running, Stop and already-running shots
INSTALL = "9MSVPJCHHRPR"  # Avowed, for the install dialog
FOCUS = "9NJWTJSVGVLJ"  # Age of Empires, the focused card in fullscreen

SHOTS = {
    # name: (width, height, window.json, controller)
    "home": (1280, 800, {}, False),
    "library-handheld": (1280, 800, {"fullscreen": True}, True),
    "running-game": (1280, 800, {"fullscreen": True}, True),
    "install-game": (1280, 800, {"fullscreen": True}, True),
    "stop-confirmation": (1280, 800, {"fullscreen": True}, True),
    "notifications": (1280, 800, {}, False),
    "compact-sidebar": (1280, 800, {"compact": True}, False),
    "home-ultrawide": (3440, 1000, {}, False),
    "already-running": (1280, 800, {}, False),
}

CLIENT = r'''#!/usr/bin/env python3
# Fake xodus-cli: one synthetic signed-in account, nothing real.
import json, os, sys
from pathlib import Path
home = Path(os.environ['HOME'])
args = [a for a in sys.argv[1:] if a != '--json']
with (home / '.client-calls').open('a') as log:
    log.write(' '.join(args) + '\n')
command = args[0] if args else ''
if command == 'account':
    print((home / '.fixture/account.json').read_text())
elif command == 'library':
    print((home / '.fixture/library.json').read_text())
elif command == 'update':
    versions = json.loads((home / '.fixture/versions.json').read_text())
    print(json.dumps({'version': versions.get(args[1], '1.0.0.0')}))
elif command == 'migrate-accounts':
    print('{}')
else:
    sys.exit(1)
'''

# The fake `ferestre`: `run <id>` waits for `ready-<id>`, then starts a
# `sleep` named like the game, as tests/gui_launch.py does.
CLI = r'''#!/usr/bin/env python3
import json, os, subprocess, sys, time
from pathlib import Path
h = Path(os.environ['HOME']) / '.fixture'
if sys.argv[1:2] != ['run']:
    # An install or update stays queued, so its notification is the queued one.
    if any(a in ('install', 'update') for a in sys.argv[1:2]):
        while not (h / 'end-all').exists(): time.sleep(.05)
    print('{}'); sys.exit(0)
id = sys.argv[2]
(h / ('launch-' + id)).write_text(str(os.getpid()))
while not (h / ('ready-' + id)).exists():
    time.sleep(.03)
# Named as the recipe's executable, which is what tells the window the game is up.
exe = json.loads((h / 'executables.json').read_text()).get(id, 'Game.exe').replace('\\', '/').split('/')[-1]
child = subprocess.Popen(['bash', '-c', 'exec -a "$0" sleep 600', exe])
(h / ('game-pid-' + id)).write_text(str(child.pid))
try:
    while not (h / ('end-' + id)).exists(): time.sleep(.05)
finally:
    child.terminate(); child.wait(timeout=5)
'''


def write(path, text, executable=False):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    if executable:
        path.chmod(0o755)


# ---------------------------------------------------------------- outer stage

def sanitized_entry(cached, pid, schema):
    """Product metadata only, from a public catalogue entry."""
    product = cached.get("product", {})
    gb = DOWNLOAD_GB.get(pid, 40)
    return {
        "schema": schema, "market": "US", "language": "en-us",
        "product": {
            "product_id": pid,
            "name": str(product["name"]),
            "publisher": str(product.get("publisher") or ""),
            "product_kind": "Game",
            "download_bytes": gb * 1_000_000_000,
            "content_ids": [f"00000000-0000-0000-0000-{zlib.crc32(pid.encode()):012d}"],
            "has_packages": True, "has_pc_package": True, "package_format": "MSIXVC",
            "image": "http://127.0.0.1:9/image.png",
            "artwork": {k: "http://127.0.0.1:9/image.png" for k in ("portrait", "landscape", "hero", "logo")},
        },
    }


def build_template(repo, scratch, catalog):
    """The demo HOME, written under scratch/template and copied into each
    namespace's /home/player. All paths in it are written as /home/player."""
    t = scratch / "template"
    cat_schema = int(re.search(r"const CACHE_SCHEMA: u32 = (\d+);",
                               (repo / "crates/ferestre-core/src/catalog.rs").read_text())[1])
    list_schema = int(re.search(r"const LISTING_SCHEMA: u32 = (\d+);",
                                (repo / "crates/ferestre-core/src/gamepass.rs").read_text())[1])
    import tomllib
    titles = {}
    for f in (repo / "titles").glob("*.toml"):
        try:
            r = tomllib.loads(f.read_text())
        except (OSError, ValueError):
            continue
        if r.get("title", {}).get("product-id"):
            titles[r["title"]["product-id"]] = (f, r)
    shutil.copytree(repo / "titles", t / ".fixture/titles",
                    ignore=shutil.ignore_patterns("*.bak", "*.orig"))
    # Catalogue: entries and the cached images, for every title shown.
    out = t / ".cache/ferestre/catalog"
    (out / "images").mkdir(parents=True)
    donors = dict(OWNED)
    for pid in dict.fromkeys([p for p, _ in OWNED] + GAMEPASS):
        source = catalog / f"{pid}.json"
        if not source.is_file():
            raise SystemExit(f"{pid} is not in {catalog}; pick another demo title")
        entry = sanitized_entry(json.loads(source.read_text()), pid, cat_schema)
        if pid in titles:  # the recipe's display name
            entry["product"]["name"] = titles[pid][1]["title"].get("name", entry["product"]["name"])
        (out / f"{pid}.json").write_text(json.dumps(entry))
        for kind in ("cover-600", "hero-960", "64"):
            for donor in (pid, donors.get(pid)):
                image = catalog / "images" / f"{donor}-{kind}.png" if donor else None
                if image and image.is_file():
                    shutil.copy2(image, out / "images" / f"{pid}-{kind}.png")
                    break
    # Installed titles: tiny fake folders, as the GUI tests make them.
    games = t / "xbox-games"
    state = t / ".local/state/ferestre"
    now = time.time()
    for n, pid in enumerate(INSTALLED):
        recipe = titles.get(pid)
        slug = recipe[1]["title"]["slug"] if recipe else pid.lower()
        d = games / slug
        write(d / "MicrosoftGame.config", f"<Game><StoreId>{pid}</StoreId></Game>")
        write(d / "appxmanifest.xml", '<Package><Identity Name="Demo.Game" Version="1.0.0.0"/></Package>')
        write(d / ".xodus-streaming.msixvc", "")
        record = {"product_id": pid, "dir": str(DEMO_HOME / "xbox-games" / slug),
                  "content_ids": ["00000000-0000-0000-0000-00000000000%d" % n], "package_version": "1.0.0.0"}
        write(state / f"installed/{pid}.json", json.dumps(record))
        if pid == UPDATE:
            write(state / f"updates/{pid}.json", json.dumps({"schema": 1, "record": record, "available": "1.2.0.0"}))
            write(t / ".fixture/versions.json", json.dumps({record["content_ids"][0]: "1.2.0.0"}))
        played = state / f"launches/{pid}.stderr"
        write(played, "")
        os.utime(played, (now - 3600 * (n + 1) * 7, now - 3600 * (n + 1) * 7))
    items = [{"productId": p, "productType": "Game", "status": "Active"} for p, _ in OWNED]
    write(state / "library.json", json.dumps({"count": len(items), "items": items}))
    write(t / ".fixture/library.json", json.dumps({"count": len(items), "items": items}))
    write(state / "gamepass.json", json.dumps({
        "schema": list_schema, "fetched": int(now), "market": "US", "product_ids": GAMEPASS,
        "tiers": {"included": {p: ["PCSubMetadata"] for p in GAMEPASS}}}))
    write(state / "account.json", json.dumps(ACCOUNT))
    write(t / ".fixture/account.json", json.dumps(ACCOUNT))
    # Runtime, client and CLI fakes.
    rt = t / ".fixture/runtime"
    write(rt / "proton", "#!/bin/sh\nexit 0\n", True)
    write(rt / "version", "0 demo-runtime\n")
    wanted = set()
    for _, r in titles.values():
        wanted |= set(r.get("runtime", {}).get("requires", [])) | set(r.get("runtime", {}).get("wants", []))
    write(rt / "files/share/ferestre/capabilities.json", json.dumps({"capabilities": sorted(wanted)}))
    write(rt / "files/bin/wineserver", "#!/bin/sh\nexit 0\n", True)
    # A wine beside wineserver lets the already-running dialog describe the usual
    # close (ask the game, then end it after the wait), not the rare fallback.
    write(rt / "files/bin/wine", "#!/bin/sh\nexit 0\n", True)
    write(t / ".fixture/client/xodus-cli", CLIENT, True)
    write(t / ".fixture/bin/ferestre", CLI, True)
    write(t / ".fixture/executables.json", json.dumps(
        {pid: r["launch"]["executable"] for pid, (_, r) in titles.items() if r.get("launch", {}).get("executable")}))
    write(t / ".config/gtk-4.0/settings.ini", "[Settings]\ngtk-enable-animations=true\n")
    (t / "Steam").mkdir()
    return titles


def prepare_scratch(repo, args):
    SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="run-", dir=SCRATCH_ROOT))
    (scratch / "out").mkdir()
    shutil.copytree(repo / "tests", scratch / "tests", ignore=shutil.ignore_patterns("__pycache__", "fixtures"))
    (scratch / "tools").mkdir()
    shutil.copy2(Path(__file__).resolve(), scratch / "tools/readme-screenshots.py")
    titles = build_template(repo, scratch, Path(args.catalog).expanduser())
    shutil.copy2(Path(args.binary).resolve(), scratch / "template/.fixture/bin/ferestre-gui")
    # The tests' fake SDL: a controller whose state is a file.
    sys.path.insert(0, str(repo / "tests"))
    from gui_controller import SDL
    write(scratch / "sdl.c", SDL)
    (scratch / "template/.fixture/lib").mkdir()
    subprocess.run(["cc", "-shared", "-fPIC", str(scratch / "sdl.c"), "-o",
                    str(scratch / "template/.fixture/lib/libSDL2-2.0.so.0")], check=True)
    write(scratch / "passwd", "root:x:0:0:root:/root:/bin/sh\n"
          f"player:x:{os.getuid()}:{os.getgid()}:Player One:{DEMO_HOME}:/bin/sh\n"
          "nobody:x:65534:65534:Nobody:/:/usr/bin/nologin\n")
    write(scratch / "group", f"root:x:0:\nplayer:x:{os.getgid()}:\nnobody:x:65534:\n")
    write(scratch / "hostname", "ferestre-demo\n")
    prefix = {}
    for pid in (RUNNING,):
        r = titles[pid][1]
        prefix[pid] = r.get("install", {}).get("prefix") or r["title"]["slug"] + "-proton"
    write(scratch / "config.json", json.dumps({
        "renderer": args.renderer, "uid": os.getuid(), "gid": os.getgid(),
        "names": {p: json.loads((scratch / f"template/.cache/ferestre/catalog/{p}.json").read_text())["product"]["name"]
                  for p, _ in OWNED},
        "running_prefix": prefix[RUNNING],
        "outer_mnt_ns": os.readlink("/proc/self/ns/mnt"),
        "real_home": pwd.getpwuid(os.getuid()).pw_dir,
        "debug": args.debug}))
    return scratch


def optimise(source, dest):
    from PIL import Image
    image = Image.open(source).convert("RGB")
    image.save(dest, optimize=True)
    if dest.stat().st_size > 600_000:
        image.quantize(colors=256, method=Image.Quantize.MEDIANCUT,
                       dither=Image.Dither.FLOYDSTEINBERG).save(dest, optimize=True)
    for tool in (["oxipng", "-o", "4", "--strip", "safe", str(dest)],):
        if shutil.which(tool[0]):
            subprocess.run(tool, check=False, capture_output=True)


def outer(args):
    repo = Path(__file__).resolve().parent.parent
    names = list(SHOTS) if not args.only else args.only.split(",")
    unknown = [n for n in names if n not in SHOTS]
    if unknown:
        raise SystemExit(f"unknown screenshot(s): {', '.join(unknown)}; known: {', '.join(SHOTS)}")
    if not Path(args.binary).is_file():
        raise SystemExit(f"no GUI binary at {args.binary}; build it first (see the top of this file)")
    out = Path(args.out).resolve() if args.out else repo / "docs/screenshots"
    out.mkdir(parents=True, exist_ok=True)
    scratch = prepare_scratch(repo, args)
    failed = []
    try:
        for name in names:
            for attempt in range(2):
                print(f"== {name}" + (" (retry)" if attempt else ""), flush=True)
                code = subprocess.call(
                    ["unshare", "--user", "--map-root-user", "--mount", "--uts", "--pid", "--fork",
                     "--kill-child", "--mount-proc", sys.executable, "-I",
                     str(scratch / "tools/readme-screenshots.py"), "--stage", "ns", "--scratch", str(scratch),
                     "--shot", name], timeout=240)
                if code == 0 and (scratch / "out" / f"{name}.png").is_file():
                    optimise(scratch / "out" / f"{name}.png", out / f"{name}.png")
                    print(f"   {out / (name + '.png')} ({(out / (name + '.png')).stat().st_size // 1024} KB)")
                    break
            else:
                failed.append(name)
    finally:
        if args.keep:
            print("scratch kept in", scratch)
        else:
            shutil.rmtree(scratch, ignore_errors=True)
    if failed:
        raise SystemExit("failed: " + ", ".join(failed))


# ------------------------------------------------- namespace and session stages

def ns_stage(args):
    """Root in a private user namespace: hide every real home and mount, set
    up the demo home and identity, then drop back to the real uid."""
    scratch = Path(args.scratch)
    config = json.loads((scratch / "config.json").read_text())
    if os.readlink("/proc/self/ns/mnt") == config["outer_mnt_ns"]:
        raise SystemExit("refusing: not in a private mount namespace")

    def mount(*argv):
        subprocess.run(["mount", *argv], check=True)
    mount("--make-rprivate", "/")
    width, height = SHOTS[args.shot][:2]
    # 700 GiB of tmpfs: only what is written is used, and it is the free space
    # the install dialog reports.
    mount("-t", "tmpfs", "-o", "size=700g,mode=755", "tmpfs", "/home")
    for hidden in ("/run/user", "/run/media", "/media", "/mnt"):
        if os.path.isdir(hidden) and not os.path.islink(hidden):
            mount("-t", "tmpfs", "-o", "size=64m,mode=755", "tmpfs", hidden)
    for name in ("passwd", "group", "hostname"):
        mount("--bind", str(scratch / name), f"/etc/{name}")
    socket.sethostname("ferestre-demo")
    shutil.copytree(scratch / "template", DEMO_HOME, symlinks=True)
    window = SHOTS[args.shot][2]
    write(DEMO_HOME / ".config/ferestre/window.json", json.dumps(window))
    runtime = Path(f"/run/user/{config['uid']}")
    runtime.mkdir(mode=0o700)
    env = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(DEMO_HOME), "USER": "player",
           "LOGNAME": "player", "XDG_RUNTIME_DIR": str(runtime), "LANG": "en_US.UTF-8"}
    conf = runtime / "session.conf"
    conf.write_text("<busconfig><type>session</type><listen>unix:tmpdir=" + str(runtime) +
                    "</listen><auth>EXTERNAL</auth><policy context=\"default\"><allow send_destination=\"*\"/>"
                    "<allow receive_sender=\"*\"/><allow own=\"*\"/></policy></busconfig>")
    os.execvpe("unshare", ["unshare", "--user", f"--map-user={config['uid']}", f"--map-group={config['gid']}",
                           "dbus-run-session", f"--config-file={conf}", "--",
                           sys.executable, "-I", str(scratch / "tools/readme-screenshots.py"),
                           "--stage", "shoot", "--scratch", str(scratch), "--shot", args.shot,
                           "--size", f"{width}x{height}"], env)


def refuse_unless_private(config):
    real_home = config["real_home"]
    problems = []
    if os.environ.get("HOME") != str(DEMO_HOME):
        problems.append(f"HOME is {os.environ.get('HOME')!r}, not {DEMO_HOME}")
    if real_home != str(DEMO_HOME) and os.path.exists(real_home):
        problems.append(f"the real home {real_home} is reachable")
    if os.readlink("/proc/self/ns/mnt") == config["outer_mnt_ns"]:
        problems.append("not in a private mount namespace")
    if Path(DEMO_HOME / ".config/ferestre").resolve().is_relative_to(Path(real_home)):
        problems.append("the config folder is inside the real home")
    if problems:
        raise SystemExit("refusing to start the GUI: " + "; ".join(problems))


def shoot_stage(args):
    scratch = Path(args.scratch)
    config = json.loads((scratch / "config.json").read_text())
    refuse_unless_private(config)
    width, height = (int(v) for v in args.size.split("x"))
    runtime = Path(os.environ["XDG_RUNTIME_DIR"])
    session = runtime / "session"
    for part in ("config", "data"):
        (session / part).mkdir(parents=True, exist_ok=True)
    (session / "config/kwinrulesrc").write_text(
        "[General]\ncount=1\nrules=1\n\n[1]\nDescription=max\nmaximizehoriz=true\nmaximizehorizrule=3\n"
        "maximizevert=true\nmaximizevertrule=3\nnoborder=true\nnoborderrule=3\nwmclass=ferestre\nwmclassmatch=2\n")
    env = dict(os.environ, XDG_CONFIG_HOME=str(session / "config"), XDG_DATA_HOME=str(session / "data"),
               QT_QPA_PLATFORM="offscreen", KWIN_SCREENSHOT_NO_PERMISSION_CHECKS="1")
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "AT_SPI_BUS_ADDRESS"):
        env.pop(key, None)
    lines = subprocess.check_output(["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1"],
                                    text=True, env=env).splitlines()
    env["AT_SPI_BUS_ADDRESS"] = lines[0]
    os.environ["AT_SPI_BUS_ADDRESS"] = lines[0]
    children = []
    log = (scratch / "out" / f"{args.shot}-session.log").open("wb")
    try:
        registry = next(p for p in ("/usr/lib/at-spi2-registryd", "/usr/libexec/at-spi2-registryd",
                                    "/usr/libexec/at-spi2-core/at-spi2-registryd") if Path(p).is_file())
        children.append(subprocess.Popen([registry], env=env, stdout=log, stderr=log))
        children.append(subprocess.Popen(
            ["kwin_wayland", "--virtual", "--width", str(width), "--height", str(height), "--no-lockscreen",
             "--no-global-shortcuts", "--no-kactivities", "--socket", SOCKET], env=env, stdout=log, stderr=log))
        until = time.monotonic() + 20
        while not (runtime / SOCKET).exists():
            if children[-1].poll() is not None or time.monotonic() > until:
                raise SystemExit("private KWin did not start")
            time.sleep(0.1)
        os.environ["WAYLAND_DISPLAY"] = SOCKET
        Scene(scratch, config, args.shot, width, height).run()
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
        os.kill(int(lines[1]), signal.SIGTERM)


# ------------------------------------------------------------------ the scenes

class Scene:
    def __init__(self, scratch, config, shot, width, height):
        self.scratch, self.config, self.shot = scratch, config, shot
        self.width, self.height = width, height
        self.names = config["names"]
        sys.path.insert(0, str(scratch / "tests"))
        import gui_smoke
        self.s = gui_smoke
        self.fixture = DEMO_HOME / ".fixture"
        self.process = None
        self.games = []

    def env(self):
        h, f = DEMO_HOME, self.fixture
        env = dict(os.environ, HOME=str(h), XDG_CONFIG_HOME=str(h / ".config"),
                   XDG_STATE_HOME=str(h / ".local/state"), XDG_CACHE_HOME=str(h / ".cache"),
                   XDG_DATA_HOME=str(h / ".local/share"), FERESTRE_TITLES=str(f / "titles"),
                   XODUS_CLI_DIR=str(f / "client"), XODUS_PROTON_DIR=str(f / "runtime"),
                   XODUS_STEAM_DIR=str(h / "Steam"), FERESTRE_MARKET="US", FERESTRE_LANGUAGE="en-us",
                   GTK_A11Y="atspi", FERESTRE_APPLICATION_ID=APP_ID, GDK_BACKEND="wayland",
                   GTK_USE_PORTAL="0", WAYLAND_DISPLAY=SOCKET, FERESTRE_TEST_NATIVE_WAYLAND="1",
                   FERESTRE_TEST_WM="1", FERESTRE_TEST_PRIVATE_KWIN="1")
        for key in ("APPDIR", "APPIMAGE", "FERESTRE_PACKAGE_VERSION", "FERESTRE_SCRIPTS_DIR",
                    "LIBGL_ALWAYS_SOFTWARE", "GSK_RENDERER", "QT_QPA_PLATFORM", "XODUS_GAMES_DIR"):
            env.pop(key, None)
        if self.config["renderer"] != "default":
            env["GSK_RENDERER"] = self.config["renderer"]
        for key in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
            env[key] = "http://127.0.0.1:9"
        env["NO_PROXY"] = env["no_proxy"] = ""
        if SHOTS[self.shot][3]:
            env["LD_LIBRARY_PATH"] = str(f / "lib")
            env["FERESTRE_TEST_PAD_FILE"] = str(f / "pad")
        return env

    def pad(self, buttons=0, kind=2):
        import struct
        (self.fixture / "pad-next").write_bytes(struct.pack("<Hhhhh2xii", buttons, 0, 0, 0, 0, kind, 1))
        (self.fixture / "pad-next").replace(self.fixture / "pad")

    def capture(self, wait=1.5):
        """The virtual screen, through KWin's ScreenShot2."""
        time.sleep(wait)
        import gi
        gi.require_version("Gio", "2.0")
        from gi.repository import Gio, GLib
        from PIL import Image
        import threading
        bus = Gio.bus_get_sync(Gio.BusType.SESSION)
        r, w = os.pipe()
        fds = Gio.UnixFDList.new()
        index = fds.append(w)
        chunks = []

        def reader():
            while True:
                c = os.read(r, 1 << 20)
                if not c:
                    break
                chunks.append(c)
        thread = threading.Thread(target=reader, daemon=True)
        thread.start()
        result, _ = bus.call_with_unix_fd_list_sync(
            "org.kde.KWin", "/org/kde/KWin/ScreenShot2", "org.kde.KWin.ScreenShot2", "CaptureWorkspace",
            GLib.Variant("(a{sv}h)", ({"native-resolution": GLib.Variant("b", True)}, index)),
            GLib.VariantType("(a{sv})"), Gio.DBusCallFlags.NONE, -1, fds, None)
        os.close(w)
        meta = result.unpack()[0]
        need = meta["stride"] * meta["height"]
        until = time.monotonic() + 20
        while sum(map(len, chunks)) < need and time.monotonic() < until:
            time.sleep(0.05)
        data = b"".join(chunks)[:need]
        # QImage formats 4-6 are 32-bit xRGB words: BGRA bytes on little endian.
        raw = "BGRA" if meta.get("format", 4) in (4, 5, 6) else "RGBA"
        image = Image.frombuffer("RGBA", (meta["width"], meta["height"]), data, "raw", raw, meta["stride"], 1)
        out = self.scratch / "out" / f"{self.shot}.png"
        image.convert("RGB").save(out)
        print("captured", out.name, meta["width"], meta["height"], flush=True)

    def start(self):
        s = self.s
        s.Atspi.init()
        if SHOTS[self.shot][3]:
            self.pad()
        with (self.scratch / "out" / f"{self.shot}-gui.log").open("wb") as log:
            self.process = subprocess.Popen([self.fixture / "bin/ferestre-gui"], env=self.env(),
                                            stdout=log, stderr=log)
        self.app = s.find_app(self.process.pid, time.monotonic() + 30)
        self.frame = s.wait_for(self.app, lambda r: next(iter(s.find_all(r, role="frame")), None),
                                "the window", time.monotonic() + 30)

    def wait(self, test, what, limit=20):
        return self.s.wait_for(self.app, test, what, time.monotonic() + limit)

    def driver(self):
        from controller_driver import Driver
        return Driver(self.app, lambda buttons=0, kind=2: self.pad(buttons, kind), kind=2)

    def library(self):
        s = self.s
        if SHOTS[self.shot][2].get("fullscreen"):
            # Fullscreen has a row of section buttons instead of the sidebar.
            s.click(self.wait(lambda r: next(iter(s.find_all(r, role="button", name="My Library")), None),
                              "the My Library tab"))
        else:
            s.select_in_list(self.frame, "My Library")
        self.wait(lambda _: s.row_button(self.frame, self.names[UPDATE], "Update"), "the library to settle")

    def run(self):
        try:
            self.start()
            getattr(self, "scene_" + self.shot.replace("-", "_"))()
        except Exception:
            if self.config.get("debug") and getattr(self, "app", None) is not None:
                self.s.dump(self.app, "scene failure")
            raise
        finally:
            (self.fixture / "end-all").write_text("")
            for name in self.fixture.glob("launch-*"):
                (self.fixture / ("end-" + name.name[7:])).write_text("")
            if self.process is not None and self.process.poll() is None:
                self.process.terminate()
                try:
                    self.process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    self.process.kill()
            for game in self.games:
                if game.poll() is None:
                    game.kill()

    # One method per picture.
    def scene_home(self):
        time.sleep(6)
        self.capture(1)

    scene_home_ultrawide = scene_home

    def scene_library_handheld(self):
        self.library()
        time.sleep(2)
        self.driver().seek(f"{self.names[FOCUS]} · Play")
        self.capture(2)

    def start_running(self):
        s, name = self.s, self.names[RUNNING]
        self.library()
        (self.fixture / f"ready-{RUNNING}").write_text("")
        driver = self.driver()
        driver.seek(f"{name} · Play")
        driver.press(0, lambda root: s.row_button(root, name, "Stop"), "the game to run")
        self.wait(lambda _: s.row_button(self.frame, name, "Resume"), "the game to be up")
        time.sleep(1.5)  # the card is rebuilt once the game is up
        return driver

    def scene_running_game(self):
        name = self.names[RUNNING]
        driver = self.start_running()
        time.sleep(1)
        self.dismiss_notifications()
        driver.seek(f"{name} · Resume")
        self.capture(2)

    def dismiss_notifications(self):
        s = self.s
        for button in s.find_all(self.frame, name="Dismiss notification"):
            try:
                s.click(button)
            except Exception:
                pass
        time.sleep(1)

    def scene_stop_confirmation(self):
        s, name = self.s, self.names[RUNNING]
        driver = self.start_running()
        for attempt in range(4):
            driver.seek(f"{name} · Stop")
            time.sleep(0.5)
            driver.pulse(0)
            try:
                s.wait_for(self.app, lambda root: s.find_all(root, role="button", name="Stop Game"),
                           "the Stop dialog", time.monotonic() + 4)
                break
            except s.Failure:
                if attempt == 3:
                    raise
        self.capture(2.5)

    def scene_install_game(self):
        s, name = self.s, self.names[INSTALL]
        self.library()
        driver = self.driver()
        driver.seek(f"{name} · Install")
        driver.press(0, lambda root: any(t.startswith(f"Install {name}") for t in s.texts(root)),
                     "the install dialog")
        self.capture(3)

    def scene_notifications(self):
        s = self.s
        self.library()
        for pid, action in (("9MWR1NC6VQ6L", "Play"), ("9NNX1VVR3KNQ", "Play"), ("9NJWTJSVGVLJ", "Play")):
            control = f"{self.names[pid]} · {action}"
            for _ in range(5):
                before = len(self.alerts())
                s.click_named(self.frame, control)
                try:
                    s.wait_for(self.frame, lambda r: len(self.alerts()) > before, control,
                               time.monotonic() + 3)
                    break
                except s.Failure:
                    pass
        self.capture(1.5)

    def alerts(self):
        s, found = self.s, []
        for node, _ in s.walk(self.frame):
            try:
                if node.get_role_name() == "alert" and node.get_state_set().contains(s.Atspi.StateType.SHOWING):
                    found.append(node.get_name())
            except s.GLib.GError:
                continue
        return found

    def scene_compact_sidebar(self):
        s = self.s
        time.sleep(4)
        names = [x.get_name() or "" for x, _ in s.walk(self.frame)]
        account = next((n for n in names if ACCOUNT["gamertag"] in n), None)
        if self.config.get("debug"):
            print("NAMES", sorted(set(n for n in names if n)), flush=True)
        if account is None:
            raise s.Failure("no account control in the rail")
        s.click_named(self.frame, account)
        self.capture(2)

    def scene_already_running(self):
        s, name = self.s, self.names[RUNNING]
        self.library()
        prefix = DEMO_HOME / "xbox-games" / self.config["running_prefix"] / "pfx"
        prefix.mkdir(parents=True, exist_ok=True)
        game = subprocess.Popen(["C:\\Games\\Control\\Control.exe", "600"], executable="sleep",
                                env={"WINEPREFIX": f"{prefix}/", "PATH": os.environ["PATH"]})
        self.games.append(game)
        time.sleep(0.5)
        title = f"{name} is already running"
        for _ in range(3):
            s.click_named(self.frame, f"{name} · Play")
            try:
                s.wait_for(self.app, lambda r: title in s.texts(r), title, time.monotonic() + 8)
                break
            except s.Failure:
                continue
        else:
            raise s.Failure("no already-running dialog")
        self.capture(2.5)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", default="/var/tmp/ferestre-readme-target/release/ferestre-gui")
    parser.add_argument("--only", help="comma-separated picture names (default: all)")
    parser.add_argument("--out", help="output folder (default: docs/screenshots)")
    parser.add_argument("--catalog", default="~/.cache/ferestre/catalog",
                        help="catalogue cache to take public Store art from")
    parser.add_argument("--renderer", default="default", help="GSK renderer: default, gl, vulkan or cairo")
    parser.add_argument("--keep", action="store_true", help="keep the scratch folder")
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--stage", default="outer", help=argparse.SUPPRESS)
    parser.add_argument("--scratch", help=argparse.SUPPRESS)
    parser.add_argument("--shot", help=argparse.SUPPRESS)
    parser.add_argument("--size", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.stage == "outer":
        if os.environ.get("HOME") == str(DEMO_HOME):
            raise SystemExit("run this from a normal shell; it makes its own demo home")
        outer(args)
    elif args.stage == "ns":
        ns_stage(args)
    elif args.stage == "shoot":
        shoot_stage(args)
    else:
        raise SystemExit("unknown stage")


if __name__ == "__main__":
    main()

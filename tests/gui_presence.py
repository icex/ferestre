#!/usr/bin/env python3
"""Online status through the real window, the real `ferestre run` and a fake client.

The fake client advertises `presence-v1`. Its `presence run` logs its argv,
writes the events a test step asks for, and on stdin EOF writes `offline` and
exits, the way the real one does. `presence get` answers from a file the test
rewrites, plus the game on a PC while a runner has posted it, the way Xbox
would. No token, no network, no keyring: the client is a script.

    tests/gui_presence.py [--binary target/debug/ferestre-gui] [--keep]
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import time

import gui_smoke as s

OWNER = "11111111111111111111111111111111"
PLAYER = "22222222222222222222222222222222"
PRODUCT = "9TESTPRESENC"
TITLE = "Presence Fixture Title"
TIMEOUT = 20

CLIENT = r'''#!/usr/bin/env python3
import json,os,sys,threading,time
from pathlib import Path
root=Path(os.environ['HOME']);args=sys.argv[1:]
env_profile=os.environ.get('XODUS_ACCOUNT_PROFILE')
profile=env_profile or '1'*32;explicit=False
if '--profile' in args:
 i=args.index('--profile');profile=args[i+1];args=args[:i]+args[i+2:];explicit=True
profiles=json.loads((root/'profiles.json').read_text())
def log(entry):
 with (root/'client.log').open('a') as f:f.write(json.dumps(entry)+'\n')
log({'args':args,'profile':profile,'explicit':explicit,'env_profile':env_profile})
command=args[0]
def emit(event):
 sys.stdout.write(json.dumps(event)+'\n');sys.stdout.flush()
if command=='migrate-accounts':print('{}')
elif command=='account-profiles':print(json.dumps(profiles))
elif command=='account':
 p=next(p for p in profiles['profiles'] if p['id']==profile)
 if not p['signed_in']:sys.exit(1)
 print(json.dumps({'gamertag':p['label'],'xuid':'fixture-'+profile[0]}))
elif command=='library':
 print(json.dumps({'items':[{'productId':'9TESTPRESENC','productType':'Game','status':'Active'}]}))
elif command=='update':print(json.dumps({'version':'1.0.0.0'}))
elif command=='logout':
 for p in profiles['profiles']:
  if p['id']==profile:p['signed_in']=False
 (root/'profiles.json').write_text(json.dumps(profiles))
elif command=='presence' and args[1]=='get':
 status=json.loads((root/'presence-get.json').read_text())
 if (root/'posted').exists():
  status['state']='Online'
  status['devices']=status['devices']+[{'type':'PC','titles':[{'id':'1','name':(root/'posted').read_text()}]}]
 print(json.dumps(status))
elif command=='presence' and args[1]=='run':
 # Steps the test asks for, by file: each is taken once.
 def steps():
  while True:
   for name,event in (('runner-retry',{'event':'retrying','reason':'unavailable','in':30}),
                      ('runner-online',{'event':'online','title':'Presence Fixture Title','heartbeat_after':60})):
    try:os.unlink(root/name)
    except FileNotFoundError:continue
    emit(event)
   time.sleep(.05)
 (root/'posted').write_text('Presence Fixture Title')
 emit({'schema':1,'event':'online','title':'Presence Fixture Title','heartbeat_after':60})
 threading.Thread(target=steps,daemon=True).start()
 sys.stdin.read()
 try:os.unlink(root/'posted')
 except FileNotFoundError:pass
 log({'runner':'closed','profile':profile})
 emit({'event':'offline'})
elif command=='run':
 (root/'game-started').write_text(str(os.getpid()))
 deadline=time.monotonic()+120
 while not (root/'end-game').exists() and time.monotonic()<deadline:time.sleep(.05)
 os.unlink(root/'game-started')
else:sys.exit(2)
'''

SERVICE = r'''#!/usr/bin/env python3
# FERESTRE_ACCOUNT_ROLES_SERVICE_V1
import json,os,socket,sys
from pathlib import Path
if '--capabilities' in sys.argv:
 print(json.dumps({'schema':1,'capabilities':['store-player-accounts','title-scoped-tokens']}));sys.exit(0)
root=Path(os.environ['HOME']);path=Path(os.environ['XODUS_RUNTIME_DIR'])/'xodus.sock'
with (root/'services.jsonl').open('a') as log:
 log.write(json.dumps({'pid':os.getpid()})+'\n')
with socket.socket(socket.AF_UNIX) as server:
 server.bind(str(path));server.listen()
 while True:
  connection,_=server.accept();connection.close()
'''

ONLINE = {"schema": 1, "state": "Online",
          "devices": [{"type": "XboxSeries", "titles": [{"id": "750323071", "name": "Home"}]}]}
OFFLINE = {"schema": 1, "state": "Offline", "devices": []}


def write(path, text, executable=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    if executable:
        path.chmod(0o755)


def deadline():
    return time.monotonic() + TIMEOUT


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/ferestre-gui")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    home = Path(tempfile.mkdtemp(prefix="fp-"))
    gui = None
    # Each window's stderr in a file, checked for panics at the end.
    windows = s.Windows(str(home))
    passed = False
    panics = []
    try:
        s.check_panic_detection(str(home))
        profiles = {"schema": 1, "capabilities": ["store-player-accounts", "presence-v1"], "default_profile": OWNER,
                    "profiles": [{"id": OWNER, "label": "Owner Fixture", "signed_in": True, "legacy_saves": True, "save_namespace": "a" * 64},
                                 {"id": PLAYER, "label": "Player Fixture", "signed_in": True, "legacy_saves": False, "save_namespace": "b" * 64}]}
        write(home / "profiles.json", json.dumps(profiles))
        write(home / "presence-get.json", json.dumps(ONLINE))
        write(home / "client/xodus-cli", CLIENT, True)
        write(home / "client/xodus-service", SERVICE, True)
        write(home / "client/SERVICE_ID", hashlib.sha256(SERVICE.encode()).hexdigest())
        write(home / "runtime/proton", "#!/bin/sh\n# close_fds=False\nexit 0\n", True)
        write(home / "runtime/version", "0 ferestre-presence-fixture\n")
        write(home / "runtime/files/share/ferestre/capabilities.json", json.dumps({"schema": 1, "capabilities": [
            "xgameruntime.store-player-accounts", "xgameruntime.service-socket-override"]}))
        write(home / "runtime/files/bin/wineserver", "#!/bin/sh\nexit 0\n", True)
        write(home / "titles/presence-test.toml", f'''schema=1
[title]
product-id="{PRODUCT}"
name="Presence Test Game"
slug="presence-test"
[launch]
executable="Game.exe"
[status]
state="playable"
summary="Synthetic presence fixture"
''')
        game = home / "games/presence-test"
        write(game / "MicrosoftGame.config", f'<Game><Identity Name="Fixture.PresenceGame" Publisher="CN=Fixture" Version="1.0.0.0"/><ExecutableList><Executable Name="Game.exe"/></ExecutableList><StoreId>{PRODUCT}</StoreId></Game>')
        write(game / "Game.exe", "fixture")
        # The package header a finished install leaves: a folder with no header
        # and no manifest is a download that has not finished, not an install.
        write(game / ".xodus-streaming.msixvc", "")
        state = home / "state/ferestre"
        write(state / f"installed/{PRODUCT}.json", json.dumps({"product_id": PRODUCT, "dir": str(game), "content_ids": ["00000000-0000-0000-0000-000000000004"], "package_version": "1.0.0.0"}))
        write(home / "cache/ferestre/catalog" / f"{PRODUCT}.json", json.dumps({"schema": 1, "market": "US", "language": "en-us", "product": {"product_id": PRODUCT, "name": "Presence Test Game", "has_packages": True, "has_pc_package": True, "package_format": "MSIXVC"}}))
        settings = home / "config/ferestre/account-roles.json"
        write(settings, json.dumps({"store": OWNER, "player": PLAYER, "generation": 1}))
        preference = home / "config/ferestre/presence.json"
        # Without animations a switch flips as soon as it is activated. With
        # them GTK 4.14 ignores a second activation while the first is still
        # sliding, so "off and straight back on" would only ever click once.
        write(home / "config/gtk-4.0/settings.ini", "[Settings]\ngtk-enable-animations=0\n")
        (home / "bin").mkdir()
        shutil.copy2(binary, home / "bin/ferestre-gui")
        (home / "bin/ferestre").symlink_to(binary.with_name("ferestre"))
        env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"), XDG_CACHE_HOME=str(home / "cache"),
                   XODUS_CLI_DIR=str(home / "client"), XODUS_GAMES_DIR=str(home / "games"), XODUS_PROTON_DIR=str(home / "runtime"), XODUS_RUNTIME_DIR=str(home / "s"),
                   FERESTRE_TITLES=str(home / "titles"), GTK_A11Y="atspi", HTTPS_PROXY="http://127.0.0.1:9", HTTP_PROXY="http://127.0.0.1:9", ALL_PROXY="http://127.0.0.1:9")
        for key in ("APPIMAGE", "FERESTRE_ACCOUNT_ROLES", "XODUS_ACCOUNT_PROFILE", "XODUS_STORE_PROFILE", "XODUS_PLAYER_PROFILE"):
            env.pop(key, None)
        env["FERESTRE_APPLICATION_ID"] = "io.github.icex.ferestre.PresenceTest"

        def calls():
            path = home / "client.log"
            return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

        def presence_calls(kind):
            return [c for c in calls() if c.get("args", [None])[:2] == ["presence", kind]]

        def closed_runners():
            return [c for c in calls() if c.get("runner") == "closed"]

        def start():
            process = windows.start([home / "bin/ferestre-gui"], env, f"window {len(windows.logs) + 1}", stdout=None)
            app = s.find_app(process.pid, deadline())
            frame = s.wait_for(app, lambda root: next(iter(s.find_all(root, role="frame", name="Ferestre")), None), "main window", deadline())
            s.wait_for(frame, lambda root: next(iter(s.find_all(root, role="button", name_contains="Owner Fixture")), None), "account button", deadline())
            return process, app, frame

        def status(root, label):
            return s.wait_for(root, lambda r: next(iter(s.find_all(r, name=f"Status: {label}")), None), f"Status: {label}", deadline())

        def status_detail(root, label, wanted, present=True):
            def found(r):
                node = next(iter(s.find_all(r, name=f"Status: {label}")), None)
                return node is not None and (wanted in (node.get_description() or "")) == present
            what = f"Status: {label} {'with' if present else 'without'} {wanted!r}"
            return s.wait_for(root, found, what, deadline())

        def runners_up():
            return len(presence_calls("run")) - len(closed_runners())

        def account_line(root):
            return next((t for t in s.texts(root) if t.startswith("Player: ")), "")

        def open_menu(app, frame):
            s.click(s.find_all(frame, role="button", name_contains="Owner Fixture")[0])
            return s.wait_for(app, lambda root: next((n for n in s.find_all(root, name="Show me online") if n.get_role_name() != "label"), None),
                              "the Show me online switch", deadline())

        def close_menu():
            Atspi = s.Atspi
            Atspi.generate_keyboard_event(0xff1b, None, Atspi.KeySynthType.SYM)
            time.sleep(.3)

        def open_accounts(app, frame):
            # The window may still be refreshing the account or checking for
            # updates, and then Accounts only says so. Ask again until it opens.
            until = deadline()
            while time.monotonic() < until:
                open_menu(app, frame)
                s.click_named(app, "Accounts")
                try:
                    return s.wait_for(app, lambda root: next(iter(s.find_all(root, role="frame", name="Accounts")), None),
                                      "Accounts dialog", time.monotonic() + 3)
                except s.Failure:
                    close_menu()
            raise s.Failure("the Accounts dialog never opened")

        def checked(node):
            return node.get_state_set().contains(s.Atspi.StateType.CHECKED)

        def sensitive(node):
            return node.get_state_set().contains(s.Atspi.StateType.SENSITIVE)

        def caption(app, text):
            s.wait_for(app, lambda root: any(t == text for t in s.texts(root)), f"caption {text!r}", deadline())

        def play(frame):
            s.select_in_list(frame, "Installed")
            button = s.wait_for(frame, lambda root: s.row_button(root, "Presence Test Game", "Play"), "Play", deadline())
            s.click(button)
            s.wait_for(frame, lambda root: (home / "game-started").exists(), "the game to start", deadline())

        def end_game():
            write(home / "end-game", "end")
            s.wait_for(None, lambda _: not (home / "game-started").exists(), "the game to exit", deadline())
            time.sleep(.5)
            (home / "end-game").unlink()

        def no_status_toast(root, why):
            s.check(not any(t.startswith("Online status:") for t in s.texts(root)), why, root)

        s.Atspi.init()
        gui, app, frame = start()

        # The badge is Xbox's answer, read as the Player and only the Player.
        status(frame, "Online")
        s.check("Player: Player Fixture" in account_line(frame) and account_line(frame).endswith("· Online"),
                "the status follows the Player's line, which keeps its text", frame)
        gets = presence_calls("get")
        s.check(gets and all(c["explicit"] and c["profile"] == PLAYER for c in gets), "presence get names the Player, never the Store account")

        # On by default after sign-in.
        switch = open_menu(app, frame)
        s.check(checked(switch) and sensitive(switch), "Show me online is on by default", app)
        caption(app, "Friends will see the games you play from Ferestre")
        close_menu()
        s.check(not preference.exists(), "the default is not written until it is changed")

        # A game launched from the window: its launch starts the runner.
        play(frame)
        runner = s.wait_for(frame, lambda root: next(iter(presence_calls("run")), None), "the presence runner", deadline())
        s.check(runner["explicit"] and runner["profile"] == PLAYER and runner["env_profile"] is None,
                "the runner is the Player's, with no Store account inherited")
        s.check(runner["args"] == ["presence", "run", "--json", "--game-dir", str(game)], "the runner gets the game directory")
        status(frame, f"Playing {TITLE}")
        s.check("Player: Player Fixture" in account_line(frame), "Player label kept while playing", frame)

        # A runner reconnecting changes the dot's description, not what Xbox
        # shows, which is what the dot and its name say.
        write(home / "runner-retry", "")
        status_detail(frame, f"Playing {TITLE}", "Reconnecting: Xbox is unavailable")
        write(home / "runner-online", "")
        status_detail(frame, f"Playing {TITLE}", "Reconnecting", present=False)
        no_status_toast(frame, "a retry that was not asked for is shown in the badge only")

        # Switched off while playing: the runner's stdin closes, the setting is saved.
        write(home / "presence-get.json", json.dumps(OFFLINE))
        switch = open_menu(app, frame)
        caption(app, f"Playing {TITLE}")
        s.click(switch)
        s.wait_for(frame, lambda root: preference.exists() and not json.loads(preference.read_text())["players"][PLAYER]["share_games"],
                   "the switch to be saved", deadline())
        s.wait_for(frame, lambda root: len(closed_runners()) == 1, "the runner's stdin to close", deadline())
        caption(app, "Friends see your status from other devices only")
        status(frame, "Off")
        s.check((home / "game-started").exists(), "switching off leaves the game running")

        # Switched on again while the same game runs: a new runner.
        s.click(switch)
        s.wait_for(frame, lambda root: len(presence_calls("run")) == 2, "a second runner", deadline())
        s.wait_for(frame, lambda root: json.loads(preference.read_text())["players"][PLAYER]["share_games"], "the switch on again", deadline())
        status(frame, f"Playing {TITLE}")

        # Off and straight back on, before the runner has answered: one runner
        # is up at the end, whichever signal the launch took first.
        s.click(switch)
        s.click(switch)
        s.wait_for(frame, lambda root: json.loads(preference.read_text())["players"][PLAYER]["share_games"], "the switch on", deadline())
        time.sleep(1)
        s.wait_for(frame, lambda root: runners_up() == 1, "one runner up after off and on", deadline())
        status(frame, f"Playing {TITLE}")
        # On... and straight back off: none is left up. This is the one that
        # must never be wrong.
        s.click(switch)
        s.wait_for(frame, lambda root: runners_up() == 0, "the runner to stop", deadline())
        status(frame, "Off")
        s.click(switch)
        s.click(switch)
        s.wait_for(frame, lambda root: not json.loads(preference.read_text())["players"][PLAYER]["share_games"], "the switch off", deadline())
        time.sleep(1)
        s.wait_for(frame, lambda root: runners_up() == 0, "no runner up after on and off", deadline())
        status(frame, "Off")
        caption(app, "Friends see your status from other devices only")
        s.click(switch)
        s.wait_for(frame, lambda root: runners_up() == 1, "a runner once it is on again", deadline())
        status(frame, f"Playing {TITLE}")
        close_menu()

        # The game exits: its runner is stopped with it.
        end_game()
        s.wait_for(frame, lambda root: runners_up() == 0, "the runner to stop with the game", deadline())
        status(frame, "Offline")

        # Closing the window does not end a game, so it does not end its status.
        runs = len(presence_calls("run"))
        play(frame)
        s.wait_for(frame, lambda root: len(presence_calls("run")) == runs + 1, "a runner for the next launch", deadline())
        status(frame, f"Playing {TITLE}")
        s.click_named(frame, "Close")
        # The window asks first, since the game keeps running without it.
        keep = s.wait_for(frame, lambda root: next(iter(s.find_all(root, role="button", name="Keep Open")), None),
                          "the close-while-running question", deadline())
        s.click_named(keep.get_parent(), "Close")
        s.check(gui.wait(timeout=10) == 0, "the window closes while a game runs")
        time.sleep(2)
        s.check(runners_up() == 1, "the runner outlives the window, like the game")
        end_game()
        s.wait_for(None, lambda _: runners_up() == 0, "the runner to stop once the game exits", deadline())

        # A Player signing out: the runner says goodbye before the token goes.
        gui, app, frame = start()
        runs = len(presence_calls("run"))
        play(frame)
        s.wait_for(frame, lambda root: len(presence_calls("run")) == runs + 1, "a runner after restarting", deadline())
        status(frame, f"Playing {TITLE}")
        dialog = open_accounts(app, frame)
        s.click_named(dialog, "Sign out Player Fixture")
        s.wait_for(frame, lambda root: any(c.get("args") == ["logout"] for c in calls()), "the Player's logout", deadline())
        log = calls()
        logout = next(i for i, c in enumerate(log) if c.get("args") == ["logout"])
        closed = [i for i, c in enumerate(log) if c.get("runner") == "closed"]
        s.check(log[logout]["profile"] == PLAYER, "the Player is the account signed out")
        s.check(len(closed) == runs + 1 and closed[-1] < logout, "the runner is stopped before logout runs")
        s.wait_for(frame, lambda root: not s.find_all(root, name_contains="Status: "), "the badge to go with the Player", deadline())
        switch = open_menu(app, frame)
        s.check(not sensitive(switch), "the switch cannot be used without a Player", app)
        caption(app, "Sign in the Player account to share your status")
        close_menu()
        end_game()
        s.click_named(frame, "Close")
        s.check(gui.wait(timeout=10) == 0, "the signed-out window closes")

        # A client without presence: nothing is asked, nothing is said.
        profiles["capabilities"] = ["store-player-accounts"]
        profiles["profiles"][1]["signed_in"] = True
        write(home / "profiles.json", json.dumps(profiles))
        before = len(calls())
        gui, app, frame = start()
        switch = open_menu(app, frame)
        caption(app, "Update the Xodus client to share your status")
        s.check(not sensitive(switch), "an old client leaves the switch insensitive", app)
        close_menu()
        s.check(not s.find_all(frame, name_contains="Status: "), "an old client shows no status", frame)
        no_status_toast(frame, "an old client raises no toast")
        s.check(not any(c.get("args", [None])[0] == "presence" for c in calls()[before:]), "an old client is never asked about presence")
        s.click_named(frame, "Close")
        s.check(gui.wait(timeout=10) == 0, "the old-client window closes")
        passed = True
    finally:
        write(home / "end-game", "end")
        if gui is not None and gui.poll() is None:
            gui.terminate()
            gui.wait(timeout=10)
        panics = windows.problems()
        service_log = home / "services.jsonl"
        if service_log.exists():
            for line in service_log.read_text().splitlines():
                try:
                    os.kill(json.loads(line)["pid"], 15)
                except ProcessLookupError:
                    pass
        if args.keep:
            print("kept", home)
        else:
            time.sleep(1)
            shutil.rmtree(home, ignore_errors=True)
    if panics:
        raise s.Failure("; ".join(panics))
    if passed:
        print("all presence GUI checks passed")


if __name__ == "__main__":
    main()

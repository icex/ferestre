#!/usr/bin/env python3
"""A local game's row, menu, anti-cheat caution, Add to Steam and forget-only removal."""
import argparse
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import time
import zlib
import gui_smoke as s
from gui_launch import prepare
from gui_updater import write
from controller_driver import start_window_manager

TITLE = 'Local Test Game'


def png():
    """A 1x1 PNG, so the art copied into Steam is a real picture."""
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 1, 1, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(b'\0\x30\x60\x90')) + chunk(b'IEND', b''))


def steam_running():
    """Ferestre refuses to write shortcuts while any Steam runs on the machine."""
    for pid in filter(str.isdigit, os.listdir('/proc')):
        try:
            if Path(f'/proc/{pid}/comm').read_text().strip() == 'steam':
                return True
        except OSError:
            pass
    return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/ferestre-gui')
    args = parser.parse_args()
    s.Atspi.init()
    # The row menu helpers only see the items they know by name.
    s.MENU_ITEMS.add('Remove from library')
    process = app = wm = None
    with tempfile.TemporaryDirectory(prefix='ferestre-local-games-') as tmp:
        h = Path(tmp)
        _, _, pad, env = prepare(h, args.binary)
        # The real CLI, so Remove from library runs the real forget.
        shutil.copy2(Path(args.binary).resolve().with_name('ferestre'), h / 'bin/ferestre')
        game = h / 'drive/Local Test'
        write(game / 'Game.exe', 'MZ')
        write(game / 'Randgrid.sys', 'driver')
        recipe = h / 'config/ferestre/titles/LOCAL-TEST.toml'
        write(recipe, f'''schema = 1
[title]
product-id = "LOCAL-TEST"
name = "{TITLE}"
slug = "local-test"
kind = "local"
[install]
dir = "{game}"
[launch]
executable = "Game.exe"
[status]
state = "untested"
summary = "Added by hand"
''')
        art = png()
        (h / 'state/ferestre/local-art/LOCAL-TEST').mkdir(parents=True)
        (h / 'state/ferestre/local-art/LOCAL-TEST/portrait.png').write_bytes(art)
        shortcuts = h / 'steam/userdata/1/config/shortcuts.vdf'
        shortcuts.parent.mkdir(parents=True)
        try:
            wm = start_window_manager()
            with (h / 'gui.log').open('wb') as log:
                process = subprocess.Popen([h / 'bin/ferestre-gui'], env=env, stdout=log, stderr=log)
            app = s.find_app(process.pid, time.monotonic() + 20)

            def wait(test, what):
                return s.wait_for(app, test, what, time.monotonic() + s.TIMEOUT)
            frame = wait(lambda root: next(iter(s.find_all(root, role='frame', name='Ferestre')), None), 'window')
            s.select_in_list(frame, 'My Library')
            wait(lambda _: s.row_button(frame, TITLE, 'Play'), 'the local game offers Play')
            s.check(any('Ricochet' in t for t in s.texts(frame)), 'the anti-cheat file is named on the row', frame)
            offers = s.row_menu_offers(app, frame, TITLE)
            s.check('Remove from library' in offers, f'the menu offers Remove from library ({offers!r})', frame)
            for absent in ('Move location', 'Delete'):
                s.check(absent not in offers, f'a local game has no {absent} ({offers!r})', frame)

            s.click_named(frame, 'Add games')
            wait(lambda root: s.find_all(root, role='button', name='Add game…'), 'the add menu offers Add game…')
            s.check(bool(s.find_all(app, role='button', name='Import from Steam')), 'the add menu offers Import from Steam', frame)
            s.click_named(frame, 'Add games')
            wait(lambda root: not s.find_all(root, role='button', name='Import from Steam') or None, 'the add menu to close')

            if steam_running():
                print('SKIP: Add to Steam, because Steam is running on this machine', flush=True)
            else:
                s.row_action(app, frame, 'Add to Steam', TITLE)
                wait(lambda root: any(t.startswith(f'Added "{TITLE}" in Steam.') for t in s.texts(root)) or None,
                     'Add to Steam to finish')
                data = shortcuts.read_bytes()
                s.check(b'run LOCAL-TEST' in data, 'the Steam shortcut runs the local game', frame)
                app_id = struct.unpack('<I', data.split(b'\x02appid\0', 1)[1][:4])[0]
                grid = shortcuts.parent / 'grid'
                s.check((grid / f'{app_id}p.png').read_bytes() == art, "Steam gets the local game's own portrait", frame)

            s.row_action(app, frame, 'Remove from library', TITLE)
            wait(lambda root: any('Nothing is deleted' in t for t in s.texts(root)) or None, 'the forget dialog')
            s.click_named(app, 'Remove')
            wait(lambda _: not recipe.exists() or None, 'the recipe to be forgotten')
            s.check((game / 'Game.exe').is_file() and (game / 'Randgrid.sys').is_file(), "the game's files are kept", frame)
            print('PASS: local game row, menu, anti-cheat caution, Add to Steam and forget-only removal', flush=True)
        except Exception:
            print((h / 'gui.log').read_text(), flush=True)
            if app is not None:
                s.dump(app, 'local games failure')
            raise
        finally:
            pad()
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            if wm is not None:
                wm.terminate()
                wm.wait(timeout=5)


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Move an installation through the real window/CLI, across real filesystems.

An isolated HOME contains only synthetic game data. No accounts or services
are used. Retail launch after relocation is a separate qualification step.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

import gui_smoke as s
from gi.repository import Atspi

PRODUCT = '9TESTMOVETTL'
TITLE = 'Move Test Game'


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/release/ferestre-gui')
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    owner = subprocess.run(['gdbus','call','--session','--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus','--method','org.freedesktop.DBus.NameHasOwner','io.github.icex.ferestre'],capture_output=True,text=True)
    if 'true' in owner.stdout:
        raise s.Failure('Close the existing Ferestre window before this isolated GUI test')
    with tempfile.TemporaryDirectory(prefix='ferestre-move-ui-') as tmp, tempfile.TemporaryDirectory(prefix='ferestre-move-ui-', dir='/dev/shm') as dest:
        home = Path(tmp)
        game = home / 'games/move-test'
        state = home / 'state/ferestre'
        write(home/'titles/test.toml', f'''schema=1
[title]
product-id="{PRODUCT}"
name="{TITLE}"
slug="move-test"
[launch]
executable="Game.exe"
[status]
state="playable"
summary="Synthetic move fixture"
''')
        write(game/'MicrosoftGame.config', f'<Game><Identity Name="Fixture.Move" Version="1.0.0.0"/><StoreId>{PRODUCT}</StoreId></Game>')
        write(game/'.xodus-streaming.msixvc', 'synthetic header')
        payload = game/'payload.bin'
        with payload.open('wb') as file:
            for _ in range(1024): file.write(b'move verification'*65536)
        expected = hashlib.file_digest(payload.open('rb'), 'sha256').hexdigest()
        record = state/f'installed/{PRODUCT}.json'
        write(record, json.dumps(dict(product_id=PRODUCT,dir=str(game),content_ids=[],package_version='1.0.0.0')))
        env = dict(os.environ, HOME=tmp, XDG_CONFIG_HOME=str(home/'config'), XDG_STATE_HOME=str(home/'state'), XDG_CACHE_HOME=str(home/'cache'),
                   FERESTRE_TITLES=str(home/'titles'), XODUS_GAMES_DIR=str(home/'games'), XODUS_CLI_DIR=str(home/'client'),
                   XODUS_STEAM_DIR=str(home/'steam'), XODUS_PROTON_DIR=str(home/'runtime'), GTK_A11Y='atspi',
                   HTTPS_PROXY='http://127.0.0.1:9', HTTP_PROXY='http://127.0.0.1:9', ALL_PROXY='http://127.0.0.1:9')
        for key in ('APPIMAGE','FERESTRE_STATE_DIR','FERESTRE_CONFIG_DIR','XODUS_ACCOUNT_PROFILE','XODUS_STORE_PROFILE','XODUS_PLAYER_PROFILE','FERESTRE_ACCOUNT_ROLES'):
            env.pop(key,None)
        with (home/'gui.log').open('wb') as log:
            gui = subprocess.Popen([str(binary)],env=env,stdout=log,stderr=log)
        try:
            app=s.find_app(gui.pid,time.monotonic()+20)
            frame=s.wait_for(app,lambda r:next(iter(s.find_all(r,role='frame')),None),'frame',time.monotonic()+20)
            s.select_in_list(frame,'Installed')
            s.wait_for(frame,lambda r:s.row_menus(r).get(TITLE),'installed game',time.monotonic()+20)
            time.sleep(1)
            s.row_action(app,frame,'Move location',TITLE)
            dialog=s.wait_for(app,lambda r:next((n for n,d in s.walk(r) if n.get_name()==f'Move {TITLE}?'),None),'move dialog',time.monotonic()+20)
            entry=s.wait_for(dialog,lambda r:next((n for n in s.find_all(r,role='text') if n.get_editable_text_iface()),None),'destination entry',time.monotonic()+10)
            # The source parent is an explicit collision, which must disable Move.
            Atspi.EditableText.set_text_contents(entry.get_editable_text_iface(),str(game.parent))
            s.click_named(dialog,'Check space')
            s.wait_for(dialog,lambda r:any('already' in t for t in s.texts(r)),'collision refusal',time.monotonic()+10)
            assert json.loads(record.read_text())['dir']==str(game)
            Atspi.EditableText.set_text_contents(entry.get_editable_text_iface(),dest)
            s.click_named(dialog,'Check space')
            s.wait_for(dialog,lambda r:any('free' in t and 'Includes copying' in t for t in s.texts(r)),'space preflight',time.monotonic()+10)
            s.click_named(dialog,'Move')
            s.wait_for(app,lambda r:any('Moving '+TITLE in t for t in s.texts(r)),'progress',time.monotonic()+10)
            s.wait_for(app,lambda r:json.loads(record.read_text())['dir']!=str(game),'record update',time.monotonic()+30)
            destination=Path(json.loads(record.read_text())['dir'])
            s.wait_for(app,lambda r:not game.exists(),'source cleanup',time.monotonic()+10)
            assert hashlib.file_digest((destination/'payload.bin').open('rb'),'sha256').hexdigest()==expected
            assert destination == Path(dest)/game.name
            # Request cancellation through the real button while a larger
            # copy is active, then check both record and original content.
            with (destination/'cancel-payload').open('wb') as file:
                file.truncate(1024*1024*1024)
            library=home/'cancel-library'
            library.mkdir()
            s.wait_for(app,lambda r:not any(t.startswith('Moving '+TITLE) for t in s.texts(r)), 'move completion',time.monotonic()+10)
            s.row_action(app,frame,'Move location',TITLE)
            dialog=s.wait_for(app,lambda r:next((n for n,d in s.walk(r) if n.get_name()==f'Move {TITLE}?'),None),'second move dialog',time.monotonic()+10)
            entry=next(n for n in s.find_all(dialog,role='text') if n.get_editable_text_iface())
            Atspi.EditableText.set_text_contents(entry.get_editable_text_iface(),str(library))
            s.click_named(dialog,'Check space')
            s.wait_for(dialog,lambda r:any('free' in t and 'Includes copying' in t for t in s.texts(r)),'second space preflight',time.monotonic()+10)
            s.click_named(dialog,'Move')
            cancel=s.wait_for(app,lambda r:next(iter(s.find_all(r,role='button',name='Cancel Move')),None),'Cancel Move',time.monotonic()+10)
            s.click(cancel)
            s.wait_for(app,lambda r:any('Original installation kept' in t for t in s.texts(r)),'cancel cleanup',time.monotonic()+20)
            assert json.loads(record.read_text())['dir']==str(destination)
            assert destination.is_dir() and not (library/destination.name).exists()
            print('PASS: GUI collision refusal, space check, progress, cross-filesystem move, SHA256 readback, source cleanup and Cancel Move')
        except BaseException:
            print((home/'gui.log').read_text(errors='replace'))
            print('Install record:', record.read_text())
            raise
        finally:
            gui.terminate()
            gui.wait(timeout=15)


if __name__=='__main__':
    main()

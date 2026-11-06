#!/usr/bin/env python3
"""Update installed games in place without asking for a new install folder."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time
import gui_smoke as s
from gui_launch import prepare
from gui_transfers import CLI
from gui_updater import write
from controller_driver import Driver, start_window_manager


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/ferestre-gui')
    args = parser.parse_args()
    s.Atspi.init()
    process = app = wm = None
    with tempfile.TemporaryDirectory(prefix='ferestre-update-existing-') as tmp:
        home = Path(tmp)
        ids, _, pad, env = prepare(home, args.binary)
        client = CLI.replace('id=sys.argv[2]', "id=sys.argv[2]\nwith (h/'arguments').open('a') as out:out.write(json.dumps(sys.argv[1:])+'\\n')")
        write(home / 'bin/ferestre', client, True)
        folders = []
        for n, id in enumerate(ids[:2]):
            folder = home / f'custom library/game {n}/renamed-install'
            folders.append(folder)
            write(folder / 'appxmanifest.xml', '<Package><Identity Name="Update.Fixture" Version="1.0.0.0" /></Package>')
            write(folder / 'keep-existing-data', f'existing game {n}')
            record = {'product_id': id, 'dir': str(folder), 'content_ids': ['fixture-content'], 'package_version': '1.0.0.0'}
            write(home / f'state/ferestre/installed/{id}.json', json.dumps(record))
            write(home / f'state/ferestre/updates/{id}.json', json.dumps({'schema': 1, 'record': record, 'available': '2.0.0.0'}))
        try:
            wm = start_window_manager()
            with (home / 'gui.log').open('wb') as log:
                process = subprocess.Popen([home / 'bin/ferestre-gui'], env=env, stdout=log, stderr=log)
            app = s.find_app(process.pid, time.monotonic() + 20)
            def wait(test, what):
                return s.wait_for(app, test, what, time.monotonic() + 20)
            def queue():
                file = home / 'state/ferestre/install-queue.json'
                return json.loads(file.read_text())['jobs'] if file.exists() else []
            def calls():
                file = home / 'arguments'
                return [json.loads(line) for line in file.read_text().splitlines()] if file.exists() else []
            frame = wait(lambda r: next(iter(s.find_all(r, role='frame')), None), 'window')
            time.sleep(.5)
            s.select_in_list(frame, 'Updates')
            s.click(wait(lambda r: s.row_button(r, 'Launch Game 0', 'Update'), 'fresh update action'))
            wait(lambda r: calls() or s.find_all(r, role='button', name='Choose install folder'), 'update starts or asks for a folder')
            s.check(not s.find_all(app, role='button', name='Choose install folder'), 'fresh Update opens no install-location prompt')
            wait(lambda _: calls(), 'existing-folder update starts')
            s.check(calls()[0] == ['install', ids[0], str(folders[0])], 'Update passes the exact recorded folder, including custom names and nesting')
            s.check(queue()[0]['updating'] and queue()[0]['destination'] == str(folders[0]), 'queue preserves update identity and installed destination')
            driver = Driver(app, pad)
            driver.seek('Launch Game 1')
            driver.press(0, lambda _: len(queue()) == 2, 'controller queues a second update')
            s.check(len(calls()) == 1, 'another update waits behind the active transfer')
            s.check(queue()[1]['updating'] and queue()[1]['destination'] == str(folders[1]), 'queued update keeps its own existing folder')
            (home / ('finish-' + ids[0])).touch()
            wait(lambda _: len(calls()) == 2, 'next update starts after the active transfer finishes')
            s.check(calls()[1] == ['install', ids[1], str(folders[1])], 'queued update also uses the exact recorded folder')
            for n, folder in enumerate(folders):
                s.check((folder / 'keep-existing-data').read_text() == f'existing game {n}', 'existing files remain intact')
            button = wait(lambda r: s.row_button(r, 'Launch Game 0', 'Update'), 'completed fixture still offers cached update')
            folders[0].rename(folders[0].with_name('disconnected-install'))
            s.click(button)
            wait(lambda r: any('installed game folder is unavailable' in t for t in s.texts(r)), 'unavailable installed folder is explained')
            s.check(len(queue()) == 1 and queue()[0]['product_id'] == ids[1], 'missing folder never queues a replacement installation elsewhere')
            s.check(not s.find_all(app, role='button', name='Choose install folder'), 'missing folder does not fall back to an install prompt')
            s.click_named(frame, 'Stop Download')
            wait(lambda _: (home / 'events').exists() and 'stop ' + ids[1] in (home / 'events').read_text(), 'test update stops cooperatively')
            s.click_named(frame, 'Close')
            s.check(process.wait(timeout=10) == 0, 'isolated update window closes cleanly')
            print('PASS: mouse/controller Update uses installed folders and queues behind active work', flush=True)
        except Exception:
            print((home / 'gui.log').read_text(), flush=True)
            if app is not None:
                s.dump(app, 'update-existing failure')
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

#!/usr/bin/env python3
"""Library reload preserves failed snapshots and removes stale recipe-only rows."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time
import gui_smoke as s
from gui_launch import prepare
from gui_updater import write
from controller_driver import start_window_manager

CLIENT = '''#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
home=Path(os.environ['HOME'])
if sys.argv[1:2]==['library']:
 if (home/'fail-library').exists():
  print(json.dumps({'code':'Unauthorized','message':'raw response should remain private','innererror':{'code':'InvalidMsaTicketFormat'}}))
  print('HTTP 401 Unauthorized (MS-CV synthetic)',file=sys.stderr)
  sys.exit(1)
 print((home/'library-response.json').read_text())
 sys.exit(0)
sys.exit(1)
'''


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/ferestre-gui')
    args = parser.parse_args()
    s.Atspi.init()
    process = app = wm = None
    with tempfile.TemporaryDirectory(prefix='ferestre-library-error-') as tmp:
        home = Path(tmp)
        ids, _, pad, env = prepare(home, args.binary)
        write(home / 'client/xodus-cli', CLIENT, True)
        write(home / 'state/ferestre/account.json', json.dumps({'gamertag': 'Reload Fixture', 'xuid': '0000000000000000'}))
        (home / 'fail-library').touch()
        cache = home / 'state/ferestre/library.json'
        original = json.loads(cache.read_text())
        former = '9PPT8K6GQHRZ'
        never_owned = '9ZZRECIPE001'
        for product_id, name in [(former, 'Former Pass Game'), (never_owned, 'Recipe Only Game')]:
            recipe = (home / f'titles/{ids[0]}.toml').read_text()
            write(home / f'titles/{product_id}.toml', recipe.replace(ids[0], product_id).replace('Launch Game 0', name).replace('launch-0', product_id.lower()))
            product = json.loads((home / f'cache/ferestre/catalog/{ids[0]}.json').read_text())
            product['product'].update(product_id=product_id, name=name)
            write(home / f'cache/ferestre/catalog/{product_id}.json', json.dumps(product))
        cached = dict(original, count=4, items=original['items'] + [{'productId': former, 'productType': 'Game', 'status': 'Active'}])
        write(cache, json.dumps(cached))
        saved = cache.read_bytes()
        refreshed = [item for item in original['items'] if item['productId'] != ids[1]]
        write(home / 'library-response.json', json.dumps({'count': len(refreshed), 'items': refreshed}))
        try:
            wm = start_window_manager()
            with (home / 'gui.log').open('wb') as log:
                process = subprocess.Popen([home / 'bin/ferestre-gui'], env=env, stdout=log, stderr=log)
            app = s.find_app(process.pid, time.monotonic() + 20)
            def wait(test, what):
                return s.wait_for(app, test, what, time.monotonic() + 20)
            frame = wait(lambda r: next(iter(s.find_all(r, role='frame')), None), 'window')
            s.select_in_list(frame, 'My Library')
            wait(lambda r: s.row_button(r, 'Former Pass Game', 'Install'), 'cached account title')
            s.check(not s.find_all(frame, name_contains='Recipe Only Game'), 'a compatibility recipe alone never populates My Library')
            def reload():
                button = wait(lambda r: next(iter(s.find_all(r, role='button', name_contains='Reload Fixture')), None), 'account menu')
                s.click(button)
                wait(lambda r: s.find_all(r, role='button', name='Reload library'), 'Reload library action')
                s.click_named(app, 'Reload library')
            reload()
            wait(lambda r: any('Microsoft rejected the Store sign-in ticket' in t for t in s.texts(r)), 'actionable Store authentication error')
            s.check(cache.read_bytes() == saved, 'failed reload preserves the saved library')
            s.check(bool(s.row_button(frame, 'Former Pass Game', 'Install')), 'failed refresh retains the last known account title')
            labels = s.texts(app)
            s.check(not any('/tmp/.mount' in t or 'raw response should remain private' in t for t in labels), 'error shows no internal AppImage path or raw service body')
            (home / 'fail-library').unlink()
            reload()
            wait(lambda _: len(json.loads(cache.read_text())['items']) == 2, 'Reload succeeds after the service recovers')
            s.check(len(json.loads(cache.read_text())['items']) == 2, 'successful reload replaces the ownership snapshot')
            wait(lambda r: not s.find_all(r, name_contains='Former Pass Game'), 'uninstalled former account title disappears despite its recipe')
            s.check(bool(s.row_button(frame, 'Launch Game 1', 'Play')), 'an installed game stays manageable after ownership disappears')
            s.click_named(frame, 'Close')
            s.check(process.wait(timeout=10) == 0, 'library-error fixture closes cleanly')
            print('PASS: library refresh preserves failures, removes recipe-only titles and retains installed games', flush=True)
        except Exception:
            print((home / 'gui.log').read_text(), flush=True)
            if app is not None:
                s.dump(app, 'library-error failure')
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

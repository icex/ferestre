#!/usr/bin/env python3
"""Real GUI + loopback image server: progressive covers and catalogue prefetch."""
import argparse
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
import threading
import time
from urllib.parse import urlsplit
import zlib

import gui_smoke as s


def png():
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    rows = (b'\0' + b'\x20\x80\x40' * 600) * 900
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 600, 900, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/ferestre-gui')
    parser.add_argument('--fullscreen', action='store_true', help='also exercise a real window manager at 720p')
    args = parser.parse_args()
    image = png()
    lock = threading.Lock()
    release = threading.Event()
    counts = Counter()
    active = peak = 0
    ids = [f'9ZZART{n:06}' for n in range(60)]

    class Images(BaseHTTPRequestHandler):
        def do_GET(self):
            nonlocal active, peak
            path = urlsplit(self.path).path
            with lock:
                active += 1
                peak = max(peak, active)
                counts[path] += 1
            try:
                if path == '/' + ids[0]:
                    release.wait(20)
                time.sleep(.025)
                self.send_response(200)
                self.send_header('Content-Type', 'image/png')
                self.send_header('Content-Length', str(len(image)))
                self.end_headers()
                self.wfile.write(image)
            finally:
                with lock:
                    active -= 1

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Images)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    process = None
    with tempfile.TemporaryDirectory(prefix='ferestre-artwork-gui-') as tmp:
        home = Path(tmp)
        def write(path, value, executable=False):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)
            if executable:
                path.chmod(0o755)
        binary = Path(args.binary).resolve()
        (home / 'bin').mkdir()
        shutil.copy2(binary, home / 'bin/ferestre-gui')
        write(home / 'bin/ferestre', '#!/bin/sh\necho "{}"\n', True)
        write(home / 'client/xodus-cli', '#!/bin/sh\nexit 1\n', True)
        write(home / 'runtime/proton', '#!/bin/sh\nexit 0\n', True)
        write(home / 'runtime/version', 'artwork-fixture\n')
        write(home / 'titles/first.toml', f'schema=1\n[title]\nproduct-id="{ids[0]}"\nname="Artwork Game 000"\nslug="artwork-first"\n[launch]\nexecutable="Game.exe"\n[status]\nstate="untested"\nsummary="Offline artwork fixture"\n')
        schema = int(re.search(r'const CACHE_SCHEMA: u32 = (\d+);', Path('crates/ferestre-core/src/catalog.rs').read_text())[1])
        listing_schema = int(re.search(r'const LISTING_SCHEMA: u32 = (\d+);', Path('crates/ferestre-core/src/gamepass.rs').read_text())[1])
        write(home / 'state/ferestre/gamepass.json', json.dumps({
            'schema': listing_schema, 'fetched': 0, 'market': 'US', 'product_ids': ids,
            'tiers': {'included': {id: ['PCSubMetadata'] for id in ids}}
        }))
        cache = home / 'cache/ferestre/catalog'
        for n, id in enumerate(ids):
            write(cache / (id + '.json'), json.dumps({
                'schema': schema, 'market': 'US', 'language': 'en-us', 'product': {
                    'product_id': id, 'name': f'Artwork Game {n:03}',
                    # Portrait-only metadata must work even without a square icon.
                    'artwork': {'portrait': f'http://127.0.0.1:{server.server_port}/{id}'},
                    'has_packages': True, 'has_pc_package': True, 'package_format': 'MSIXVC'
                }
            }))
        env = dict(os.environ, HOME=tmp, XDG_CONFIG_HOME=str(home / 'config'),
                   XDG_STATE_HOME=str(home / 'state'), XDG_CACHE_HOME=str(home / 'cache'),
                   XDG_DATA_HOME=str(home / 'data'), FERESTRE_TITLES=str(home / 'titles'),
                   XODUS_CLI_DIR=str(home / 'client'), XODUS_PROTON_DIR=str(home / 'runtime'),
                   XODUS_GAMES_DIR=str(home / 'games'), XODUS_STEAM_DIR=str(home / 'steam'),
                   FERESTRE_MARKET='US', FERESTRE_LANGUAGE='en-us',
                   GTK_A11Y='atspi', FERESTRE_APPLICATION_ID='io.github.icex.ferestre.ArtworkTest')
        for key in ('APPDIR', 'APPIMAGE', 'FERESTRE_PACKAGE_VERSION'):
            env.pop(key, None)
        for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy'):
            env[key] = 'http://127.0.0.1:9'
        env['NO_PROXY'] = env['no_proxy'] = '127.0.0.1'
        def covered():
            return list((cache / 'images').glob('*-cover-600.png'))
        wm = None
        if args.fullscreen and os.environ.get('FERESTRE_TEST_WM') != '1':
            with (home / 'wm.log').open('wb') as log:
                wm = subprocess.Popen(['openbox', '--sm-disable'], env=env, stdout=log, stderr=log)
            time.sleep(.3)
        try:
            with (home / 'gui.log').open('wb') as log:
                process = subprocess.Popen([home / 'bin/ferestre-gui'], env=env, stdout=log, stderr=log)
            app = s.find_app(process.pid, time.monotonic() + s.TIMEOUT)
            frame = s.wait_for(app, lambda r: next(iter(s.find_all(r, role='frame')), None),
                               'artwork window', time.monotonic() + s.TIMEOUT)
            s.select_in_list(frame, 'Game Pass')
            s.wait_for(frame, lambda _: len(covered()) >= 8, 'other covers while one image is stalled', time.monotonic() + 10)
            s.check(not (cache / 'images' / (ids[0] + '-cover-600.png')).exists(),
                    'a stalled image does not hold the rest of the page')
            release.set()
            # Stay on page one: later pages must fill without being visited.
            s.wait_for(frame, lambda _: len(covered()) == len(ids), 'the full catalogue to prefetch', time.monotonic() + 20)
            s.check(len(covered()) == 60, 'all three pages preload after opening Game Pass')
            with lock:
                s.check(peak <= 4, f'network concurrency remains bounded (peak {peak})')
                before = counts.copy()
            s.click_named(frame, 'Next page')
            s.wait_for(frame, lambda r: 'Artwork Game 020' in s.texts(r), 'page two', time.monotonic() + 10)
            time.sleep(.3)
            with lock:
                s.check(counts == before, 'pagination reuses the preloaded cover cache')
            if args.fullscreen:
                s.click_named(frame, 'Enter fullscreen')
                s.wait_for(frame, lambda r: s.find_all(r, role='button', name='Exit fullscreen'),
                           'fullscreen transition', time.monotonic() + 10)
                time.sleep(.3)
                bounds = frame.get_component_iface().get_extents(s.Atspi.CoordType.WINDOW)
                s.check((bounds.width, bounds.height) == (1280, 720), 'fullscreen fills the 720p handheld display')
                cards = [b.get_component_iface().get_extents(s.Atspi.CoordType.WINDOW)
                         for b in s.find_all(frame, role='button') if 'Artwork Game' in (b.get_name() or '') and ' · Install' in (b.get_name() or '')]
                print('handheld card allocations',[(b.x,b.y,b.width,b.height) for b in cards],flush=True)
                s.check(len({b.x for b in cards}) >= 4, 'handheld Game Pass has at least four cover columns')
                s.check(any(b.y > 0 and b.y + b.height <= 688 for b in cards), 'complete cover cards fit above controller hints at 720p')
                s.check(any(b.get_state_set().contains(s.Atspi.StateType.FOCUSED)
                            for b in s.find_all(frame, role='button') if ' · Install' in (b.get_name() or '')),
                        'fullscreen gives a game controller focus')
                preference = home / 'config/ferestre/window.json'
                s.check(json.loads(preference.read_text())['fullscreen'], 'fullscreen preference is saved')
                s.click_named(frame, 'Exit fullscreen')
                s.wait_for(frame, lambda r: s.find_all(r, role='button', name='Enter fullscreen'),
                           'touch exit from fullscreen', time.monotonic() + 10)
                s.check(not json.loads(preference.read_text())['fullscreen'], 'touch can return to desktop mode')
            s.click_named(frame, 'Close')
            s.check(process.wait(timeout=10) == 0, 'artwork fixture closes cleanly')
            if args.fullscreen:
                preference.write_text('{"fullscreen":true}')
                with (home / 'gui.log').open('ab') as log:
                    process = subprocess.Popen([home / 'bin/ferestre-gui'], env=env, stdout=log, stderr=log)
                app = s.find_app(process.pid, time.monotonic() + s.TIMEOUT)
                frame = s.wait_for(app, lambda r: next(iter(s.find_all(r, role='frame')), None),
                                   'restarted handheld', time.monotonic() + s.TIMEOUT)
                s.wait_for(frame, lambda r: s.find_all(r, role='button', name='Exit fullscreen'),
                           'remembered fullscreen mode', time.monotonic() + 10)
                s.check(True, 'a handheld can start directly in its remembered fullscreen mode')
                s.click_named(frame, 'Exit fullscreen')
                s.wait_for(frame, lambda r: s.find_all(r, role='button', name='Enter fullscreen'),
                           'desktop mode', time.monotonic() + 10)
                s.click_named(frame, 'Close')
                s.check(process.wait(timeout=10) == 0, 'restarted handheld closes cleanly')
            s.check('panicked at' not in (home / 'gui.log').read_text(), 'background artwork does not panic')
            print('all progressive artwork and prefetch checks passed', flush=True)
        except Exception:
            print("fixture image requests:", dict(counts), "cached covers:", len(covered()), flush=True)
            print((home / 'gui.log').read_text(), flush=True)
            s.dump(frame, 'artwork failure')
            raise
        finally:
            release.set()
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            if wm is not None:
                wm.terminate()
                wm.wait(timeout=5)
            server.shutdown()
            server.server_close()


if __name__ == '__main__':
    main()

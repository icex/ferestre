#!/usr/bin/env python3
"""Desktop decoration settings on the actual split-view and fullscreen UI."""
import argparse
from pathlib import Path
import subprocess
import tempfile
import time

import gui_smoke as s
from gui_launch import prepare
from controller_driver import start_window_manager


def visible(node):
    return node.get_state_set().contains(s.Atspi.StateType.SHOWING)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/ferestre-gui')
    parser.add_argument('--narrow', action='store_true')
    args = parser.parse_args()
    s.Atspi.init()
    layouts = [
        ('close,minimize,maximize:', ['Close', 'Minimize', 'Maximize'], []),
        (':minimize,maximize,close', [], ['Minimize', 'Maximize', 'Close']),
        ('close:minimize,maximize', ['Close'], ['Minimize', 'Maximize']),
        (':close', [], ['Close']),
        (':', [], []),
    ]
    wm = start_window_manager()
    try:
        for layout, left, right in layouts:
            with tempfile.TemporaryDirectory(prefix='ferestre-window-controls-') as directory:
                home = Path(directory)
                _, _, pad, env = prepare(home, args.binary)
                settings = home / 'config/gtk-4.0/settings.ini'
                settings.write_text('[Settings]\ngtk-decoration-layout=' + layout + '\n')
                process = app = None
                try:
                    with (home / 'gui.log').open('wb') as log:
                        process = subprocess.Popen([home / 'bin/ferestre-gui'], env=env, stdout=log, stderr=log)
                    app = s.find_app(process.pid, time.monotonic() + 20)
                    def wait(predicate, what):
                        return s.wait_for(app, predicate, what, time.monotonic() + 20)
                    frame = wait(lambda root: next(iter(s.find_all(root, role='frame')), None), 'window')
                    def controls():
                        return {name: [n for n in s.find_all(app, role='button', name=name) if visible(n)]
                                for name in ('Close', 'Minimize', 'Maximize')}
                    def ready():
                        found = controls()
                        return all(len(found[name]) == int(name in left + right) for name in found)
                    def positions():
                        wait(lambda _: ready(), 'exactly the configured decoration buttons')
                        width = frame.get_component_iface().get_extents(s.Atspi.CoordType.WINDOW).width
                        for names, edge in [(left, 'left'), (right, 'right')]:
                            boxes = [controls()[name][0].get_component_iface().get_extents(s.Atspi.CoordType.WINDOW) for name in names]
                            coordinates = [box.x for box in boxes]
                            s.check(coordinates == sorted(coordinates), f'{layout} preserves {edge} button order')
                            s.check(all(box.width > 0 and (box.x < 160 if edge == 'left' else box.x > width - 170)
                                        for box in boxes), f'{layout} places controls at the outside {edge} edge')
                        s.check(ready(), 'each configured control appears once')
                    positions()
                    navigation = s.find_all(frame, role='button', name='Show navigation')
                    s.check(any(visible(n) for n in navigation) == args.narrow,
                            'test exercises the requested wide/narrow split view', frame)
                    if args.narrow:
                        s.click_named(frame, 'Show navigation')
                        wait(lambda root: any(visible(n) for n in s.find_all(root, name='Settings')), 'sidebar page')
                        positions()
                        s.select_in_list(frame, 'My Library')
                        positions()
                    s.click_named(frame, 'Enter fullscreen')
                    wait(lambda root: s.find_all(root, role='button', name='Exit fullscreen'), 'fullscreen')
                    wait(lambda _: not any(controls().values()), 'all window controls hidden in fullscreen')
                    s.check(True, f'{layout} hides controls on both sides in fullscreen')
                    s.click_named(frame, 'Exit fullscreen')
                    wait(lambda root: s.find_all(root, role='button', name='Enter fullscreen'), 'return to desktop')
                    positions()
                    if args.narrow:
                        s.click_named(frame, 'Show navigation')
                    s.select_in_list(frame, 'Settings')
                    s.click_named(frame, 'Quit Ferestre')
                    s.check(process.wait(timeout=10) == 0, 'isolated window closes cleanly')
                    s.check('panicked at' not in (home / 'gui.log').read_text(), 'no decoration or fullscreen panic')
                except Exception:
                    print((home / 'gui.log').read_text(), flush=True)
                    if app is not None:
                        s.dump(app, 'window control failure')
                    raise
                finally:
                    pad()
                    if process is not None and process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
        print('PASS: automatic desktop button layouts, split views and fullscreen', flush=True)
    finally:
        if wm is not None:
            wm.terminate()
            wm.wait(timeout=5)


if __name__ == '__main__':
    main()

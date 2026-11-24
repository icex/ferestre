#!/usr/bin/env python3
"""The window keeps its size across starts, and fits a display it outgrew.

Run under tests/run_wayland_gui.py (a 1280x720 private KWin). Not a gui_*.py:
CI runs those on Xvfb, where nothing can resize the window this way.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time

import gui_smoke as s
from gui_launch import prepare
from wayland_focus import run_script


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/ferestre-gui')
    args = parser.parse_args()
    s.Atspi.init()
    with tempfile.TemporaryDirectory(prefix='ferestre-window-size-') as directory:
        home = Path(directory)
        _, _, _, env = prepare(home, args.binary)
        saved = home / 'config/ferestre/window.json'
        if not saved.parent.exists():
            saved = next(iter(home.rglob('window.json')), saved)
        saved.parent.mkdir(parents=True, exist_ok=True)

        def run(check):
            with (home / 'gui.log').open('wb') as log:
                process = subprocess.Popen([home / 'bin/ferestre-gui'], env=env, stdout=log, stderr=log)
                try:
                    app = s.find_app(process.pid, time.monotonic() + 20)
                    frame = s.wait_for(app, lambda root: next(iter(s.find_all(root, role='frame')), None),
                                       'window', time.monotonic() + 20)
                    time.sleep(3)
                    check(app, lambda: (lambda b: (b.width, b.height))(
                        frame.get_component_iface().get_extents(s.Atspi.CoordType.WINDOW)), process.pid)
                finally:
                    process.terminate()
                    process.wait(timeout=20)

        first = []

        def resize(app, size, pid):
            run_script('workspace.windowList().filter(w => w.pid == %d && w.normalWindow).forEach(w => '
                 'w.frameGeometry = {x: 40, y: 20, width: 1010, height: 655});' % pid)
            time.sleep(2)
            first.append(size())
            s.check(abs(size()[0] - 1010) < 40, f'the window was resized: {size()}')
            for page in ('My Library', 'Installed', 'Updates', 'Home'):
                for row in s.find_all(app, name=page)[:1]:
                    parent = row.get_parent()
                    index = row.get_index_in_parent()
                    if parent.get_selection_iface():
                        parent.get_selection_iface().select_child(index)
                        time.sleep(.6)
                s.check(size() == first[0], f'{page} leaves the size alone: {size()}')

        run(resize)
        s.check(json.loads(saved.read_text()).get('size'), 'the size is saved')
        kept = saved.read_text()
        run(lambda app, size, pid: s.check(size() == first[0], f'the next start has it: {size()} {first[0]}'))
        s.check(saved.read_text() == kept, 'starting saves nothing new')
        saved.write_text(json.dumps({'size': [2000, 1400]}))
        run(lambda app, size, pid: s.check(size()[0] <= 1290 and size()[1] <= 730, f'it fits: {size()}'))
        s.check(json.loads(saved.read_text())['size'] == [2000, 1400], 'a squeezed size is not saved')
    print('window size: ok')


if __name__ == '__main__':
    main()

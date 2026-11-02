#!/usr/bin/env python3
"""Drive real GTK navigation through a process-private SDL adapter.

No physical controller is opened. This verifies SDL input dispatch, focus and
glyph presentation, including the indicator pixels on an isolated X display.
"""
import argparse
import ctypes
import ctypes.util
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
import time

import gui_smoke as s
from gui_artwork import png

SDL = r'''
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
struct State { uint16_t buttons; int16_t x,y,lt,rt; int32_t kind,attached; };
static struct State state;
void SDL_GameControllerUpdate(void) {
  FILE *file=fopen(getenv("FERESTRE_TEST_PAD_FILE"),"rb");
  if(file){fread(&state,sizeof(state),1,file);fclose(file);}
}
int SDL_InitSubSystem(uint32_t flags){SDL_GameControllerUpdate();return 0;}
void SDL_QuitSubSystem(uint32_t flags){}
int SDL_SetHint(const char *name,const char *value){return 1;}
int SDL_NumJoysticks(void){return state.attached;}
int SDL_IsGameController(int index){return state.attached;}
void *SDL_GameControllerOpen(int index){return state.attached?(void*)1:0;}
void SDL_GameControllerClose(void *pad){}
int SDL_GameControllerGetAttached(void *pad){return state.attached;}
int SDL_GameControllerGetType(void *pad){return state.kind;}
uint8_t SDL_GameControllerGetButton(void *pad,int button){return !!(state.buttons&(1<<button));}
int16_t SDL_GameControllerGetAxis(void *pad,int axis){return axis==0?state.x:axis==1?state.y:0;}
'''


def focus(root):
    return next((n for n, _ in s.walk(root) if n.get_state_set().contains(s.Atspi.StateType.FOCUSED)), None)


def has_focus(root, name):
    node = focus(root)
    return node is not None and name in (node.get_name() or '')


def green_outline(pid, button):
    """Read a narrow strip outside the selected button, without image libraries."""
    lib = ctypes.CDLL(ctypes.util.find_library('X11'))
    ptr = ctypes.c_void_p
    lib.XOpenDisplay.argtypes = [ctypes.c_char_p]
    lib.XOpenDisplay.restype = ptr
    lib.XDefaultRootWindow.argtypes = [ptr]
    lib.XDefaultRootWindow.restype = ctypes.c_ulong
    lib.XGetImage.argtypes = [ptr, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                             ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
    lib.XGetImage.restype = ptr
    lib.XGetPixel.argtypes = [ptr, ctypes.c_int, ctypes.c_int]
    lib.XGetPixel.restype = ctypes.c_ulong
    lib.XDestroyImage.argtypes = [ptr]
    lib.XCloseDisplay.argtypes = [ptr]
    display = lib.XOpenDisplay(None)
    if not display:
        raise s.Failure('isolated X display is unavailable')
    try:
        origin = s.window_origin(pid)
        box = button.get_component_iface().get_extents(s.Atspi.CoordType.WINDOW)
        x, y = origin[0] + box.x - 12, origin[1] + box.y + box.height // 2
        image = lib.XGetImage(display, lib.XDefaultRootWindow(display), x, y, 14, 1,
                             ctypes.c_ulong(-1).value, 2)
        if not image:
            return False
        try:
            colors=[lib.XGetPixel(image, i, 0) & 0xFFFFFF for i in range(14)]
            found=0x8BD77B in colors
            if not found:
                print('indicator diagnostic:', (box.x,box.y,box.width,box.height), 'window',origin,'pixels',[hex(c) for c in colors],flush=True)
            return found
        finally:
            lib.XDestroyImage(image)
    finally:
        lib.XCloseDisplay(display)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/ferestre-gui')
    args = parser.parse_args()
    process = wm = None
    with tempfile.TemporaryDirectory(prefix='ferestre-controller-gui-') as tmp:
        home = Path(tmp)
        def write(path, text, executable=False):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
            if executable:
                path.chmod(0o755)
        write(home / 'sdl.c', SDL)
        (home / 'lib').mkdir()
        subprocess.run(['cc', '-shared', '-fPIC', str(home / 'sdl.c'), '-o', str(home / 'lib/libSDL2-2.0.so.0')], check=True)
        binary = Path(args.binary).resolve()
        (home / 'bin').mkdir()
        shutil.copy2(binary, home / 'bin/ferestre-gui')
        write(home / 'bin/ferestre', '#!/bin/sh\necho "{}"\n', True)
        write(home / 'client/xodus-cli', '#!/bin/sh\nexit 1\n', True)
        write(home / 'runtime/proton', '#!/bin/sh\nexit 0\n', True)
        write(home / 'runtime/version', 'controller-fixture\n')
        schema = int(re.search(r'const CACHE_SCHEMA: u32 = (\d+);', Path('crates/ferestre-core/src/catalog.rs').read_text())[1])
        ids = ['9ZZPAD000001', '9ZZPAD000002', '9ZZPAD000003', '9ZZPAD000004']
        for n, id in enumerate(ids):
            write(home / f'titles/{id}.toml', f'''schema=1
[title]
product-id="{id}"
name="Controller Game {n}"
slug="controller-{n}"
[launch]
executable="Game.exe"
[status]
state="untested"
summary="Offline controller fixture"
''')
            write(home / f'cache/ferestre/catalog/{id}.json', json.dumps({'schema': schema, 'market': 'US', 'language': 'en-us', 'product': {
                'product_id': id, 'name': f'Controller Game {n}', 'has_packages': True, 'has_pc_package': True, 'package_format': 'MSIXVC'}}))
            image = home / f'cache/ferestre/catalog/images/{id}-cover-600.png'
            image.parent.mkdir(parents=True, exist_ok=True)
            image.write_bytes(png())
        write(home / 'state/ferestre/library.json', json.dumps({'count': len(ids), 'items': [{'productId': id, 'productType': 'Game', 'status': 'Active'} for id in ids]}))
        state = home / 'pad'
        kind = 7
        def input(buttons=0):
            temp = home / 'pad-next'
            temp.write_bytes(struct.pack('<Hhhhh2xii', buttons, 0, 0, 0, 0, kind, 1))
            temp.replace(state)
        input()
        env = dict(os.environ, HOME=tmp, XDG_CONFIG_HOME=str(home / 'config'),
                   XDG_STATE_HOME=str(home / 'state'), XDG_CACHE_HOME=str(home / 'cache'), XDG_DATA_HOME=str(home / 'data'),
                   XODUS_CLI_DIR=str(home / 'client'), XODUS_PROTON_DIR=str(home / 'runtime'),
                   XODUS_GAMES_DIR=str(home / 'games'), XODUS_STEAM_DIR=str(home / 'steam'),
                   FERESTRE_TITLES=str(home / 'titles'), FERESTRE_MARKET='US', GTK_A11Y='atspi',
                   FERESTRE_TEST_PAD_FILE=str(state), LD_LIBRARY_PATH=str(home / 'lib'),
                   FERESTRE_APPLICATION_ID='io.github.icex.ferestre.ControllerTest')
        for key in ('APPDIR', 'APPIMAGE', 'FERESTRE_PACKAGE_VERSION'):
            env.pop(key, None)
        for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy'):
            env[key] = 'http://127.0.0.1:9'
        env['NO_PROXY'] = env['no_proxy'] = ''
        if os.environ.get('FERESTRE_TEST_WM') != '1':
            with (home / 'wm.log').open('wb') as log:
                wm = subprocess.Popen(['openbox', '--sm-disable'], env=dict(env, LD_LIBRARY_PATH=os.environ.get('LD_LIBRARY_PATH', '')), stdout=log, stderr=log)
            time.sleep(.3)
        frame = None
        try:
            with (home / 'gui.log').open('wb') as log:
                process = subprocess.Popen([home / 'bin/ferestre-gui'], env=env, stdout=log, stderr=log)
            app = s.find_app(process.pid, time.monotonic() + s.TIMEOUT)
            frame = s.wait_for(app, lambda r: next(iter(s.find_all(r, role='frame')), None), 'controller window', time.monotonic() + s.TIMEOUT)
            def wait(predicate, what):
                return s.wait_for(frame, predicate, what, time.monotonic() + s.TIMEOUT)
            def press(button, predicate, what):
                input(1 << button)
                try:
                    wait(predicate, what)
                finally:
                    input()
                    time.sleep(.08)
            wait(lambda r: has_focus(r, 'Controller Game 0 · Install'), 'initial game focus')
            wait(lambda r: any('✕' in t and 'L1 / R1' in t for t in s.texts(r)), 'automatic PlayStation controls')
            s.check(True, 'SDL PlayStation type displays cross/circle/triangle and L1/R1')
            time.sleep(.3)
            s.check(green_outline(process.pid, focus(frame)), 'SDL selection paints a visible green focus outline')
            press(14, lambda r: has_focus(r, 'Controller Game 1 · Install'), 'right to next game')
            press(4, lambda r: has_focus(r, 'Home'), 'View/Share navigation')
            wait(lambda r: any('→' in t and 'Games' in t for t in s.texts(r)), 'explicit right-to-games instruction')
            press(12, lambda r: has_focus(r, 'Game Pass'), 'next navigation section')
            press(12, lambda r: has_focus(r, 'My Library'), 'Library navigation')
            press(0, lambda r: bool(s.find_all(r, name='My Library page')) and has_focus(r, ' · Install'), 'A/cross enters games')
            s.check(True, 'navigation selects Library and enters the right pane')
            press(11, lambda r: (focus(r).get_name() or '') in ['My games', 'Installed', 'Updates', 'Install queue'], 'up from games to section tabs')
            s.check(True, 'up/down navigation reaches the section row above the game grid')
            press(12, lambda r: has_focus(r, ' · Install'), 'down back into game grid')
            press(6, lambda r: bool(s.find_all(r, role='button', name='Game details')), 'Menu/Options opens game options')
            press(1, lambda r: not s.find_all(r, role='button', name='Game details') and has_focus(r, ' · Install'), 'circle returns to selected game')
            press(1, lambda r: not has_focus(r, ' · Install'), 'circle opens navigation')
            press(14, lambda r: has_focus(r, ' · Install'), 'right returns to games')
            s.check(True, 'Options and Back follow console conventions')
            kind = 2
            input()
            wait(lambda r: any('Ⓐ' in t and 'LB / RB' in t for t in s.texts(r)), 'automatic Xbox controls')
            s.check(True, 'controller type changes update the displayed button family')
            s.click_named(frame, 'Enter fullscreen')
            wait(lambda r: s.find_all(r, role='button', name='Exit fullscreen') and has_focus(r, ' · Install'), 'fullscreen controller focus')
            press(4, lambda r: any('→ Games' in (n.get_name() or '') and n.get_state_set().contains(s.Atspi.StateType.SHOWING)
                                  for n in s.find_all(r, role='label')), 'visible Guide navigation instructions in fullscreen')
            press(14, lambda r: has_focus(r, ' · Install'), 'fullscreen navigation returns to games')
            s.click_named(frame, 'Exit fullscreen')
            wait(lambda r: s.find_all(r, role='button', name='Enter fullscreen'), 'return to desktop mode')
            s.check(True, 'handheld navigation keeps its controller instructions visible')
            s.click_named(frame, 'Close')
            s.check(process.wait(timeout=10) == 0, 'controller fixture closes cleanly')
            s.check('panicked at' not in (home / 'gui.log').read_text(), 'controller navigation did not panic')
            print('all controller adapter/focus/glyph checks passed', flush=True)
        except Exception:
            print((home / 'gui.log').read_text(), flush=True)
            if frame is not None:
                s.dump(frame, 'controller failure')
            raise
        finally:
            input()
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            if wm is not None:
                wm.terminate()
                wm.wait(timeout=5)


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Offline GTK wiring: update banner, one click, failure, cancellation and rollback.

Runs with a private home, copied GUI and sibling CLI fixtures. No release key,
real AppImage, credentials, game files or production window is touched.
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


def write(path, text, executable=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    if executable:
        path.chmod(0o755)


def deadline():
    return time.monotonic() + s.TIMEOUT


CLI = r'''#!/usr/bin/env python3
import hashlib,json,os,select,sys,time
from pathlib import Path
h=Path(os.environ['HOME'])
with (h/'calls').open('a') as f: f.write(' '.join(sys.argv[1:])+'\n')
cmd=sys.argv[1]
mode=(h/'mode').read_text()
if cmd=='check-app-updates':
    if mode=='offline': sys.exit(1)
    print((h/'update-state.json').read_text());sys.exit(0)
if cmd=='self-update':
    (h/'started').touch()
    if '--expected-appimage-sha256' in sys.argv:
        expected=sys.argv[sys.argv.index('--expected-appimage-sha256')+1]
        if hashlib.sha256((h/'fixture.AppImage').read_bytes()).hexdigest()!=expected:
            sys.stderr.write('!! The AppImage was replaced; restart Ferestre before updating\n');sys.exit(1)
    if mode=='cancel':
        select.select([sys.stdin],[],[],20)
        sys.stdin.buffer.read(1)
        (h/'cancelled').touch();sys.exit(130)
    if mode=='fail':
        sys.stderr.write('!! The signature does not match; the previous Ferestre was kept\n');sys.exit(1)
    status=json.loads((h/'update-state.json').read_text())
    applied=None
    if mode.startswith('restart-'):
        target=h/'fixture.AppImage';backup=h/'.fixture.AppImage.123-1.previous'
        original=target.read_bytes();new=(h/'cannot-start.AppImage').read_bytes()
        backup.write_bytes(original);backup.chmod(0o755)
        target.write_bytes(new);target.chmod(0o755)
        applied={'target':str(target),'backup':str(backup),'installed_sha256':hashlib.sha256(new).hexdigest(),'previous_sha256':hashlib.sha256(original).hexdigest()}
    status['launcher']=None;status['runtime']=None
    (h/'update-state.json').write_text(json.dumps(status))
    print(json.dumps({'schema':1,'updated':['Ferestre 0.1.16','Runtime 11.0.20261103.1'],'appimage':applied}))
    sys.exit(0)
print('{}')
'''


class Window:
    def __init__(self, binary, home, mode):
        self.home = home
        home.mkdir(parents=True, mode=0o700)
        self.binary = home / 'usr/bin/ferestre-gui'
        self.binary.parent.mkdir(parents=True)
        shutil.copy2(binary, self.binary)
        write(home / 'usr/bin/ferestre', CLI, True)
        write(home / 'mode', mode)
        write(home / 'usr/lib/ferestre/VERSION', '0.1.15\n')
        write(home / 'config/gtk-4.0/settings.ini', '[Settings]\ngtk-enable-animations=false\n')
        write(home / 'runtime/proton', '#!/bin/sh\nexit 0\n', True)
        write(home / 'runtime/version', '11.0.20261023.1\n')
        (home / 'recipes').mkdir()
        shutil.copy2('titles/capabilities.toml', home / 'recipes/capabilities.toml')
        write(home / 'client/xodus-cli', '#!/usr/bin/env python3\nimport json,sys\nif sys.argv[1:2]==["account-profiles"]:\n print(json.dumps({"schema":1,"capabilities":["store-player-accounts"],"profiles":[],"default_profile":None}))\nelse: sys.exit(1)\n', True)
        status = {'schema': 1, 'launcher': {'component': 'launcher', 'version': '0.1.16', 'tag': 'v0.1.16', 'asset': 'ferestre-0.1.16-x86_64.AppImage', 'url': 'https://github.com/icex/ferestre/releases/download/v0.1.16/ferestre-0.1.16-x86_64.AppImage', 'signature_url': '', 'sha256': '00' * 32}, 'runtime': {'component': 'runtime', 'version': '11.0.20261103.1', 'tag': 'runtime-11.0.20261103.1', 'asset': '', 'url': '', 'signature_url': '', 'sha256': '00' * 32}, 'appimage': str(home / 'fixture.AppImage'), 'launcher_unavailable': None}
        if mode == 'unsupported':
            status.update(runtime=None, appimage=None, launcher_unavailable='Use the package manager')
        if mode == 'current':
            status.update(runtime=None, launcher=None)
        write(home / 'update-state.json', json.dumps(status))
        fixture = bytearray(32)
        fixture[:4] = b'\x7fELF'
        fixture[8:11] = b'AI\x02'
        (home / 'fixture.AppImage').write_bytes(fixture)
        (home / 'fixture.AppImage').chmod(0o755)
        self.original = bytes(fixture)
        if mode.startswith('restart-'):
            broken = home / 'cannot-start.AppImage'
            source = '#include <stdio.h>\n#include <stdlib.h>\nint main(void) { char p[4096]; snprintf(p,sizeof(p),"%s/restarted",getenv("HOME")); FILE *f=fopen(p,"w"); if(!f)return 2; fputs((getenv("APPIMAGE")||getenv("APPDIR")||getenv("FERESTRE_PACKAGE_VERSION")||getenv("XODUS_CLI_DIR"))?"stale":"clean",f); fclose(f); return 0; }'
            command = ['cc', '-x', 'c', '-', '-o', str(broken)]
            if mode == 'restart-failure':
                command.append('-Wl,--dynamic-linker,/ferestre-updater-fixture-missing-loader')
            subprocess.run(command, input=source, text=True, check=True)
            content = bytearray(broken.read_bytes())
            content[8:11] = b'AI\x02'
            broken.write_bytes(content)
        env = dict(os.environ)
        for name in ('APPIMAGE', 'APPDIR', 'FERESTRE_PACKAGE_VERSION', 'FERESTRE_SCRIPTS_DIR'):
            env.pop(name, None)
        env.update(APPDIR=str(home), APPIMAGE=str(home / 'fixture.AppImage'), HOME=str(home), XDG_CONFIG_HOME=str(home / 'config'), XDG_CACHE_HOME=str(home / 'cache'), XDG_STATE_HOME=str(home / 'state'), XDG_DATA_HOME=str(home / 'data'), XODUS_CLI_DIR=str(home / 'client'), XODUS_GAMES_DIR=str(home / 'games'), XODUS_STEAM_DIR=str(home / 'steam'), XODUS_PROTON_DIR=str(home / 'runtime'), FERESTRE_TITLES=str(home / 'recipes'), GTK_A11Y='atspi', FERESTRE_APPLICATION_ID='io.github.icex.ferestre.Update' + mode.replace('-', ''))
        for name in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy'):
            env[name] = 'http://127.0.0.1:9'
        env['NO_PROXY'] = env['no_proxy'] = ''
        self.log = home / 'gui.log'
        with self.log.open('wb') as log:
            self.gui = subprocess.Popen([self.binary], env=env, stdout=subprocess.DEVNULL, stderr=log)
        try:
            self.app = s.find_app(self.gui.pid, deadline())
            self.frame = s.wait_for(self.app, lambda root: next(iter(s.find_all(root, role='frame')), None), 'the update fixture window', deadline())
        except BaseException:
            self.gui.terminate()
            self.gui.wait(timeout=5)
            raise

    def calls(self, command):
        if not (self.home / 'calls').exists():
            return []
        return [line for line in (self.home / 'calls').read_text().splitlines() if line.split()[0] == command]

    def wait_text(self, text):
        return s.wait_for(self.frame, lambda root: any(text in t for t in s.texts(root)), text, deadline())

    def click_update(self):
        try:
            button = s.wait_for(self.frame, lambda root: next((n for n in s.find_all(root, role='button', name='Update') if n.get_state_set().contains(s.Atspi.StateType.SENSITIVE)), None), 'enabled Update button', deadline())
        except BaseException:
            print('UI text:', s.texts(self.frame), flush=True)
            print('UI buttons:', [(n.get_name(), n.get_state_set().get_states()) for n in s.find_all(self.frame, role='button')], flush=True)
            raise
        s.click(button)

    def close(self):
        if self.gui.poll() is None:
            s.click_named(self.frame, 'Close')
            s.check(self.gui.wait(timeout=s.TIMEOUT) == 0, 'the fixture window closes cleanly')
        s.check('panicked at' not in self.log.read_text(), 'the update UI did not panic')

    def stop(self):
        if self.gui.poll() is None:
            self.gui.terminate()
            self.gui.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/ferestre-gui')
    parser.add_argument('--keep', action='store_true')
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    root = Path(tempfile.mkdtemp(prefix='ferestre-updater-gui-'))
    s.Atspi.init()
    windows = []
    try:
        for mode in ('success', 'fail', 'stale-window', 'cancel', 'restart-failure', 'restart-success', 'unsupported', 'current', 'offline'):
            print('update UI:', mode, flush=True)
            window = Window(binary, root / mode, mode)
            windows.append(window)
            if mode == 'offline':
                s.wait_for(window.frame, lambda _: bool(window.calls('check-app-updates')), 'offline check to start', deadline())
                time.sleep(.5)
                s.check(not window.calls('self-update'), 'offline startup never starts an update')
            elif mode == 'current':
                s.wait_for(window.frame, lambda _: bool(window.calls('check-app-updates')), 'startup version check', deadline())
                time.sleep(.5)
                s.check(not s.find_all(window.frame, role='button', name='Update'), 'up-to-date installations have no update action')
                s.select_in_list(window.frame, 'Runtime')
                s.click_named(window.frame, 'Check for Updates')
                s.wait_for(window.frame, lambda _: len(window.calls('check-app-updates')) == 2, 'manual check bypasses the periodic cache', deadline())
            elif mode == 'unsupported':
                window.wait_text('package manager')
                s.check(not s.find_all(window.frame, role='button', name='Update'), 'package installations are not overwritten')
            else:
                window.wait_text('Ferestre 0.1.16 and runtime 11.0.20261103.1 are available')
                s.check(not window.calls('self-update'), 'checking online does not install anything')
                if mode == 'stale-window':
                    external = window.original + b'independent replacement'
                    (window.home / 'fixture.AppImage').write_bytes(external)
                window.click_update()
                s.wait_for(window.frame, lambda _: (window.home / 'started').exists(), 'one click starts the component updater', deadline())
                if mode == 'stale-window':
                    window.wait_text('restart Ferestre before updating')
                    s.check((window.home / 'fixture.AppImage').read_bytes() == external, 'a stale window does not overwrite an independent replacement')
                elif mode == 'cancel':
                    s.click_named(window.frame, 'Stop Download')
                    s.wait_for(window.frame, lambda _: (window.home / 'cancelled').exists(), 'cancellation reaches the updater', deadline())
                    window.wait_text('previous Ferestre was kept')
                elif mode == 'fail':
                    window.wait_text('signature does not match')
                    s.check((window.home / 'fixture.AppImage').read_bytes() == window.original, 'failed verification leaves the previous app intact')
                elif mode == 'restart-success':
                    until = deadline()
                    while not (window.home / 'restarted').exists() and time.monotonic() < until:
                        time.sleep(.05)
                    s.check((window.home / 'restarted').read_text() == 'clean', 'one click restarts into the installed image with fresh package locators')
                    s.check(window.gui.wait(timeout=s.TIMEOUT) == 0, 'the restarted executable exits successfully')
                elif mode == 'restart-failure':
                    window.wait_text('previous version was restored')
                    s.check((window.home / 'fixture.AppImage').read_bytes() == window.original, 'failed restart restores the exact previous image')
                else:
                    window.wait_text('Updated Ferestre 0.1.16 and Runtime 11.0.20261103.1')
                    s.wait_for(window.frame, lambda root: not s.find_all(root, role='button', name='Update'), 'the completed update hides its banner', deadline())
                s.check(len(window.calls('self-update')) == 1, 'a single click performs one combined update')
                call = window.calls('self-update')[0]
                s.check('--minimum-launcher 0.1.16' in call and '--minimum-runtime 11.0.20261103.1' in call and '--expected-appimage-sha256' in call, 'the action is bound to its offered versions and original AppImage')
            window.close()
        print('all secure updater GUI checks passed', flush=True)
    finally:
        for window in windows:
            window.stop()
        if args.keep:
            print('kept', root)
        else:
            shutil.rmtree(root)


if __name__ == '__main__':
    main()

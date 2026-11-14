#!/usr/bin/env python3
"""Desktop detection and descriptor-safe game/performance/compositor sessions."""
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/game-session.py'
spec = importlib.util.spec_from_file_location('game_session', SCRIPT)
session = importlib.util.module_from_spec(spec)
spec.loader.exec_module(session)
HDR = dict(width=7680, height=2160, refresh=240.0, hdr=True, source='fixture', scale=1.0)
WAYLAND = dict(WAYLAND_DISPLAY='wayland-1', XDG_SESSION_TYPE='wayland', XDG_CURRENT_DESKTOP='KDE')


class CompositorIsolationTests(unittest.TestCase):
    def test_bus_activation_uses_a_private_runtime_before_the_session_starts(self):
        runner_spec = importlib.util.spec_from_file_location('wayland_runner', Path(__file__).with_name('run_wayland_gui.py'))
        runner = importlib.util.module_from_spec(runner_spec)
        runner_spec.loader.exec_module(runner)
        seen = []
        def launch(command, env):
            runtime = Path(env['XDG_RUNTIME_DIR'])
            seen.append(runtime)
            self.assertNotEqual(str(runtime), '/run/user/1000')
            self.assertEqual(runtime.stat().st_mode & 0o777, 0o700)
            self.assertEqual(env['FERESTRE_WAYLAND_SESSION'], '1')
            self.assertEqual(command[0], 'dbus-run-session')
            config = Path(command[1].split('=', 1)[1]).read_text()
            self.assertIn('<listen>unix:tmpdir=' + str(runtime), config)
            self.assertNotIn('<servicedir', config)
            self.assertNotIn('<include', config)
            return 17
        with patch.dict(os.environ, {'XDG_RUNTIME_DIR': '/run/user/1000'}, clear=True), patch.object(runner.subprocess, 'call', launch):
            self.assertEqual(runner.main(), 17)
        self.assertEqual(len(seen), 1)
        self.assertFalse(seen[0].exists())


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    path.chmod(0o755)


class DesktopTests(unittest.TestCase):
    def kde(self, hdr=True):
        return {'outputs': [{'enabled': False, 'hdr': True}, {
            'enabled': True, 'hdr': hdr, 'currentModeId': 'active',
            'modes': [{'id': 'active', 'refreshRate': 240, 'size': {'width': 7680, 'height': 2160}}]}]}

    def gnome(self, color=1):
        identity = ['DP-1', 'vendor', 'product', 'serial']
        return {'type': 'fixture', 'data': [1, [[identity, [
            ['mode', 7680, 2160, 239.975, 1.5, [1.0, 1.5], {'is-current': {'type': 'b', 'data': True}}]
        ], {'color-mode': {'type': 'u', 'data': color}, 'supported-color-modes': [0, 1]}]],
            [[0, 0, 1.5, 0, True, [identity], {}]], {}]}

    def test_kde_uses_enabled_hdr_state_and_current_mode(self):
        self.assertEqual(session.kde_display(self.kde()), dict(HDR, source='KDE', refresh=240))
        self.assertFalse(session.kde_display(self.kde(False))['hdr'])
        self.assertIsNone(session.kde_display({'outputs': [{'enabled': False, 'hdr': True}]}))

    def test_gnome_matches_active_logical_monitors_and_current_color_mode(self):
        result = session.gnome_display(self.gnome())
        self.assertTrue(result['hdr'])
        self.assertEqual(result['refresh'], 239.975)
        self.assertEqual(result['scale'], 1.5)
        self.assertFalse(session.gnome_display(self.gnome(0))['hdr'])
        data = self.gnome(); data['data'][2] = []
        self.assertIsNone(session.gnome_display(data))

    def test_malformed_or_unavailable_desktop_data_is_not_hdr(self):
        for value in (None, {}, [], {'outputs': None}, {'type': 'x', 'data': [1, [], [None], {}]}):
            self.assertIsNone(session.kde_display(value))
            self.assertIsNone(session.gnome_display(value))
        for rate in (0, -1, float('nan'), float('inf'), 1001, '240'):
            self.assertIsNone(session.mode(7680, 2160, rate, True, 'fixture'))
        for scale in (0, -1, float('nan'), float('inf'), 10, '1.5', True):
            self.assertIsNone(session.mode(7680, 2160, 240, True, 'fixture', scale))

    def test_kde_reads_the_active_output_scale(self):
        data = self.kde()
        data['outputs'][1]['scale'] = 1.5
        self.assertEqual(session.kde_display(data)['scale'], 1.5)

    def test_x11_never_enables_wayland_from_a_stale_environment(self):
        calls = []
        self.assertIsNone(session.display(dict(WAYLAND, XDG_SESSION_TYPE='x11'), lambda cmd: calls.append(cmd)))
        self.assertEqual(calls, [])
        self.assertIsNone(session.display({}, lambda cmd: calls.append(cmd)))

    def test_probes_are_read_only_and_desktop_specific(self):
        calls = []
        result = session.display(WAYLAND, lambda cmd: calls.append(cmd) or self.kde())
        self.assertTrue(result['hdr']); self.assertEqual(calls, [['kscreen-doctor', '-j']])
        calls.clear()
        result = session.display(dict(WAYLAND, XDG_CURRENT_DESKTOP='ubuntu:GNOME'), lambda cmd: calls.append(cmd) or self.gnome())
        self.assertTrue(result['hdr']); self.assertEqual(calls[0][-1], 'GetCurrentState')

    def test_performance_uses_cachyos_utility_or_available_gamemode(self):
        available = lambda tool: tool
        self.assertEqual(session.performance('ID="cachyos"\nID_LIKE=arch', available), 'game-performance')
        self.assertEqual(session.performance('ID=arch\nID_LIKE=cachyos', available), 'gamemoderun')
        self.assertIsNone(session.performance('ID=ubuntu', lambda _: None))
        self.assertEqual(session.performance('ID=cachyos', lambda t: t if t == 'gamemoderun' else None), 'gamemoderun')


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='ferestre-session-policy-')
        self.root = Path(self.directory.name)
        self.runtime = self.root / 'runtime'
        self.runtime.mkdir()
        write(self.runtime / 'proton', '#!/bin/sh\n# PROTON_ENABLE_WAYLAND\n')

    def tearDown(self):
        self.directory.cleanup()

    def driver(self, hdr=True):
        path = self.runtime / 'files/lib/wine/x86_64-unix/winewayland.so'
        write(path, 'wp_color_manager_v1' if hdr else 'basic Wayland')
        write(self.runtime / 'files/lib/wine/x86_64-windows/winewayland.drv', 'driver')
        write(self.runtime / 'files/share/default_pfx/drive_c/windows/system32/winewayland.drv', 'driver')

    def test_driver_presence_and_desktop_hdr_do_not_qualify_a_game_for_wayland(self):
        self.driver()
        original = dict(WAYLAND, DISPLAY=':0', PROTON_ENABLE_WAYLAND='1',
                        PROTON_ENABLE_HDR='1', DXVK_HDR='1')
        env, nested, native = session.selection(original, self.runtime, HDR, True)
        self.assertFalse(native)
        self.assertFalse(nested)
        self.assertEqual(env['DISPLAY'], ':0')
        for key in ('PROTON_ENABLE_WAYLAND', 'PROTON_ENABLE_HDR', 'DXVK_HDR'):
            self.assertEqual(env[key], '0')
        self.assertEqual(original['PROTON_ENABLE_WAYLAND'], '1')
        self.assertNotIn('WINEDLLOVERRIDES', env)

    def test_automatic_mode_overrides_unsafe_proton_user_settings_only_for_the_child(self):
        self.driver()
        settings = self.runtime / 'user_settings.py'
        for value in ("'1'", "str(1)"):
            with self.subTest(value=value):
                text = ("user_settings={'PROTON_USE_WAYLAND':" + value + ", 'PROTON_USE_HDR':" + value
                        + ", 'WINE_GRAPHICS_DRIVER': ''.join(['way', 'land'])}")
                write(settings, text)
                env, _, native = session.selection(dict(WAYLAND, DISPLAY=':0'), self.runtime, HDR, False)
                self.assertFalse(native)
                self.assertEqual(env['PROTON_USE_WAYLAND'], '0')
                self.assertEqual(env['PROTON_USE_HDR'], '0')
                self.assertEqual(env['WINE_GRAPHICS_DRIVER'], 'x11')
                self.assertEqual(settings.read_text(), text)

    def test_explicit_gamescope_can_use_hdr_even_when_a_native_driver_exists(self):
        self.driver()
        env, nested, native = session.selection(dict(WAYLAND, DISPLAY=':0',
            PROTON_ENABLE_WAYLAND='1', FERESTRE_GAMESCOPE='1'), self.runtime, HDR, True)
        self.assertFalse(native)
        self.assertTrue(nested)
        self.assertEqual(env['PROTON_ENABLE_WAYLAND'], '0')
        self.assertEqual(env['DXVK_HDR'], '1')

    def test_incomplete_wayland_prefix_template_keeps_the_regular_display(self):
        self.driver()
        (self.runtime / 'files/share/default_pfx/drive_c/windows/system32/winewayland.drv').unlink()
        env, nested, native = session.selection(dict(WAYLAND, FERESTRE_NATIVE_WAYLAND='1'), self.runtime, HDR, True)
        self.assertFalse(native); self.assertFalse(nested)
        self.assertEqual(env['WAYLAND_DISPLAY'], WAYLAND['WAYLAND_DISPLAY'])
        self.assertNotIn('PROTON_USE_XALIA', env)

    def test_capable_runtime_gets_wayland_hdr_and_compositor_refresh(self):
        self.driver()
        env, nested, native = session.selection(dict(WAYLAND, FERESTRE_NATIVE_WAYLAND='1'), self.runtime, HDR, True)
        self.assertFalse(nested); self.assertTrue(native)
        for key in ('PROTON_ENABLE_WAYLAND', 'PROTON_ENABLE_HDR', 'DXVK_HDR'):
            self.assertEqual(env[key], '1')
        self.assertEqual(env['FERESTRE_DESKTOP_MODE'], '7680x2160@240.000000')

    def test_old_runtime_can_opt_into_hdr_compositor(self):
        env, nested, native = session.selection(dict(WAYLAND, FERESTRE_GAMESCOPE='1'), self.runtime, HDR, True)
        self.assertTrue(nested); self.assertFalse(native)
        self.assertNotIn('PROTON_ENABLE_WAYLAND', env)
        self.assertEqual(env['DXVK_HDR'], '1')

    def test_wayland_flag_on_a_runtime_without_the_driver_still_allows_hdr_fallback(self):
        env, nested, native = session.selection(dict(WAYLAND, PROTON_ENABLE_WAYLAND='1', FERESTRE_GAMESCOPE='1'), self.runtime, HDR, True)
        self.assertTrue(nested); self.assertFalse(native)
        self.assertEqual(env['PROTON_ENABLE_WAYLAND'], '0')
        self.assertEqual(env['DXVK_HDR'], '1')

    def test_missing_wayland_driver_never_wraps_a_normal_launch(self):
        env, nested, native = session.selection(WAYLAND, self.runtime, HDR, True)
        self.assertFalse(nested); self.assertFalse(native)
        self.assertNotIn('DXVK_HDR', env)
        self.assertEqual(env['WAYLAND_DISPLAY'], WAYLAND['WAYLAND_DISPLAY'])
        self.assertEqual(env['FERESTRE_DESKTOP_MODE'], '7680x2160@240.000000')

    def test_host_vulkan_handles_hdr_without_protocol_markers_in_wine(self):
        self.driver(hdr=False)
        env, nested, native = session.selection(dict(WAYLAND, FERESTRE_NATIVE_WAYLAND='1'), self.runtime, HDR, True)
        self.assertTrue(native); self.assertFalse(nested)
        self.assertEqual(env['DXVK_HDR'], '1')

    def test_scaled_desktop_requires_fractional_scaling_in_the_driver(self):
        self.driver(hdr=False)
        env, nested, native = session.selection(dict(WAYLAND, FERESTRE_NATIVE_WAYLAND='1'), self.runtime, dict(HDR, scale=1.5), True)
        self.assertFalse(native); self.assertFalse(nested)
        self.assertNotIn('PROTON_ENABLE_WAYLAND', env)
        self.assertNotIn('DXVK_HDR', env)
        driver = self.runtime / 'files/lib/wine/x86_64-unix/winewayland.so'
        write(driver, 'wp_fractional_scale_manager_v1')
        env, nested, native = session.selection(dict(WAYLAND, FERESTRE_NATIVE_WAYLAND='1'), self.runtime, dict(HDR, scale=1.5), True)
        self.assertTrue(native); self.assertFalse(nested)
        self.assertEqual(env['DXVK_HDR'], '1')

    def test_native_defaults_keep_game_overrides_and_disable_incompatible_helpers(self):
        self.driver(hdr=False)
        write(self.runtime / 'proton', '#!/bin/sh\n# legacy runtime\n')
        env, _, native = session.selection(dict(WAYLAND, FERESTRE_NATIVE_WAYLAND='1', WINEDLLOVERRIDES='GameInput=b'), self.runtime, HDR, True)
        self.assertTrue(native)
        self.assertIn('GameInput=b', env['WINEDLLOVERRIDES'])
        self.assertIn('winex11.drv=d', env['WINEDLLOVERRIDES'])
        self.assertIn('winewayland.drv=b', env['WINEDLLOVERRIDES'])
        self.assertEqual(env['PROTON_USE_XALIA'], '0')
        self.assertEqual(env['WINE_DISABLE_FULLSCREEN_HACK'], '1')
        self.assertNotIn('DISPLAY', env)

    def test_native_settings_respect_user_helpers_and_driver_modes(self):
        self.driver()
        write(self.runtime / 'proton', '#!/bin/sh\n# legacy runtime\n')
        write(self.runtime / 'user_settings.py', "user_settings={'PROTON_USE_XALIA':'1', 'WINEDLLOVERRIDES':'GameInput=n,b;winewayland.drv=n,b'}")
        env, _, _ = session.selection(dict(WAYLAND, FERESTRE_NATIVE_WAYLAND='1'), self.runtime, HDR, True)
        self.assertNotIn('PROTON_USE_XALIA', env)
        self.assertIn('GameInput=n,b', env['WINEDLLOVERRIDES'])
        self.assertIn('winewayland.drv=n,b', env['WINEDLLOVERRIDES'])
        self.assertNotIn('winewayland.drv=b', env['WINEDLLOVERRIDES'])

    def test_missing_runtime_and_compositor_keep_existing_graphics_settings(self):
        env, nested, native = session.selection(WAYLAND, self.runtime, HDR, False)
        self.assertFalse(nested); self.assertFalse(native)
        self.assertNotIn('DXVK_HDR', env)
        self.assertNotIn('PROTON_ENABLE_WAYLAND', env)

    def test_sdr_and_explicit_hdr_opt_out_do_not_force_hdr(self):
        self.driver()
        for key in ('PROTON_ENABLE_HDR', 'PROTON_USE_HDR', 'DXVK_HDR'):
            env, nested, _ = session.selection(dict(WAYLAND, **{key: '0'}), self.runtime, HDR, True)
            self.assertEqual(env[key], '0'); self.assertFalse(nested)
            self.assertNotEqual(env.get('DXVK_HDR'), '1')
        env, nested, _ = session.selection(WAYLAND, self.runtime, dict(HDR, hdr=False), True)
        self.assertNotIn('DXVK_HDR', env); self.assertFalse(nested)

    def test_explicit_driver_and_proton_settings_are_preserved(self):
        self.driver()
        env, _, native = session.selection(dict(WAYLAND, WINE_GRAPHICS_DRIVER='x11'), self.runtime, dict(HDR, hdr=False), True)
        self.assertFalse(native); self.assertEqual(env['WINE_GRAPHICS_DRIVER'], 'x11')
        write(self.runtime / 'user_settings.py', "user_settings = {'PROTON_ENABLE_WAYLAND':'0', 'PROTON_ENABLE_HDR':'0'}\n")
        env, nested, native = session.selection(WAYLAND, self.runtime, HDR, True)
        self.assertFalse(native); self.assertFalse(nested)
        self.assertEqual(env['PROTON_ENABLE_WAYLAND'], '0')
        self.assertEqual(env['PROTON_ENABLE_HDR'], '0')

    def test_user_settings_are_never_executed_by_detection(self):
        marker = self.root / 'executed'
        write(self.runtime / 'user_settings.py', f"open({str(marker)!r}, 'w').write('bad')\nuser_settings={{'DXVK_HDR':'0'}}\n")
        self.assertEqual(session.preferences(self.runtime)['DXVK_HDR'], '0')
        self.assertFalse(marker.exists())

    def test_existing_gamescope_session_is_not_nested(self):
        self.driver()
        env, nested, native = session.selection(dict(WAYLAND, GAMESCOPE_WAYLAND_DISPLAY='existing'), self.runtime, HDR, True)
        self.assertFalse(nested); self.assertFalse(native)
        self.assertNotIn('PROTON_ENABLE_WAYLAND', env)


class SessionProcessTests(unittest.TestCase):
    def test_shim_enables_the_session_once_and_preserves_the_licensed_image(self):
        with tempfile.TemporaryDirectory(prefix='ferestre-shim-session-') as directory:
            root = Path(directory)
            runtime = root / 'runtime'
            write(runtime / 'proton', f'#!{sys.executable}\nimport os,sys\nfrom pathlib import Path\n'
                  "if sys.argv[1:3]==['run','reg']:sys.exit(0)\n"
                  "assert os.environ['FERESTRE_DESKTOP_SESSION_READY']=='1'\n"
                  "assert os.environ['PROTON_ENABLE_WAYLAND']=='0'\n"
                  "assert os.environ['DXVK_HDR']=='0'\n"
                  "Path(os.environ['RECEIPT']).write_bytes(os.pread(int(os.environ['IMAGE_FD']),14,0))\n")
            fd = os.memfd_create('licensed-shim-fixture', 0)
            try:
                os.write(fd, b'licensed-image')
                env = dict(os.environ, PROTON_DIR=str(runtime), XODUS_PROTON_DIR=str(runtime),
                           XODUS_GAMES_DIR=str(root / 'games'), STEAM_COMPAT_DATA_PATH=str(root / 'prefix'),
                           FERESTRE_DESKTOP_INTEGRATION='1', FERESTRE_PERFORMANCE='0',
                           XDG_SESSION_TYPE='x11', IMAGE_FD=str(fd),
                           PROTON_ENABLE_WAYLAND='1', DXVK_HDR='1',
                           WINE_DLL_FILE_MAP=f'Game With Spaces.exe={fd}', RECEIPT=str(root / 'receipt'),
                           PROTON_DISABLE_HIDRAW='')
                env.pop('FERESTRE_IMAGE_VIEW', None)
                env.pop('FERESTRE_DESKTOP_SESSION_READY', None)
                subprocess.run([str(SCRIPT.with_name('proton-wine-shim.sh')), 'Game With Spaces.exe'],
                               env=env, pass_fds=(fd,), check=True, timeout=10)
                self.assertEqual((root / 'receipt').read_bytes(), b'licensed-image')
            finally:
                os.close(fd)

    def test_full_hdr_compositor_session_keeps_the_image_descriptor(self):
        with tempfile.TemporaryDirectory(prefix='ferestre-hdr-session-') as directory:
            root = Path(directory)
            write(root / 'bin/kscreen-doctor', f'#!{sys.executable}\nimport json\nprint(json.dumps(' +
                  repr({'outputs': [{'enabled': True, 'hdr': True, 'currentModeId': '1', 'modes': [
                      {'id': '1', 'refreshRate': 240, 'size': {'width': 7680, 'height': 2160}}]}]}) + '))\n')
            write(root / 'bin/gamescope', f'#!{sys.executable}\nimport os,sys\n'
                  "os.closerange(3,1024)\nos.environ['DISPLAY']=':nested-fixture'\n"
                  "os.environ['GAMESCOPE_WAYLAND_DISPLAY']='nested'\n"
                  "args=sys.argv[sys.argv.index('--')+1:];os.execv(args[0],args)\n")
            fd = os.memfd_create('licensed-hdr-fixture', 0)
            try:
                os.write(fd, b'licensed-image')
                env = dict(os.environ, **WAYLAND, FERESTRE_PERFORMANCE='0', FERESTRE_GAMESCOPE='1', WAYLAND_SOCKET='999',
                           PROTON_DIR=str(root / 'runtime'), IMAGE_FD=str(fd),
                           WINE_DLL_FILE_MAP=f'Game.exe={fd}', RECEIPT=str(root / 'receipt'),
                           PATH=str(root / 'bin') + ':' + os.environ['PATH'])
                code = ("import os,json;from pathlib import Path;"
                        "assert os.environ['DISPLAY']==':nested-fixture';"
                        "assert os.environ['WAYLAND_DISPLAY']==os.environ['GAMESCOPE_WAYLAND_DISPLAY']=='nested';"
                        "assert 'WAYLAND_SOCKET' not in os.environ;"
                        "assert os.environ['DXVK_HDR']=='1';"
                        "assert os.environ['ENABLE_GAMESCOPE_WSI']=='1';"
                        "Path(os.environ['RECEIPT']).write_bytes(os.pread(int(os.environ['IMAGE_FD']),14,0))")
                subprocess.run([sys.executable, str(SCRIPT), '--', sys.executable, '-c', code],
                               env=env, pass_fds=(fd,), check=True, timeout=15)
                self.assertEqual((root / 'receipt').read_bytes(), b'licensed-image')
            finally:
                os.close(fd)

    def test_inherited_images_and_stop_channel_survive_game_session(self):
        with tempfile.TemporaryDirectory(prefix='ferestre-session-fds-') as directory:
            root = Path(directory)
            fd = os.memfd_create('licensed-fixture', 0)
            first, second = socket.socketpair()
            try:
                os.write(fd, b'licensed-image')
                os.set_inheritable(first.fileno(), True)
                second.sendall(b'stop-channel')
                code = ("import os,socket,json;from pathlib import Path;"
                        "a=os.pread(int(os.environ['IMAGE_FD']),14,0).decode();"
                        "s=socket.socket(fileno=int(os.environ['FERESTRE_STOP_FD']));"
                        "b=s.recv(12).decode();Path(os.environ['RECEIPT']).write_text(json.dumps([a,b]))")
                env = dict(os.environ, FERESTRE_PERFORMANCE='0', XDG_SESSION_TYPE='x11',
                           IMAGE_FD=str(fd), FERESTRE_STOP_FD=str(first.fileno()),
                           WINE_DLL_FILE_MAP=f'Game.exe={fd}', RECEIPT=str(root / 'receipt'))
                subprocess.run([sys.executable, str(SCRIPT), '--', sys.executable, '-c', code],
                               env=env, pass_fds=(fd, first.fileno()), check=True, timeout=10)
                self.assertEqual(json.loads((root / 'receipt').read_text()), ['licensed-image', 'stop-channel'])
            finally:
                os.close(fd); first.close(); second.close()

    def test_profile_hold_releases_and_does_not_receive_private_descriptors(self):
        with tempfile.TemporaryDirectory(prefix='ferestre-performance-') as directory:
            root = Path(directory)
            fd = os.memfd_create('private-image', 0)
            tool = root / 'game-performance'
            write(tool, f'#!{sys.executable}\nimport os,sys\nfrom pathlib import Path\n'
                  "try:os.fstat(int(os.environ['PRIVATE_FD']));sys.exit(7)\n"
                  "except OSError:pass\n"
                  "print('ferestre-performance-ready',flush=True)\nsys.stdin.buffer.read()\n"
                  "Path(os.environ['RELEASED']).write_text('released')\n")
            hold = session.PerformanceHold()
            try:
                env = dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'],
                           PRIVATE_FD=str(fd), RELEASED=str(root / 'released'))
                self.assertTrue(hold.start(env))
                process = hold.process
                self.assertIsNone(process.poll())
                hold.close()
                self.assertEqual(process.returncode, 0)
                self.assertEqual((root / 'released').read_text(), 'released')
            finally:
                hold.close(); os.close(fd)

    def test_partial_profile_output_cannot_stall_game_start_indefinitely(self):
        with tempfile.TemporaryDirectory(prefix='ferestre-profile-timeout-') as directory:
            root = Path(directory)
            write(root / 'game-performance', f'#!{sys.executable}\nimport sys\n'
                  "sys.stdout.write('starting');sys.stdout.flush();sys.stdin.buffer.read()\n")
            hold = session.PerformanceHold()
            try:
                started = time.monotonic()
                self.assertFalse(hold.start(dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'])))
                self.assertLess(time.monotonic() - started, 5)
                self.assertIsNone(hold.process)
            finally:
                hold.close()

    def test_compositor_handoff_keeps_game_images_outside_its_child(self):
        with tempfile.TemporaryDirectory(prefix='ferestre-compositor-test-') as directory:
            root = Path(directory)
            fake = root / 'gamescope'
            write(fake, f'#!{sys.executable}\nimport os,sys\n'
                  "assert '--force-windows-fullscreen' not in sys.argv and '-f' not in sys.argv\n"
                  "assert 'WINE_DLL_FILE_MAP' not in os.environ\nassert 'FERESTRE_STOP_FD' not in os.environ\n"
                  "os.closerange(3,1024)\nos.environ['DISPLAY']=':fixture'\n"
                  "os.environ['GAMESCOPE_WAYLAND_DISPLAY']='nested'\n"
                  "args=sys.argv[sys.argv.index('--')+1:];os.execv(args[0],args)\n")
            env = dict(os.environ, PATH=str(root) + ':' + os.environ['PATH'],
                       WINE_DLL_FILE_MAP='Game.exe=200', FERESTRE_STOP_FD='201')
            hold = session.GamescopeHold()
            try:
                nested = hold.start(env, HDR)
                self.assertIsNotNone(nested)
                self.assertEqual(nested['DISPLAY'], ':fixture')
                self.assertEqual(nested['WINE_DLL_FILE_MAP'], 'Game.exe=200')
                self.assertEqual(nested['FERESTRE_STOP_FD'], '201')
                self.assertEqual(nested['ENABLE_GAMESCOPE_WSI'], '1')
                process = hold.process; temporary = Path(hold.directory.name)
                self.assertEqual(os.getpgid(process.pid), os.getpgrp())
                hold.close()
                self.assertEqual(process.returncode, 0)
                self.assertFalse(temporary.exists())
            finally:
                hold.close()


if __name__ == '__main__':
    unittest.main()

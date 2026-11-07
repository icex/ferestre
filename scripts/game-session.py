#!/usr/bin/env python3
"""Desktop-aware game environment, preserving the client's inherited images.

The performance/profile and optional compositor helpers run beside the game.
They must never sit between Xodus and Proton: both powerprofilesctl and
Gamescope close inherited descriptors when starting their command.
"""
import ast
import json
import math
import os
from pathlib import Path
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time


DISPLAY_ENV = ('DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'GAMESCOPE_WAYLAND_DISPLAY')
FLAGS = ('PROTON_ENABLE_WAYLAND', 'PROTON_USE_WAYLAND', 'WINE_GRAPHICS_DRIVER',
         'PROTON_ENABLE_HDR', 'PROTON_USE_HDR', 'DXVK_HDR', 'WINEDLLOVERRIDES',
         'PROTON_USE_XALIA', 'WINE_USE_EGL', 'WINE_DISABLE_FULLSCREEN_HACK')


def output(arguments):
    try:
        result = subprocess.run(arguments, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                timeout=3, close_fds=True, check=False)
        if result.returncode == 0 and len(result.stdout) <= 1024 * 1024:
            return json.loads(result.stdout)
    except (OSError, subprocess.SubprocessError, ValueError):
        pass
    return None


def mode(width, height, refresh, hdr, source):
    if (isinstance(width, int) and isinstance(height, int)
            and isinstance(refresh, (int, float)) and not isinstance(refresh, bool)
            and 16 <= width <= 32768 and 16 <= height <= 32768
            and math.isfinite(refresh) and 1 <= refresh <= 1000):
        return dict(width=width, height=height, refresh=refresh, hdr=hdr is True, source=source)
    return None


def kde_display(data):
    if not isinstance(data, dict) or not isinstance(data.get('outputs'), list):
        return None
    active = [o for o in data['outputs'] if isinstance(o, dict)
              and o.get('enabled') is True and o.get('connected') is not False]
    active.sort(key=lambda o: not (o.get('priority') == 1 or o.get('primary') is True))
    for monitor in active:
        modes = monitor.get('modes', [])
        if isinstance(modes, dict):
            modes = list(modes.values())
        if not isinstance(modes, list):
            continue
        for current in modes:
            if not isinstance(current, dict) or current.get('id') != monitor.get('currentModeId'):
                continue
            size = current.get('size', {})
            if isinstance(size, dict):
                return mode(size.get('width'), size.get('height'), current.get('refreshRate'),
                            monitor.get('hdr'), 'KDE')
    return None


def unvariant(value):
    if isinstance(value, dict):
        if set(value) == {'type', 'data'}:
            return unvariant(value['data'])
        return {k: unvariant(v) for k, v in value.items()}
    if isinstance(value, list):
        return [unvariant(v) for v in value]
    return value


def gnome_display(data):
    data = unvariant(data)
    if not isinstance(data, list) or len(data) != 4:
        return None
    monitors, logical = data[1:3]
    if not isinstance(monitors, list) or not isinstance(logical, list):
        return None
    active = [m for m in logical if isinstance(m, list) and len(m) == 7 and isinstance(m[5], list)]
    active.sort(key=lambda m: m[4] is not True)
    for logical_monitor in active:
        for spec in logical_monitor[5]:
            for monitor in monitors:
                if not isinstance(monitor, list) or len(monitor) != 3 or monitor[0] != spec:
                    continue
                properties = monitor[2]
                if not isinstance(properties, dict) or properties.get('is-for-lease') is True:
                    continue
                if not isinstance(monitor[1], list):
                    continue
                for current in monitor[1]:
                    if (isinstance(current, list) and len(current) == 7
                            and isinstance(current[6], dict) and current[6].get('is-current') is True):
                        return mode(current[1], current[2], current[3],
                                    properties.get('color-mode') == 1, 'GNOME')
    return None


def display(env, query=output):
    if (not env.get('WAYLAND_DISPLAY')
            or env.get('XDG_SESSION_TYPE', 'wayland').lower() != 'wayland'):
        return None
    desktop = (env.get('XDG_CURRENT_DESKTOP', '') + ':' + env.get('DESKTOP_SESSION', '')).lower()
    if 'kde' in desktop or 'plasma' in desktop:
        return kde_display(query(['kscreen-doctor', '-j']))
    if 'gnome' in desktop:
        return gnome_display(query([
            'busctl', '--user', '--json=short', 'call', 'org.gnome.Mutter.DisplayConfig',
            '/org/gnome/Mutter/DisplayConfig', 'org.gnome.Mutter.DisplayConfig', 'GetCurrentState']))
    return None


def preferences(runtime):
    """Respect Proton's settings file without executing it in the launcher."""
    try:
        tree = ast.parse((runtime / 'user_settings.py').read_text())
    except (OSError, SyntaxError, UnicodeError):
        return {}
    protected = {node.value: '' for node in ast.walk(tree)
                 if isinstance(node, ast.Constant) and node.value in FLAGS}
    for node in ast.walk(tree):
        if isinstance(node, ast.Dict):
            for key, value in zip(node.keys, node.values):
                if isinstance(key, ast.Constant) and key.value in FLAGS:
                    try:
                        protected[key.value] = str(ast.literal_eval(value))
                    except (ValueError, TypeError):
                        pass
    return protected


def wayland_driver(runtime):
    for lib in ('lib', 'lib64'):
        for name in ('winewayland.so', 'winewayland.drv.so'):
            path = runtime / 'files' / lib / 'wine/x86_64-unix' / name
            if path.is_file():
                return path
    return None


def native_hdr(driver):
    # Wine creates the Wayland Vulkan surface; the host Vulkan WSI implements
    # color management and advertises its HDR formats to DXVK/vkd3d. Those
    # protocols need not be embedded in Wine's own driver binary.
    return driver is not None


def enabled(value):
    return value not in ('', '0')


def selection(env, runtime, screen, gamescope_available):
    """Pure selection apart from inspecting the selected runtime's own files."""
    result = dict(env)
    policy = preferences(runtime)
    policy.update(env)
    driver = wayland_driver(runtime)
    hdr = screen is not None and screen['hdr']
    if any(key in policy and not enabled(policy[key])
           for key in ('PROTON_ENABLE_HDR', 'PROTON_USE_HDR', 'DXVK_HDR')):
        hdr = False
    native = screen is not None and driver is not None
    if any(key in policy and not enabled(policy[key])
           for key in ('PROTON_ENABLE_WAYLAND', 'PROTON_USE_WAYLAND')):
        native = False
    if 'WINE_GRAPHICS_DRIVER' in policy and policy['WINE_GRAPHICS_DRIVER'] != 'wayland':
        native = False
    already_nested = bool(env.get('GAMESCOPE_WAYLAND_DISPLAY'))
    use_gamescope = (hdr and not native_hdr(driver) and gamescope_available and not already_nested
                     and env.get('FERESTRE_GAMESCOPE') == '1'
                     and (driver is None or not any(enabled(policy.get(k, ''))
                                                    for k in ('PROTON_ENABLE_WAYLAND', 'PROTON_USE_WAYLAND'))))
    if already_nested:
        native = False
    if use_gamescope:
        native = False
    if native:
        if not any(k in policy for k in ('PROTON_ENABLE_WAYLAND', 'PROTON_USE_WAYLAND')):
            result['PROTON_ENABLE_WAYLAND'] = '1'
        # Upstream Proton does not consume GE's Wayland flag. Hiding DISPLAY
        # selects Wine's Wayland fallback, without editing the user's prefix.
        try:
            support = (runtime / 'proton').read_text(errors='replace')
        except OSError:
            support = ''
        # Xalia cannot position native Wayland windows. Keep this per-launch;
        # the prefix and desktop graphics settings must remain untouched.
        for key, value in (('PROTON_USE_XALIA', '0'), ('WINE_USE_EGL', '1'),
                           ('WINE_DISABLE_FULLSCREEN_HACK', '1')):
            if key not in policy:
                result[key] = value
        if 'PROTON_ENABLE_WAYLAND' not in support and 'WINE_GRAPHICS_DRIVER' not in policy:
            overrides = policy.get('WINEDLLOVERRIDES', '')
            specified = {name.strip().lower() for group in overrides.split(';')
                         for name in group.split('=', 1)[0].split(',')}
            defaults = [name + '=' + value for name, value in
                        (('winex11.drv', 'd'), ('winewayland.drv', 'b'))
                        if name not in specified]
            result['WINEDLLOVERRIDES'] = ';'.join(filter(None, [overrides, *defaults]))
            result.pop('DISPLAY', None)
    if hdr and (use_gamescope or (native and native_hdr(driver))):
        for key in ('PROTON_ENABLE_HDR', 'DXVK_HDR'):
            if key not in policy:
                result[key] = '1'
    if screen is not None:
        # Consumed by the runtime's overflow workaround; harmless to old builds.
        result.setdefault('FERESTRE_DESKTOP_MODE',
                          f"{screen['width']}x{screen['height']}@{screen['refresh']:.6f}")
    result['FERESTRE_DESKTOP_SESSION_READY'] = '1'
    return result, use_gamescope, native


def performance(os_release, available):
    identity = dict(line.split('=', 1) for line in os_release.splitlines()
                    if '=' in line and not line.lstrip().startswith('#'))
    if identity.get('ID', '').strip().strip('\"\'').lower() == 'cachyos' and available('game-performance'):
        return 'game-performance'
    return 'gamemoderun' if available('gamemoderun') else None


def stop_helper(process, group=True):
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        if group:
            os.killpg(process.pid, signal.SIGTERM)
        else:
            process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            if group:
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
            process.wait()


class PerformanceHold:
    def __init__(self):
        self.process = None

    def start(self, env):
        import select
        code = "import sys;print('ferestre-performance-ready',flush=True);sys.stdin.buffer.read()"
        try:
            self.process = subprocess.Popen(['game-performance', sys.executable, '-I', '-c', code],
                                            env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                            stderr=subprocess.DEVNULL, close_fds=True, start_new_session=True)
            deadline = time.monotonic() + 3
            data = bytearray()
            while len(data) < 4096:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not select.select([self.process.stdout], [], [], remaining)[0]:
                    break
                chunk = os.read(self.process.stdout.fileno(), 4096 - len(data))
                if not chunk:
                    break
                data.extend(chunk)
                if b'\nferestre-performance-ready\n' in b'\n' + data:
                    return True
        except OSError:
            pass
        self.close()
        return False

    def close(self):
        if self.process is not None:
            self.process.stdin.close()
            stop_helper(self.process)
            self.process.stdout.close()
            self.process = None


class GamescopeHold:
    def __init__(self):
        self.process = self.connection = self.directory = None

    def start(self, env, screen):
        self.directory = tempfile.TemporaryDirectory(prefix='ferestre-compositor-')
        address = str(Path(self.directory.name) / 'ready')
        try:
            with socket.socket(socket.AF_UNIX) as listener:
                listener.bind(address)
                listener.listen(1)
                listener.settimeout(8)
                args = ['gamescope', '--backend', 'wayland', '--hdr-enabled',
                        '-W', str(screen['width']), '-H', str(screen['height']),
                        '-w', str(screen['width']), '-h', str(screen['height']),
                        '-r', str(round(screen['refresh'])), '--',
                        sys.executable, '-I', str(Path(__file__).resolve()), '--report-display', address]
                isolated = {k: v for k, v in env.items()
                            if k not in ('WINE_DLL_FILE_MAP', 'WINE_PACKAGE_MANIFEST', 'FERESTRE_STOP_FD')}
                # Keep the compositor in the title's process group so Home,
                # Resume and Stop identify its outer desktop window too.
                self.process = subprocess.Popen(args, env=isolated, close_fds=True)
                self.connection, _ = listener.accept()
            credentials = self.connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
            if struct.unpack('3i', credentials)[1] != os.getuid():
                raise ValueError('unexpected compositor helper owner')
            self.connection.settimeout(3)
            data = bytearray()
            while not data.endswith(b'\n') and len(data) < 65536:
                chunk = self.connection.recv(4096)
                if not chunk:
                    break
                data.extend(chunk)
            context = json.loads(data)
            if (not isinstance(context, dict) or not context.get('DISPLAY')
                    or not context.get('GAMESCOPE_WAYLAND_DISPLAY')):
                raise ValueError('compositor did not report a display')
            result = dict(env)
            for key in DISPLAY_ENV:
                if key in context and isinstance(context[key], str):
                    result[key] = context[key]
            # Gamescope leaves WAYLAND_DISPLAY pointing at its parent. Its
            # WSI layer bypasses that parent only when the game's Wayland
            # locator identifies Gamescope too; otherwise HDR formats vanish.
            result['WAYLAND_DISPLAY'] = result['GAMESCOPE_WAYLAND_DISPLAY']
            result.pop('WAYLAND_SOCKET', None)
            result['ENABLE_GAMESCOPE_WSI'] = '1'
            return result
        except (OSError, ValueError):
            self.close()
            return None

    def close(self):
        if self.connection is not None:
            self.connection.close()
            self.connection = None
        if self.process is not None:
            stop_helper(self.process, group=False)
            self.process = None
        if self.directory is not None:
            self.directory.cleanup()
            self.directory = None


def report_display(address):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(10)
        connection.connect(address)
        context = {k: os.environ[k] for k in DISPLAY_ENV if k in os.environ}
        connection.sendall(json.dumps(context).encode() + b'\n')
        connection.settimeout(None)
        while connection.recv(1):
            pass


def run(command, env, screen):
    runtime = Path(env.get('PROTON_DIR', env.get('XODUS_PROTON_DIR', '')))
    selected, use_gamescope, native = selection(env, runtime, screen, bool(shutil.which('gamescope')))
    hold, compositor = PerformanceHold(), GamescopeHold()
    child = None
    pending = []
    def forward(signum, _):
        pending.append(signum)
        if child is not None and child.poll() is None:
            try:
                child.send_signal(signum)
            except ProcessLookupError:
                pass
    previous = {sig: signal.signal(sig, forward) for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP)}
    try:
        try:
            release = Path('/etc/os-release').read_text()
        except OSError:
            release = ''
        wrapper = performance(release, shutil.which) if env.get('FERESTRE_PERFORMANCE', '1') != '0' else None
        if wrapper == 'game-performance':
            if hold.start(env):
                print(':: CachyOS performance profile held for this game', file=sys.stderr)
            else:
                print('-- CachyOS performance profile unavailable; continuing without it', file=sys.stderr)
        if use_gamescope:
            nested = compositor.start(selected, screen)
            if nested is not None:
                selected = nested
                print(f":: desktop HDR through Gamescope ({screen['width']}x{screen['height']} at {screen['refresh']:g} Hz)", file=sys.stderr)
            else:
                # No game has started. Keep the previous working display path.
                selected, _, _ = selection(env, runtime, screen, False)
                print('-- HDR compositor unavailable; continuing without compositor HDR', file=sys.stderr)
        elif native:
            print(':: using the selected runtime\'s native Wayland display driver', file=sys.stderr)
        elif screen and screen['hdr']:
            print('-- selected runtime has no native Wayland HDR support; existing display settings were kept', file=sys.stderr)
        if wrapper == 'gamemoderun':
            command = ['gamemoderun', *command]
        if pending:
            return 128 + pending[0]
        # Xodus image descriptors and the GUI Stop channel must reach Proton.
        child = subprocess.Popen(command, env=selected, close_fds=False)
        for signum in pending:
            if child.poll() is None:
                child.send_signal(signum)
        code = child.wait()
        return code if code >= 0 else 128 - code
    finally:
        compositor.close()
        hold.close()
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == '--report-display':
        report_display(sys.argv[2])
        return 0
    if len(sys.argv) < 3 or sys.argv[1] != '--':
        print('usage: game-session.py -- command [arguments...]', file=sys.stderr)
        return 2
    env = dict(os.environ)
    return run(sys.argv[2:], env, display(env))


if __name__ == '__main__':
    raise SystemExit(main())

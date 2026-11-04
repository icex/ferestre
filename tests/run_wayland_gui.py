#!/usr/bin/env python3
"""Run a GUI fixture inside a private, software-rendered native KWin session.

Example: python3 tests/run_wayland_gui.py tests/gui_running.py --binary target/debug/ferestre-gui
Requires kwin_wayland, qdbus6 and the AT-SPI registry. No user settings change.
"""
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time


def main():
    if os.environ.get('FERESTRE_WAYLAND_SESSION')!='1':
        env=dict(os.environ,FERESTRE_WAYLAND_SESSION='1')
        return subprocess.call(['dbus-run-session','--',sys.executable,__file__,*sys.argv[1:]],env=env)
    with tempfile.TemporaryDirectory(prefix='ferestre-wayland-gui-') as tmp:
        root=Path(tmp)
        for part in ('home','runtime','config','data'):(root/part).mkdir(mode=0o700)
        env=dict(os.environ,HOME=str(root/'home'),XDG_RUNTIME_DIR=str(root/'runtime'),XDG_CONFIG_HOME=str(root/'config'),XDG_DATA_HOME=str(root/'data'),QT_QPA_PLATFORM='offscreen',LIBGL_ALWAYS_SOFTWARE='1',GSK_RENDERER='cairo',FERESTRE_TEST_WM='1',FERESTRE_TEST_PRIVATE_KWIN='1',FERESTRE_TEST_NATIVE_WAYLAND='1',GDK_BACKEND='wayland',GTK_USE_PORTAL='0')
        for key in ('DISPLAY','WAYLAND_DISPLAY','AT_SPI_BUS_ADDRESS'):env.pop(key,None)
        lines=subprocess.check_output(['dbus-daemon','--session','--fork','--print-address=1','--print-pid=1'],text=True).splitlines()
        env['AT_SPI_BUS_ADDRESS']=lines[0];bus_pid=int(lines[1]);registry=None;kwin=None
        try:
            registry_path=next((p for p in ('/usr/lib/at-spi2-registryd','/usr/libexec/at-spi2-registryd','/usr/libexec/at-spi2-core/at-spi2-registryd') if Path(p).is_file()),None)
            if not registry_path:raise RuntimeError('AT-SPI registry not found')
            with (root/'session.log').open('wb') as log:
                registry=subprocess.Popen([registry_path],env=env,stdout=log,stderr=log)
                kwin=subprocess.Popen(['kwin_wayland','--virtual','--width','1280','--height','720','--no-lockscreen','--no-global-shortcuts','--no-kactivities','--socket','ferestre-test-wayland'],env=env,stdout=log,stderr=log)
            until=time.monotonic()+20
            while not (root/'runtime/ferestre-test-wayland').exists():
                if kwin.poll() is not None or time.monotonic()>until:raise RuntimeError('private KWin did not start')
                time.sleep(.1)
            env['WAYLAND_DISPLAY']='ferestre-test-wayland'
            result=subprocess.call([sys.executable,*sys.argv[1:]],env=env)
            if result:print((root/'session.log').read_text()[-5000:],flush=True)
            return result
        finally:
            for child in (kwin,registry):
                if child is not None and child.poll() is None:child.terminate();child.wait(timeout=10)
            os.kill(bus_pid,signal.SIGTERM)
if __name__=='__main__':raise SystemExit(main())

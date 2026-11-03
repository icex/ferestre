#!/usr/bin/env python3
"""Launch spinner, per-game readiness, Stop, and background Home on private fixtures."""
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
from gui_controller import SDL
from gui_updater import write, deadline

CLI = r'''#!/usr/bin/env python3
import json,os,signal,subprocess,sys,time
from pathlib import Path
h=Path(os.environ['HOME'])
if sys.argv[1:2]!=['run']:
 print('{}');sys.exit(0)
id=sys.argv[2]
(h/('launch-'+id)).write_text(str(os.getpid()))
while not (h/('ready-'+id)).exists():
 if (h/('fail-'+id)).exists():
  print('!! Fixture startup failed',file=sys.stderr);sys.exit(1)
 time.sleep(.03)
child=subprocess.Popen(['bash','-c','exec -a Game.exe sleep 120'])
try:
 while not (h/('end-'+id)).exists(): time.sleep(.03)
finally:
 child.terminate();child.wait(timeout=5)
'''


def active_pid():
    x=ctypes.CDLL(ctypes.util.find_library('X11'));ptr=ctypes.c_void_p;ulong=ctypes.c_ulong
    x.XOpenDisplay.argtypes=[ctypes.c_char_p];x.XOpenDisplay.restype=ptr
    x.XDefaultRootWindow.argtypes=[ptr];x.XDefaultRootWindow.restype=ulong
    x.XInternAtom.argtypes=[ptr,ctypes.c_char_p,ctypes.c_int];x.XInternAtom.restype=ulong
    x.XGetWindowProperty.argtypes=[ptr,ulong,ulong,ctypes.c_long,ctypes.c_long,ctypes.c_int,ulong]+[ptr]*5
    x.XFree.argtypes=[ptr];x.XCloseDisplay.argtypes=[ptr]
    d=x.XOpenDisplay(None)
    try:
        def prop(w,name):
            kind=ulong();fmt=ctypes.c_int();count=ulong();left=ulong();data=ctypes.POINTER(ulong)()
            x.XGetWindowProperty(d,w,x.XInternAtom(d,name,0),0,1,0,0,ctypes.byref(kind),ctypes.byref(fmt),ctypes.byref(count),ctypes.byref(left),ctypes.byref(data))
            try:return data[0] if data and count.value else None
            finally:
                if data:x.XFree(data)
        w=prop(x.XDefaultRootWindow(d),b'_NET_ACTIVE_WINDOW')
        return prop(w,b'_NET_WM_PID') if w else None
    finally:x.XCloseDisplay(d)


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/ferestre-gui');args=parser.parse_args()
    s.Atspi.init();process=other=wm=None;frame=None
    with tempfile.TemporaryDirectory(prefix='ferestre-launch-ui-') as tmp:
        h=Path(tmp);(h/'bin').mkdir();shutil.copy2(Path(args.binary).resolve(),h/'bin/ferestre-gui')
        write(h/'bin/ferestre',CLI,True);write(h/'client/xodus-cli','#!/bin/sh\nexit 1\n',True)
        write(h/'runtime/proton','#!/bin/sh\nexit 0\n',True);write(h/'runtime/version','launch-fixture\n')
        write(h/'config/gtk-4.0/settings.ini','[Settings]\ngtk-enable-animations=false\n')
        write(h/'sdl.c',SDL);(h/'lib').mkdir();subprocess.run(['cc','-shared','-fPIC',str(h/'sdl.c'),'-o',str(h/'lib/libSDL2-2.0.so.0')],check=True)
        ids=['9ZZLAUNCH001','9ZZLAUNCH002']
        schema=int(re.search(r'const CACHE_SCHEMA: u32 = (\d+);',Path('crates/ferestre-core/src/catalog.rs').read_text())[1])
        for n,id in enumerate(ids):
            write(h/f'titles/{id}.toml',f'''schema=1
[title]
product-id="{id}"
name="Launch Game {n}"
slug="launch-{n}"
[launch]
executable="Game.exe"
[status]
state="untested"
summary="Private launch fixture"
''')
            write(h/f'games/launch-{n}/appxmanifest.xml','<Package><Identity Name="Launch.Fixture" Version="1.0.0.0" /></Package>')
            write(h/f'state/ferestre/installed/{id}.json',json.dumps({'product_id':id,'dir':str(h/f'games/launch-{n}')}))
            write(h/f'cache/ferestre/catalog/{id}.json',json.dumps({'schema':schema,'market':'US','language':'en-us','product':{'product_id':id,'name':f'Launch Game {n}','has_packages':True,'has_pc_package':True,'package_format':'MSIXVC'}}))
        write(h/'state/ferestre/library.json',json.dumps({'items':[{'productId':id,'productType':'Game','status':'Active'} for id in ids]}))
        def pad(buttons=0,kind=7):
            (h/'pad-next').write_bytes(struct.pack('<Hhhhh2xii',buttons,0,0,0,0,kind,1));(h/'pad-next').replace(h/'pad')
        pad()
        env=dict(os.environ,HOME=tmp,XDG_CONFIG_HOME=str(h/'config'),XDG_STATE_HOME=str(h/'state'),XDG_CACHE_HOME=str(h/'cache'),XDG_DATA_HOME=str(h/'data'),XODUS_CLI_DIR=str(h/'client'),XODUS_GAMES_DIR=str(h/'games'),XODUS_STEAM_DIR=str(h/'steam'),XODUS_PROTON_DIR=str(h/'runtime'),FERESTRE_TITLES=str(h/'titles'),GTK_A11Y='atspi',FERESTRE_APPLICATION_ID='io.github.icex.ferestre.LaunchTest',LD_LIBRARY_PATH=str(h/'lib'),FERESTRE_TEST_PAD_FILE=str(h/'pad'))
        for k in ('APPDIR','APPIMAGE','FERESTRE_PACKAGE_VERSION','FERESTRE_SCRIPTS_DIR'):env.pop(k,None)
        for k in ('HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','http_proxy','https_proxy','all_proxy'):env[k]='http://127.0.0.1:9'
        env['NO_PROXY']=env['no_proxy']=''
        if os.environ.get('FERESTRE_TEST_WM')!='1':
            wm=subprocess.Popen(['openbox','--sm-disable'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);time.sleep(.3)
        try:
            with (h/'gui.log').open('wb') as log:process=subprocess.Popen([h/'bin/ferestre-gui'],env=env,stdout=log,stderr=log)
            app=s.find_app(process.pid,deadline());frame=s.wait_for(app,lambda r:next(iter(s.find_all(r,role='frame')),None),'window',deadline())
            def wait(test,what):return s.wait_for(frame,test,what,deadline())
            s.select_in_list(frame,'My Library')
            def button(n,action):return s.row_button(frame,f'Launch Game {n}',action)
            def caption(n,text):
                node=button(n,'Stop');return node and any(text in t for t in s.texts(node))
            wait(lambda _:button(0,'Play'),'first Play')
            s.click_named(frame,'Launch Game 0 · Play')
            wait(lambda _:caption(0,'starting'),'starting caption')
            s.check(any('Starting game' in t for t in s.texts(button(0,'Stop'))),'starting tile contains the animated spinner')
            s.click_named(frame,'Launch Game 1 · Play');wait(lambda _:caption(1,'starting'),'second starting caption')
            (h/f'ready-{ids[0]}').touch();wait(lambda _:caption(0,'running'),'observed game becomes running')
            s.check(not any('Starting game' in t for t in s.texts(button(0,'Stop'))),'running tile removes its spinner')
            s.check(caption(1,'starting'),'another game remains starting independently')
            (h/f'fail-{ids[1]}').touch();wait(lambda _:button(1,'Play'),'failed launch returns to Play')
            s.check(caption(0,'running'),'one launch failing does not reset another game')
            (h/f'fail-{ids[1]}').unlink()
            s.click_named(frame,'Launch Game 1 · Play');wait(lambda _:caption(1,'starting'),'retry starts a fresh loading state')
            time.sleep(1.6);s.click_named(frame,'Launch Game 1 · Stop')
            wait(lambda _:button(1,'Play'),'startup can be cancelled')
            s.check(caption(0,'running'),'cancelling startup leaves the other game running')
            # A separate GTK application represents the focused game window.
            standin="import gi;gi.require_version('Gtk','4.0');from gi.repository import Gtk;app=Gtk.Application(application_id='io.github.icex.ferestre.FocusFixture');app.connect('activate',lambda a:Gtk.ApplicationWindow(application=a,title='Game focus fixture',default_width=350,default_height=180).present());app.run()"
            with (h/'other.log').open('wb') as log:other=subprocess.Popen(['python3','-c',standin],env=env,stdout=log,stderr=log)
            wait(lambda _:active_pid()==other.pid,'another application owns focus')
            pad(1);time.sleep(.2);pad();time.sleep(.1)
            s.check(active_pid()==other.pid and button(0,'Stop'),'A in the background does not activate Ferestre or stop the game')
            for kind in (7,2):
                if kind == 2:
                    subprocess.run(['python3','-c',standin],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5,check=True)
                    wait(lambda _:active_pid()==other.pid,'game takes focus again')
                pad(1<<5,kind);wait(lambda _:active_pid()==process.pid,'PS/Xbox Home activates Ferestre')
                time.sleep(.2)
                s.check(active_pid()==process.pid and button(0,'Stop'),'Home brings the main window forward without activating Stop')
                pad(0,kind);time.sleep(.1)
            other.terminate();other.wait(timeout=5);other=None
            (h/f'end-{ids[0]}').touch();wait(lambda _:button(0,'Play'),'a game exiting normally resets its tile')
            (h/f'end-{ids[0]}').unlink()
            s.click_named(frame,'Launch Game 0 · Play');wait(lambda _:caption(0,'running'),'a new launch has its own readiness observation')
            time.sleep(1.6)
            s.click_named(frame,'Launch Game 0 · Stop')
            wait(lambda _:button(0,'Play'),'Stop returns the tile to Play')
            s.check(not any('Starting game' in t for t in s.texts(frame)),'exit clears per-game loading state')
            s.click_named(frame,'Close');s.check(process.wait(timeout=10)==0,'launch fixture closes cleanly')
            s.check('panicked at' not in (h/'gui.log').read_text(),'launch transitions do not panic')
            print('all launch/loading/Home checks passed',flush=True)
        except Exception:
            print((h/'gui.log').read_text(),flush=True)
            if frame is not None:s.dump(frame,'launch UI failure')
            raise
        finally:
            pad()
            for p in (other,process,wm):
                if p is not None and p.poll() is None:p.terminate();p.wait(timeout=5)
            # Test launch children own their sessions; clean only our private pids.
            for f in h.glob('launch-*'):
                try:
                    pid=int(f.read_text())
                    environ=Path(f'/proc/{pid}/environ').read_bytes().split(b'\0')
                    if ('HOME='+str(h)).encode() in environ and any(v.startswith(b'FERESTRE_LAUNCH_ID=') for v in environ):
                        os.killpg(pid,15)
                except (OSError,ValueError):pass

if __name__=='__main__':main()

#!/usr/bin/env python3
"""Input -> Home -> selected game Resume -> Stop/Cancel, with real game windows."""
import argparse
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import gui_smoke as s
from gui_launch import prepare,active_pid
from controller_driver import Driver,focus,start_window_manager


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/ferestre-gui');args=parser.parse_args()
    s.Atspi.init();process=None;app=None
    with tempfile.TemporaryDirectory(prefix='ferestre-running-ui-') as tmp:
        home=Path(tmp);ids,_,pad,env=prepare(home,args.binary)
        (home/'real-game-window').touch()
        wm=None
        try:
            wm=start_window_manager()
            with (home/'gui.log').open('wb') as log:process=subprocess.Popen([home/'bin/ferestre-gui'],env=env,stdout=log,stderr=log)
            app=s.find_app(process.pid,time.monotonic()+20)
            def wait(test,what):return s.wait_for(app,test,what,time.monotonic()+20)
            frame=wait(lambda root:next(iter(s.find_all(root,role='frame')),None),'window')
            s.select_in_list(frame,'My Library')
            driver=Driver(app,pad)
            game_pids=[];launch_pids=[]
            for n in (0,1):
                if n:
                    driver.pulse(5);wait(lambda _:active_pid()==process.pid,'Home returns to Ferestre')
                    driver.seek(f'Launch Game {n} · Play')
                else:driver.seek('Launch Game 0 · Play')
                driver.press(0,lambda root:s.row_button(root,f'Launch Game {n}','Stop'),'controller starts the selected game')
                (home/f'ready-{ids[n]}').touch()
                wait(lambda _: (home/('game-pid-'+ids[n])).exists(),'game process receipt')
                game_pid=int((home/('game-pid-'+ids[n])).read_text());game_pids.append(game_pid)
                launch_pids.append((home/('launch-'+ids[n])).read_text())
                if os.environ.get('FERESTRE_TEST_NATIVE_WAYLAND')=='1':
                    from wayland_focus import focus_fixture
                    time.sleep(.4);focus_fixture(game_pid)
                wait(lambda _:active_pid()==game_pid,'controlled game owns focus')
            for kind,label in [(4,'PS4'),(7,'PS5'),(2,'Xbox'),(0,'generic')]:
                driver.kind=kind
                driver.press(5,lambda _:active_pid()==process.pid,label+' Home brings Ferestre back')
                driver.seek('Launch Game 0 · Resume')
                driver.press(0,lambda _:active_pid()==game_pids[0],label+' Resume activates the selected existing game')
                s.check((home/('launch-'+ids[0])).read_text()==launch_pids[0],label+' Resume never launches a second copy')
                s.check(Path('/proc',str(game_pids[1])).exists(),'another game remains running')
            driver.press(5,lambda _:active_pid()==process.pid,'Home returns for Stop')
            driver.seek('Launch Game 0 · Stop')
            driver.press(0,lambda root:s.find_all(root,role='button',name='Stop Game'),'Stop opens an in-app confirmation')
            wait(lambda root: (node:=focus(root)) is not None and node.get_name()=='Cancel','Cancel is the safe initial selection')
            driver.press(0,lambda root:not s.find_all(root,role='button',name='Stop Game'),'A on default Cancel keeps the game alive')
            s.check(Path('/proc',str(game_pids[0])).exists(),'default Cancel did not stop the game')
            driver.seek('Launch Game 0 · Stop')
            driver.press(0,lambda root:s.find_all(root,role='button',name='Stop Game'),'reopen Stop confirmation')
            driver.press(1,lambda root:not s.find_all(root,role='button',name='Stop Game'),'B/circle cancels Stop')
            s.check(Path('/proc',str(game_pids[0])).exists(),'Back preserved the game')
            for n in (0,1):
                driver.seek(f'Launch Game {n} · Stop')
                driver.press(0,lambda root:s.find_all(root,role='button',name='Stop Game'),'Stop confirmation')
                driver.seek('Stop Game')
                driver.press(0,lambda root:s.row_button(root,f'Launch Game {n}','Play'),'confirmed controller Stop closes only this game')
                if n==0:s.check(Path('/proc',str(game_pids[1])).exists(),'stopping one game preserves the other')
            s.click_named(frame,'Close');s.check(process.wait(timeout=10)==0,'running-game fixture exits cleanly')
            print('all controller Home/Resume/Stop/Cancel checks passed',flush=True)
        except Exception:
            print((home/'gui.log').read_text(),flush=True)
            if app is not None:s.dump(app,'running-game failure')
            raise
        finally:
            pad()
            if wm is not None:
                wm.terminate();wm.wait(timeout=5)
            if process is not None and process.poll() is None:process.terminate();process.wait(timeout=5)
            for file in home.glob('launch-*'):
                try:
                    pid=int(file.read_text());envs=Path(f'/proc/{pid}/environ').read_bytes().split(b'\0')
                    if ('HOME='+str(home)).encode() in envs and any(v.startswith(b'FERESTRE_LAUNCH_ID=') for v in envs):os.killpg(pid,signal.SIGTERM)
                except (OSError,ValueError):pass
if __name__=='__main__':main()

#!/usr/bin/env python3
"""Controller runtime choices persist and feed the real CLI launch plan."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import tomllib
import gui_smoke as s
from gui_launch import prepare
from controller_driver import Driver, start_window_manager


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/ferestre-gui');args=parser.parse_args()
    s.Atspi.init();process=app=wm=None
    with tempfile.TemporaryDirectory(prefix='ferestre-runtime-choice-') as tmp:
        home=Path(tmp);ids,_,pad,env=prepare(home,args.binary)
        # Two managed runtimes plus the environment override from prepare().
        versions=['11.0.1','11.0.2']
        runtimes=[home/'state/ferestre/runtimes'/v for v in versions]
        for path,version in zip(runtimes,versions):
            path.mkdir(parents=True);(path/'proton').write_text('#!/bin/sh\nexit 0\n');(path/'version').write_text(version+'\n')
        choices=home/'config/ferestre/runtime-selections.toml'
        def saved():return tomllib.loads(choices.read_text()) if choices.exists() else {}
        def plan(id):
            result=subprocess.run([str(Path(args.binary).resolve().with_name('ferestre')),'--json','run',id,'--dry-run'],env=env,capture_output=True,text=True,timeout=20)
            if result.returncode:raise s.Failure(result.stderr)
            return json.loads(result.stdout)['plan']['env_set']['PROTON_DIR']
        try:
            wm=start_window_manager()
            with (home/'gui.log').open('wb') as log:process=subprocess.Popen([home/'bin/ferestre-gui'],env=env,stdout=log,stderr=log)
            app=s.find_app(process.pid,time.monotonic()+20)
            def wait(test,what):return s.wait_for(app,test,what,time.monotonic()+20)
            frame=wait(lambda root:next(iter(s.find_all(root,role='frame',name='Ferestre')),None),'window')
            s.select_in_list(frame,'Settings');driver=Driver(app,pad)
            driver.pulse(12);driver.seek('Choose default runtime')
            driver.press(0,lambda root:s.find_all(root,role='frame',name='Default game runtime'),'controller opens default runtime chooser')
            default_name=f'11.0.1 ({runtimes[0]})'
            driver.seek(default_name);driver.press(0,lambda _:saved().get('default')==str(runtimes[0]),'save global default')
            s.check(plan(ids[0])==str(runtimes[0]),'real CLI uses Settings default ahead of environment override')
            s.select_in_list(frame,'My Library')
            driver.pulse(12);driver.seek('Launch Game 0 · Play')
            driver.press(6,lambda root:s.find_all(root,role='button',name='Choose runtime'),'controller opens game options')
            driver.seek('Choose runtime');driver.press(0,lambda root:s.find_all(root,role='frame',name='Runtime for Launch Game 0'),'controller opens per-game runtime chooser')
            driver.seek(f'11.0.2 ({runtimes[1]})');driver.press(0,lambda _:saved().get('titles',{}).get(ids[0])==str(runtimes[1]),'save game override')
            s.check(plan(ids[0])==str(runtimes[1]) and plan(ids[1])==str(runtimes[0]),'game override applies only to its own launch')
            driver.seek('Launch Game 0 · Play');driver.press(6,lambda root:s.find_all(root,role='button',name='Choose runtime'),'reopen options')
            driver.seek('Choose runtime');driver.press(0,lambda root:s.find_all(root,role='button',name='Use default runtime'),'open inherited choice')
            driver.seek('Use default runtime');driver.press(0,lambda _:ids[0] not in saved().get('titles',{}),'restore inherited default')
            s.check(plan(ids[0])==str(runtimes[0]),'removing game override restores Settings default')
            s.select_in_list(frame,'Settings');driver.pulse(12);driver.seek('Choose default runtime')
            driver.press(0,lambda root:s.find_all(root,role='frame',name='Default game runtime'),'open default again')
            driver.seek('Automatic (newest compatible)');driver.press(0,lambda _:'default' not in saved(),'restore automatic default')
            s.check(plan(ids[0])==str(home/'runtime'),'automatic restores existing environment override')
            driver.seek('Quit Ferestre');driver.pulse(0);s.check(process.wait(timeout=10)==0,'runtime selection session closes cleanly')
            print('PASS: controller default/per-game runtime selection and real CLI precedence',flush=True)
        except Exception:
            print((home/'gui.log').read_text(),flush=True)
            if app is not None:s.dump(app,'runtime selection failure')
            raise
        finally:
            pad()
            if process is not None and process.poll() is None:process.terminate();process.wait(timeout=5)
            if wm is not None:wm.terminate();wm.wait(timeout=5)
if __name__=='__main__':main()

#!/usr/bin/env python3
"""Controller-only search, keyboard, install destination, queue and app exit."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import gui_smoke as s
from gui_launch import prepare
from gui_transfers import CLI
from controller_driver import Driver,focus,start_window_manager


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/ferestre-gui');args=parser.parse_args()
    s.Atspi.init();process=app=None
    with tempfile.TemporaryDirectory(prefix='ferestre-controller-install-') as tmp:
        home=Path(tmp);ids,_,pad,env=prepare(home,args.binary)
        (home/'bin/ferestre').write_text(CLI)
        shutil.rmtree(home/'state/ferestre/installed');shutil.rmtree(home/'games')
        (home/'Library').mkdir()
        wm=None
        try:
            wm=start_window_manager()
            with (home/'gui.log').open('wb') as log:process=subprocess.Popen([home/'bin/ferestre-gui'],env=env,stdout=log,stderr=log)
            app=s.find_app(process.pid,time.monotonic()+20)
            def wait(test,what):return s.wait_for(app,test,what,time.monotonic()+20)
            frame=wait(lambda root:next(iter(s.find_all(root,role='frame')),None),'window')
            s.select_in_list(frame,'My Library');driver=Driver(app,pad)
            driver.seek('Launch Game 0 · Install')
            driver.pulse(3);driver.press(0,lambda root:s.find_all(root,role='button',name='Done'),'controller opens the in-app keyboard')
            driver.seek('a');driver.pulse(0)
            driver.press(0,lambda root:s.find_all(root,role='label',name='aa'),'two rapid Accept presses both enter text')
            driver.press(1,lambda root:not s.find_all(root,role='button',name='Done'),'Back cancels text entry')
            def installed_choices():
                return all(s.row_button(frame,f'Launch Game {n}','Install') for n in range(3))
            s.check(installed_choices(),'cancelling the keyboard preserves the original unfiltered library')
            driver.pulse(3);driver.press(0,lambda root:s.find_all(root,role='button',name='Done'),'open keyboard again')
            draft=''
            for ch in 'needle':
                driver.seek(ch);draft+=ch
                driver.press(0,lambda root:s.find_all(root,role='label',name=draft),'keyboard draft '+repr(draft))
            def no_matches(root):return s.find_all(root,role='label',name='Nothing matches "needle"')
            driver.seek('Done');driver.press(0,no_matches,'Done applies the entered search to visible game results')
            driver.pulse(3);driver.press(0,lambda root:s.find_all(root,role='button',name='Done'),'reopen the search keyboard')
            wait(lambda root:s.find_all(root,role='label',name='needle'),'keyboard reads the previously committed field value')
            driver.seek('Clear');driver.pulse(0)
            driver.press(1,no_matches,'cancelling an edited draft preserves the existing search')
            driver.pulse(3);driver.press(0,lambda root:s.find_all(root,role='button',name='Done'),'open keyboard to clear search')
            driver.seek('Clear');driver.pulse(0)
            driver.seek('Done');driver.press(0,lambda _:installed_choices(),'clearing search restores the library')
            driver.seek('Launch Game 0 · Install')
            driver.press(0,lambda root:s.find_all(root,role='button',name='Choose install folder'),'controller opens Install confirmation')
            driver.press(1,lambda root:not s.find_all(root,role='button',name='Choose install folder'),'Back cancels installation')
            s.check(not (home/'calls').exists(),'cancelled install started no download')
            driver.seek('Launch Game 0 · Install')
            driver.press(0,lambda root:s.find_all(root,role='button',name='Choose install folder'),'install again')
            driver.seek('Choose install folder');driver.press(0,lambda root:s.find_all(root,role='button',name='Use This Folder'),'open in-app folder picker')
            driver.seek('Library');driver.pulse(0)
            driver.seek('Use This Folder');driver.press(0,lambda root:not s.find_all(root,role='button',name='Use This Folder'),'controller chooses a library folder')
            driver.seek('Install');driver.press(0,lambda _: (home/'calls').exists(),'controller confirms installation')
            wait(lambda _:ids[0] in (home/'calls').read_text(),'fixture download started')
            queue=json.loads((home/'state/ferestre/install-queue.json').read_text())['jobs']
            s.check(queue[0]['destination']==str(home/'Library'),'chosen folder reaches the durable download queue')
            driver.seek('Install queue');driver.press(0,lambda root:s.find_all(root,role='button',name='Pause'),'open Install Queue')
            def event(text):return (home/'events').exists() and text in (home/'events').read_text()
            driver.seek('Pause');driver.press(0,lambda _:event('pause '+ids[0]),'controller pauses download')
            driver.seek('Resume');driver.press(0,lambda _:event('resume '+ids[0]),'controller resumes download')
            driver.seek('Stop Download');driver.press(0,lambda _:event('stop '+ids[0]),'controller stops download and retains progress')
            driver.pulse(4);driver.seek('Settings');driver.press(0,lambda root:s.find_all(root,role='button',name='Quit Ferestre'),'controller opens Settings')
            driver.seek('Quit Ferestre');driver.pulse(0)
            s.check(process.wait(timeout=10)==0,'controller can quit after downloads stop')
            print('all controller install/keyboard/folder/queue checks passed',flush=True)
        except Exception:
            print((home/'gui.log').read_text(),flush=True)
            if app is not None:s.dump(app,'controller-install failure')
            raise
        finally:
            pad()
            if wm is not None:
                wm.terminate();wm.wait(timeout=5)
            if process is not None and process.poll() is None:process.terminate();process.wait(timeout=5)
if __name__=='__main__':main()

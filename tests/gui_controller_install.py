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
from controller_driver import Driver,focus


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/ferestre-gui');args=parser.parse_args()
    s.Atspi.init();process=app=None
    with tempfile.TemporaryDirectory(prefix='ferestre-controller-install-') as tmp:
        home=Path(tmp);ids,_,pad,env=prepare(home,args.binary)
        (home/'bin/ferestre').write_text(CLI)
        shutil.rmtree(home/'state/ferestre/installed');shutil.rmtree(home/'games')
        (home/'Library').mkdir()
        try:
            with (home/'gui.log').open('wb') as log:process=subprocess.Popen([home/'bin/ferestre-gui'],env=env,stdout=log,stderr=log)
            app=s.find_app(process.pid,time.monotonic()+20)
            def wait(test,what):return s.wait_for(app,test,what,time.monotonic()+20)
            frame=wait(lambda root:next(iter(s.find_all(root,role='frame')),None),'window')
            s.select_in_list(frame,'My Library');driver=Driver(app,pad)
            driver.seek('Launch Game 0 · Install')
            driver.pulse(3);driver.press(0,lambda root:s.find_all(root,role='button',name='Done'),'controller opens the in-app keyboard')
            driver.seek('a');driver.pulse(0)
            driver.press(1,lambda root:not s.find_all(root,role='button',name='Done'),'Back cancels text entry')
            def search_text():
                return next((s.Atspi.Text.get_text(n,0,-1) for n in s.find_all(frame,role='entry') if n.get_text_iface()),None)
            s.check(search_text()=='','cancelling the keyboard preserves the original search')
            driver.pulse(3);driver.press(0,lambda root:s.find_all(root,role='button',name='Done'),'open keyboard again')
            for ch in 'launch':driver.seek(ch);driver.pulse(0)
            driver.seek('Done');driver.press(0,lambda _:search_text()=='launch','Done commits controller-entered search text')
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
            if process is not None and process.poll() is None:process.terminate();process.wait(timeout=5)
if __name__=='__main__':main()

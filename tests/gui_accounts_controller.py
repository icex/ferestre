#!/usr/bin/env python3
"""Navigate account-role dropdowns using controller input, with synthetic profiles."""
import argparse,json,struct,subprocess,tempfile,time
from pathlib import Path
import gui_smoke as s
from gui_accounts import prepare,OWNER,PLAYER
from gui_controller import SDL
from controller_driver import Driver,focus,start_window_manager


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/ferestre-gui');args=parser.parse_args()
    process=app=None;s.Atspi.init()
    with tempfile.TemporaryDirectory(prefix='ferestre-account-pad-') as tmp:
        home=Path(tmp);env=prepare(home,Path(args.binary).resolve())
        profiles=json.loads((home/'profiles.json').read_text());profiles['profiles'].append({'id':PLAYER,'label':'Player Fixture','signed_in':True,'legacy_saves':False,'save_namespace':'b'*64});(home/'profiles.json').write_text(json.dumps(profiles))
        (home/'sdl.c').write_text(SDL);(home/'lib').mkdir();subprocess.run(['cc','-shared','-fPIC',home/'sdl.c','-o',home/'lib/libSDL2-2.0.so.0'],check=True)
        def pad(buttons=0,kind=4):
            (home/'pad-next').write_bytes(struct.pack('<Hhhhh2xii',buttons,0,0,0,0,kind,1));(home/'pad-next').replace(home/'pad')
        pad();env.update(LD_LIBRARY_PATH=str(home/'lib'),FERESTRE_TEST_PAD_FILE=str(home/'pad'))
        wm=None
        try:
            wm=start_window_manager()
            with (home/'gui.log').open('wb') as log:process=subprocess.Popen([home/'bin/ferestre-gui'],env=env,stdout=log,stderr=log)
            app=s.find_app(process.pid,time.monotonic()+20)
            def wait(test,what):return s.wait_for(app,test,what,time.monotonic()+20)
            button=wait(lambda root:next(iter(s.find_all(root,role='button',name_contains='Owner Fixture')),None),'Store profile loaded')
            driver=Driver(app,pad,4);driver.seek(button.get_name());driver.press(0,lambda root:s.find_all(root,role='button',name='Accounts'),'account menu')
            driver.seek('Accounts');driver.press(0,lambda root:s.find_all(root,role='frame',name='Accounts'),'controller opens Accounts')
            driver.seek('Player account');driver.pulse(0);driver.pulse(12);driver.pulse(0)
            wait(lambda root:any('Player Fixture plays with' in text for text in s.texts(root)),'controller selects the Player dropdown option')
            driver.seek('Apply accounts');driver.pulse(0)
            settings=home/'config/ferestre/account-roles.json'
            wait(lambda _:settings.exists() and json.loads(settings.read_text()).get('player')==PLAYER,'controller applies the selected roles')
            s.check(json.loads(settings.read_text())['store']==OWNER,'controller changes only the selected Player account')
            s.click_named(app,'Close');s.check(process.wait(timeout=10)==0,'account controller fixture closes cleanly')
            print('all account controller checks passed',flush=True)
        except Exception:
            print((home/'gui.log').read_text(),flush=True)
            if app is not None:s.dump(app,'account controller failure')
            raise
        finally:
            pad()
            if wm is not None:
                wm.terminate();wm.wait(timeout=5)
            if process is not None and process.poll() is None:process.terminate();process.wait(timeout=5)
if __name__=='__main__':main()

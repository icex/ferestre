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
            def signed_in(profile):
                return next(p['signed_in'] for p in json.loads((home/'profiles.json').read_text())['profiles'] if p['id']==profile)
            def account_menu():
                button=wait(lambda root:next(iter(s.find_all(root,role='button',name_contains='Owner Fixture')),None),'account menu button')
                driver.seek(button.get_name());driver.press(0,lambda root:s.find_all(root,role='button',name='Accounts'),'account menu')
            def confirm_open(heading):
                wait(lambda root:s.find_all(root,name=heading),'sign-out confirmation')
                wait(lambda root:(node:=focus(root)) is not None and node.get_name()=='Cancel','Cancel is the safe initial selection')
                s.check(signed_in(OWNER) and signed_in(PLAYER),'opening confirmation never signs an account out')
            account_menu()
            driver.seek('Sign out');driver.pulse(0);confirm_open('Sign out?')
            driver.press(0,lambda root:not s.find_all(root,name='Sign out?'),'default Cancel keeps Store sign-in')
            s.check(signed_in(OWNER),'controller Accept on default Cancel preserves Store credentials')
            account_menu()
            driver.seek('Accounts');driver.press(0,lambda root:s.find_all(root,role='frame',name='Accounts'),'Accounts window')
            for cancel in (0,1):
                driver.seek('Sign out Player Fixture');driver.pulse(0);confirm_open('Sign out Player Fixture?')
                driver.press(cancel,lambda root:not s.find_all(root,name='Sign out Player Fixture?'),'controller cancels profile sign-out')
                s.check(signed_in(PLAYER) and bool(s.find_all(app,role='frame',name='Accounts')),'Cancel/Back retains credentials and Accounts window')
            driver.seek('Sign out Player Fixture');driver.pulse(0);confirm_open('Sign out Player Fixture?')
            driver.seek('Sign out');driver.press(0,lambda _:not signed_in(PLAYER),'controller confirms Player-only sign-out')
            s.check(signed_in(OWNER),'confirmed Player sign-out preserves Store credentials')
            account_menu()
            driver.seek('Sign out');driver.pulse(0)
            wait(lambda root:s.find_all(root,name='Sign out?'),'Store confirmation')
            wait(lambda root:(node:=focus(root)) is not None and node.get_name()=='Cancel','Store Cancel default')
            driver.press(1,lambda root:not s.find_all(root,name='Sign out?'),'Back cancels Store sign-out')
            s.check(signed_in(OWNER),'Back preserves Store credentials')
            account_menu()
            driver.seek('Sign out');driver.pulse(0)
            wait(lambda root:s.find_all(root,name='Sign out?'),'Store confirmation again')
            driver.seek('Sign out');driver.press(0,lambda _:not signed_in(OWNER),'controller explicitly confirms Store sign-out')
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

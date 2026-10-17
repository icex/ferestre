#!/usr/bin/env python3
"""Real GTK/core launch path with two credential-free client profiles."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

import gui_smoke as s

OWNER = "11111111111111111111111111111111"
PLAYER = "22222222222222222222222222222222"
PRODUCT = "9TESTROLE0ID"


CLIENT = r'''#!/usr/bin/env python3
import json,os,sys,time
from pathlib import Path
root=Path(os.environ['HOME']);args=sys.argv[1:]
profile=os.environ.get('XODUS_ACCOUNT_PROFILE','1'*32)
if '--profile' in args:
 i=args.index('--profile');profile=args[i+1];args=args[:i]+args[i+2:]
profiles=json.loads((root/'profiles.json').read_text())
with (root/'client.log').open('a') as log:log.write(json.dumps({'args':args,'profile':profile})+'\n')
command=args[0]
if command=='migrate-accounts': print('{}')
elif command=='account-profiles':print(json.dumps(profiles))
elif command=='account':
 p=next(p for p in profiles['profiles'] if p['id']==profile)
 if not p['signed_in']:sys.exit(1)
 print(json.dumps({'gamertag':p['label'],'xuid':'fixture-'+profile[0]}))
elif command=='library':
 print(json.dumps({'items':[{'productId':'9TESTROLE0ID' if profile=='1'*32 else '9OTHERROLE00', 'productType':'Game','status':'Active'}]}))
elif command=='update':print(json.dumps({'version':'1.0.0.0'}))
elif command=='add-account':
 if (root/'cancel-login').exists():sys.exit(1)
 if not any(p['id']=='2'*32 for p in profiles['profiles']):
  profiles['profiles'].append({'id':'2'*32,'label':'Player Fixture','signed_in':True,'legacy_saves':False,'save_namespace':'b'*64})
 (root/'profiles.json').write_text(json.dumps(profiles));print('{}')
elif command=='logout':
 for p in profiles['profiles']:
  if p['id']==profile:p['signed_in']=False
 (root/'profiles.json').write_text(json.dumps(profiles))
elif command=='run':
 data={key:os.environ.get(key) for key in ['XODUS_ACCOUNT_PROFILE','XODUS_STORE_PROFILE','XODUS_PLAYER_PROFILE','STEAM_COMPAT_DATA_PATH','XODUS_RUNTIME_DIR']}
 (root/'launched.json').write_text(json.dumps(data))
 time.sleep(20)
else:sys.exit(2)
'''

SERVICE = r'''#!/usr/bin/env python3
# FERESTRE_ACCOUNT_ROLES_SERVICE_V1
import json,os,socket,sys
from pathlib import Path
if '--capabilities' in sys.argv:
 print(json.dumps({'schema':1,'capabilities':['store-player-accounts']}));sys.exit(0)
root=Path(os.environ['HOME']);path=Path(os.environ['XODUS_RUNTIME_DIR'])/'xodus.sock'
with (root/'services.jsonl').open('a') as log:
 log.write(json.dumps({'pid':os.getpid(),'store':os.environ['XODUS_STORE_PROFILE'],'player':os.environ['XODUS_PLAYER_PROFILE'],'socket':str(path)})+'\n')
with socket.socket(socket.AF_UNIX) as server:
 server.bind(str(path));server.listen()
 while True:
  connection,_=server.accept();connection.close()
'''


def write(path, text, executable=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    if executable:
        path.chmod(0o755)


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/ferestre-gui")
    parser.add_argument("--keep", action="store_true")
    args=parser.parse_args()
    binary=Path(args.binary).resolve()
    home=Path(tempfile.mkdtemp(prefix="fa-"))
    gui=None
    try:
        write(home/"profiles.json",json.dumps({"schema":1,"capabilities":["store-player-accounts"],"default_profile":OWNER,
            "profiles":[{"id":OWNER,"label":"Owner Fixture","signed_in":True,"legacy_saves":True,"save_namespace":"a"*64}]}))
        write(home/"client/xodus-cli",CLIENT,True)
        write(home/"client/xodus-service",SERVICE,True)
        write(home/"client/SERVICE_ID",hashlib.sha256(SERVICE.encode()).hexdigest())
        write(home/"runtime/proton","#!/bin/sh\n# close_fds=False\nexit 0\n",True)
        write(home/"runtime/version","0 ferestre-account-fixture\n")
        write(home/"runtime/files/share/ferestre/capabilities.json",json.dumps({"schema":1,"capabilities":[
            "xgameruntime.service-socket-override"]}))
        write(home/"runtime/files/bin/wineserver", "#!/bin/sh\nprintf '%s\\n' \"$WINEPREFIX\" > \"$HOME/stopped-prefix\"\n", True)
        write(home/"titles/role-test.toml",f'''schema=1
[title]
product-id="{PRODUCT}"
name="Account Test Game"
slug="account-test"
[launch]
executable="Game.exe"
[status]
state="playable"
summary="Synthetic account fixture"
''')
        game=home/"games/account-test"
        write(game/"MicrosoftGame.config",f'<Game><Identity Name="Fixture.AccountGame" Publisher="CN=Fixture" Version="1.0.0.0"/><ExecutableList><Executable Name="Game.exe"/></ExecutableList><StoreId>{PRODUCT}</StoreId></Game>')
        write(game/"Game.exe","fixture")
        state=home/"state/ferestre"
        write(state/f"installed/{PRODUCT}.json",json.dumps({"product_id":PRODUCT,"dir":str(game),"content_ids":["00000000-0000-0000-0000-000000000003"],"package_version":"1.0.0.0"}))
        write(home/"cache/ferestre/catalog"/f"{PRODUCT}.json",json.dumps({"schema":1,"market":"US","language":"en-us","product":{"product_id":PRODUCT,"name":"Account Test Game","has_packages":True,"has_pc_package":True,"package_format":"MSIXVC"}}))
        (home/"bin").mkdir()
        shutil.copy2(binary,home/"bin/ferestre-gui")
        (home/"bin/ferestre").symlink_to(binary.with_name("ferestre"))
        env=dict(os.environ,HOME=str(home),XDG_CONFIG_HOME=str(home/"config"),XDG_STATE_HOME=str(home/"state"),XDG_CACHE_HOME=str(home/"cache"),
            XODUS_CLI_DIR=str(home/"client"),XODUS_GAMES_DIR=str(home/"games"),XODUS_PROTON_DIR=str(home/"runtime"),XODUS_RUNTIME_DIR=str(home/"s"),
            FERESTRE_TITLES=str(home/"titles"),GTK_A11Y="atspi",HTTPS_PROXY="http://127.0.0.1:9",HTTP_PROXY="http://127.0.0.1:9",ALL_PROXY="http://127.0.0.1:9")
        for key in ("APPIMAGE","FERESTRE_ACCOUNT_ROLES"):
            env.pop(key,None)
        # Its own application id, so a Ferestre already open on this desktop
        # is not the instance that receives the activation.
        env["FERESTRE_APPLICATION_ID"]="io.github.icex.ferestre.AccountTest"
        gui=subprocess.Popen([home/"bin/ferestre-gui"],env=env,stderr=subprocess.PIPE)
        s.Atspi.init()
        app=s.find_app(gui.pid,time.monotonic()+20)
        frame=s.wait_for(app,lambda root:next(iter(s.find_all(root,role="frame",name="Ferestre")),None),"main window",time.monotonic()+20)

        def open_accounts():
            button=s.wait_for(frame,lambda root:next(iter(s.find_all(root,role="button",name_contains="Owner Fixture")),None),"account button",time.monotonic()+20)
            s.click(button)
            s.wait_for(app,lambda root:bool(s.find_all(root,name="Accounts")),"Accounts menu",time.monotonic()+20)
            s.click_named(app,"Accounts")
            return s.wait_for(app,lambda root:next(iter(s.find_all(root,role="frame",name="Accounts")),None),"Accounts dialog",time.monotonic()+20)

        dialog=open_accounts()
        write(home/"cancel-login", "cancel")
        s.click_named(dialog,"Add account")
        s.wait_for(app,lambda root:any("Sign-in was cancelled or failed" in t for t in s.texts(root)),"cancelled sign-in",time.monotonic()+20)
        s.check(not (home/"config/ferestre/account-roles.json").exists(),"cancelled sign-in keeps existing role selection")
        dialog=s.wait_for(app,lambda root:next((d for d in s.find_all(root,role="frame",name="Accounts") if len(s.find_all(d,role="combo box"))==2),None),"rebuilt accounts dialog",time.monotonic()+20)
        (home/"cancel-login").unlink()
        s.check("Store account" in s.texts(dialog) and "Player account" in s.texts(dialog),"both roles are explained",dialog)
        s.click_named(dialog,"Add account")
        s.wait_for(app,lambda root:len(json.loads((home/"profiles.json").read_text())["profiles"])==2,"new account metadata",time.monotonic()+20)
        dialog=s.wait_for(app,lambda root:next((d for d in s.find_all(root,role="frame",name="Accounts") if s.find_all(d,name="Player account")),None),"accounts after another sign-in",time.monotonic()+20)
        def choose_player(dialog,label):
            combo=s.find_all(dialog,role="combo box")[1]
            s.click(s.find_all(combo,role="toggle button")[0])
            def popup_row(root):
                for node in s.find_all(root,name=label):
                    if node.get_role_name()!="label":continue
                    ancestor=node.get_parent()
                    while ancestor is not None:
                        if ancestor.get_role_name()=="list item":return ancestor
                        ancestor=ancestor.get_parent()
                return None
            row=s.wait_for(app,popup_row,"player choice",time.monotonic()+20)
            s.press(row,gui.pid)
            s.wait_for(dialog,lambda root:s.find_all(root,role="combo box",name=label),"selected player",time.monotonic()+20)
        choose_player(dialog,"Player Fixture")
        s.click_named(dialog,"Apply accounts")
        settings=home/"config/ferestre/account-roles.json"
        s.wait_for(frame,lambda root:settings.exists(),"role settings",time.monotonic()+20)
        selected=json.loads(settings.read_text())
        s.check(selected['store']==OWNER and selected['player']==PLAYER,"owner and player are selected independently")
        s.wait_for(frame,lambda root:any("Player: Player Fixture" in t for t in s.texts(root)),"visible player identity",time.monotonic()+20)
        s.check(any("Store: Owner Fixture" in t for t in s.texts(frame)),"both identities are visible before launch",frame)
        s.select_in_list(frame,"Installed")
        play=s.wait_for(frame,lambda root:s.row_button(root,"Account Test Game","Play"),"Play",time.monotonic()+20)
        s.click(play)
        s.wait_for(frame,lambda root:any("Select an account-sharing runtime" in t for t in s.texts(root)),"old-runtime refusal",time.monotonic()+20)
        s.check(not (home/"launched.json").exists(),"old runtime cannot silently launch a split pair")
        write(home/"runtime/files/share/ferestre/capabilities.json",json.dumps({"schema":1,"capabilities":["xgameruntime.store-player-accounts","xgameruntime.service-socket-override"]}))
        generation=json.loads(settings.read_text())["generation"]
        dialog=open_accounts();s.click_named(dialog,"Apply accounts")
        s.wait_for(frame,lambda root:json.loads(settings.read_text())["generation"]>generation,"runtime reload after applying accounts",time.monotonic()+20)
        play=s.wait_for(frame,lambda root:s.row_button(root,"Account Test Game","Play"),"Play with upgraded runtime",time.monotonic()+20)
        s.click(play)
        s.wait_for(frame,lambda root:(home/"launched.json").exists(),"core launch record",time.monotonic()+20)
        launched=json.loads((home/"launched.json").read_text())
        s.check(launched['XODUS_STORE_PROFILE']==OWNER and launched['XODUS_PLAYER_PROFILE']==PLAYER,"actual launch keeps Store and Player contexts")
        s.check('players/'+('b'*64) in launched['STEAM_COMPAT_DATA_PATH'],"player receives an isolated prefix")
        services=[json.loads(line) for line in (home/"services.jsonl").read_text().splitlines()]
        s.check(services[0]['store']==OWNER and services[0]['player']==PLAYER,"service is bound to the same immutable pair")
        dialog=open_accounts()
        choose_player(dialog,"Owner Fixture")
        s.click_named(dialog,"Apply accounts")
        s.wait_for(frame,lambda root:json.loads(settings.read_text())["player"]==OWNER,"next-launch player change",time.monotonic()+20)
        s.check(json.loads((home/"launched.json").read_text())["XODUS_PLAYER_PROFILE"]==PLAYER,"changing selection preserves the running game's player")
        stop=s.wait_for(frame,lambda root:s.row_button(root,"Account Test Game","Stop"),"Stop",time.monotonic()+20)
        s.click(stop)
        s.wait_for(frame,lambda root:(home/"stopped-prefix").exists(),"prefix-specific shutdown",time.monotonic()+20)
        s.check((home/"stopped-prefix").read_text().strip()==launched['STEAM_COMPAT_DATA_PATH']+'/pfx',"Stop uses the launched player's prefix")
        dialog=open_accounts()
        choose_player(dialog,"Player Fixture")
        s.click_named(dialog,"Apply accounts")
        s.wait_for(frame,lambda root:json.loads(settings.read_text())["player"]==PLAYER,"restore split selection",time.monotonic()+20)
        dialog=open_accounts()
        s.click_named(dialog,"Sign out Player Fixture")
        s.wait_for(frame,lambda root:not json.loads((home/"profiles.json").read_text())["profiles"][1]["signed_in"],"Player-only logout",time.monotonic()+20)
        s.check(json.loads((home/"profiles.json").read_text())["profiles"][0]["signed_in"],"Player sign-out preserves Store credentials")
        s.wait_for(frame,lambda root:any("Store: Owner Fixture" in t for t in s.texts(root)),"Store identity after Player sign-out",time.monotonic()+20)
        s.check(any("Purchased" in t for t in s.texts(frame)),"Player sign-out preserves Store library access labels",frame)
        s.wait_for(frame,lambda root:any("Player Fixture (signed out)" in t for t in s.texts(root)),"signed-out Player label",time.monotonic()+20)
        (home/"launched.json").unlink()
        play=s.wait_for(frame,lambda root:s.row_button(root,"Account Test Game","Play"),"Play after Player sign-out",time.monotonic()+20)
        s.click(play)
        s.wait_for(frame,lambda root:any("Player account is signed out" in t for t in s.texts(root)),"signed-out Player refusal",time.monotonic()+20)
        s.check(not (home/"launched.json").exists(),"signed-out Player cannot launch using Store identity")
        s.click_named(frame,"Close")
        s.check(gui.wait(timeout=10)==0,"account test window closes")
        profiles=json.loads((home/"profiles.json").read_text())
        profiles["profiles"][0]["signed_in"]=False
        write(home/"profiles.json",json.dumps(profiles))
        settings.unlink()
        # Stand in for an account-menu sign-out, which also deletes the unscoped account cache.
        (state/"account.json").unlink(missing_ok=True)
        gui=subprocess.Popen([home/"bin/ferestre-gui"],env=env,stderr=subprocess.PIPE)
        app=s.find_app(gui.pid,time.monotonic()+20)
        frame=s.wait_for(app,lambda root:next(iter(s.find_all(root,role="frame",name="Ferestre")),None),"signed-out restart",time.monotonic()+20)
        button=s.wait_for(frame,lambda root:next(iter(s.find_all(root,role="button",name_contains="Not signed in")),None),"signed-out account button",time.monotonic()+20)
        s.wait_for(frame,lambda root:any(n.get_name()=="No activity" for n in s.find_all(root,role="progress bar")),"signed-out refresh completes",time.monotonic()+20)
        s.click(button)
        dialog=s.wait_for(app,lambda root:next(iter(s.find_all(root,role="frame",name="Accounts")),None),"reauthentication accounts dialog",time.monotonic()+20)
        s.check(bool(s.find_all(dialog,role="button",name="Add account")),"signed-out default profile can reopen Accounts without saved role settings")
        commands=[json.loads(line)["args"][0] for line in (home/"client.log").read_text().splitlines()]
        s.check("login" not in commands,"profile-aware reauthentication does not attempt the legacy identity-replacing login")
        s.click_named(dialog,"Close")
        s.wait_for(app,lambda root:not s.find_all(root,role="frame",name="Accounts"),"Accounts dialog closes",time.monotonic()+10)
        s.click_named(frame,"Close")
        s.check(gui.wait(timeout=10)==0,"signed-out restart closes")
        print("all account GUI checks passed")
    finally:
        if gui is not None and gui.poll() is None:
            gui.terminate();gui.wait(timeout=10)
        service_log=home/"services.jsonl"
        if service_log.exists():
            for line in service_log.read_text().splitlines():
                try:os.kill(json.loads(line)["pid"],15)
                except ProcessLookupError:pass
        if args.keep:
            print("kept",home)
        else:shutil.rmtree(home)


if __name__=="__main__":
    main()

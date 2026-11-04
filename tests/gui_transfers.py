#!/usr/bin/env python3
"""Drive the real queue/pause UI with an offline controlled child process."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

import gui_smoke as s

CLI=r'''#!/usr/bin/env python3
import json,os,select,sys,time
from pathlib import Path
h=Path(os.environ['HOME']);cmd=sys.argv[1]
if cmd=='check-app-updates':
 print(json.dumps({'schema':1,'launcher':None,'runtime':None,'appimage':None,'launcher_unavailable':None}));sys.exit(0)
if cmd!='install':print('{}');sys.exit(0)
id=sys.argv[2]
with (h/'calls').open('a') as out:out.write(id+'\n')
paused=False;sequence=0;done=0
while not (h/('finish-'+id)).exists():
 if select.select([sys.stdin],[],[],.05)[0]:
  byte=sys.stdin.buffer.read(1)
  if byte in [b'x',b'']:
   with (h/'events').open('a') as out:out.write('stop '+id+'\n')
   sys.exit(130)
  paused=byte==b'p'
  with (h/'events').open('a') as out:out.write(('pause ' if paused else 'resume ')+id+'\n')
 if not paused:done=min(done+1048576,100000000)
 sequence+=1
 print('FERESTRE_PROGRESS '+json.dumps({'schema':1,'event':'progress','sequence':sequence,'phase':'downloading','completedBytes':done,'totalBytes':200000000,'paused':paused,'canPause':True,'networkBytes':done//2,'reusedBytes':done//2}),flush=True)
if (h/('fail-'+id)).exists():
 print('FERESTRE_TRANSFER_DIAGNOSTIC_V1 transfer-status 403',file=sys.stderr);sys.exit(1)
sys.exit(0)
'''

def wait(predicate,what):
 deadline=time.monotonic()+s.TIMEOUT
 while time.monotonic()<deadline:
  try:value=predicate()
  except (FileNotFoundError,json.JSONDecodeError):value=False
  if value:return value
  time.sleep(.1)
 raise s.Failure('timed out waiting for '+what)

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/ferestre-gui');parser.add_argument('--keep',action='store_true');args=parser.parse_args()
 binary=Path(args.binary).resolve();home=Path(tempfile.mkdtemp(prefix='ferestre-queue-gui-'))
 def write(path,text,executable=False):
  path.parent.mkdir(parents=True,exist_ok=True);path.write_text(text)
  if executable:path.chmod(0o755)
 local=home/'bin/ferestre-gui';local.parent.mkdir();shutil.copy2(binary,local);write(home/'bin/ferestre',CLI,True)
 ids=['9ZZPAUSE0001','9ZZPAUSE0002'];names=['Queue One','Queue Two']
 for id,name in zip(ids,names):
  write(home/f'recipes/{id}.toml',f'''schema=1
[title]
product-id="{id}"
name="{name}"
slug="{id.lower()}"
[launch]
executable="Game.exe"
[status]
state="untested"
summary="Offline queue fixture"
''')
 write(home/'state/ferestre/library.json',json.dumps({'count':2,'items':[{'productId':id,'productType':'Game','status':'Active'} for id in ids]}))
 write(home/'runtime/proton','#!/bin/sh\nexit 0\n',True);write(home/'runtime/version','11.0.20261023.1\n');write(home/'client/xodus-cli','#!/bin/sh\nexit 1\n',True)
 env=dict(os.environ,HOME=str(home),XDG_CONFIG_HOME=str(home/'config'),XDG_STATE_HOME=str(home/'state'),XDG_CACHE_HOME=str(home/'cache'),XDG_DATA_HOME=str(home/'data'),FERESTRE_TITLES=str(home/'recipes'),XODUS_CLI_DIR=str(home/'client'),XODUS_GAMES_DIR=str(home/'games'),XODUS_STEAM_DIR=str(home/'steam'),XODUS_PROTON_DIR=str(home/'runtime'),GTK_A11Y='atspi',FERESTRE_APPLICATION_ID='io.github.icex.ferestre.QueueFixture',GSK_RENDERER=os.environ.get('GSK_RENDERER','cairo'))
 for key in ['APPDIR','APPIMAGE','FERESTRE_PACKAGE_VERSION','FERESTRE_SCRIPTS_DIR']:env.pop(key,None)
 for key in ['HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','http_proxy','https_proxy','all_proxy']:env[key]='http://127.0.0.1:9'
 env['NO_PROXY']=env['no_proxy']=''
 process=None;log=home/'gui.log'
 def start():
  with log.open('ab') as out:p=subprocess.Popen([local],env=env,stdout=out,stderr=out)
  try:
   app=s.find_app(p.pid,time.monotonic()+s.TIMEOUT);frame=s.wait_for(app,lambda root:next(iter(s.find_all(root,role='frame')),None),'queue window',time.monotonic()+s.TIMEOUT)
   return p,app,frame
  except Exception:
   print('Queue GUI startup exit status:',p.poll(),flush=True)
   print(log.read_text(),flush=True)
   if p.poll() is None:p.terminate();p.wait(timeout=5)
   raise
 def calls():return (home/'calls').read_text().splitlines() if (home/'calls').exists() else []
 def events():return (home/'events').read_text().splitlines() if (home/'events').exists() else []
 def queue():return json.loads((home/'state/ferestre/install-queue.json').read_text())['jobs']
 try:
  process,app,frame=start();s.select_in_list(frame,'My Library')
  for name in names:
   button=s.wait_for(frame,lambda root:s.row_button(root,name,'Install'),'Install '+name,time.monotonic()+s.TIMEOUT);s.click(button)
   s.wait_for(frame,lambda root:any('Install '+name+'?' in t for t in s.texts(root)),'install confirmation',time.monotonic()+s.TIMEOUT)
   s.click_named(frame,'Install')
   wait(lambda:len(queue())==names.index(name)+1,'saved queue entry')
  wait(lambda:calls()==[ids[0]],'only the first queued game to start')
  s.check(calls()==[ids[0]],'a second install queues instead of running concurrently')
  pause=s.wait_for(frame,lambda root:next((n for n in s.find_all(root,role='button',name='Pause') if n.get_state_set().contains(s.Atspi.StateType.SHOWING)),None),'Pause control',time.monotonic()+s.TIMEOUT);s.click(pause)
  wait(lambda:'pause '+ids[0] in events(),'pause byte');s.check(queue()[0]['status']=='paused','Pause is saved persistently')
  s.click_named(frame,'Resume');wait(lambda:'resume '+ids[0] in events(),'resume byte')
  s.click_named(frame,'Stop Download');wait(lambda:'stop '+ids[0] in events(),'cooperative stop')
  # A stopped entry remains paused; the next explicitly queued job starts.
  wait(lambda:ids[1] in calls(),'next queued transfer')
  s.click_named(frame,'Stop Download');wait(lambda:'stop '+ids[1] in events(),'second stop')
  wait(lambda:all(j['status']=='paused' for j in queue()),'paused queue')
  s.click_named(frame,'Close');s.check(process.wait(timeout=10)==0,'stopped transfers allow the window to close')
  previous=calls();process,app,frame=start();time.sleep(.5)
  s.check(calls()==previous,'a restart never silently starts saved downloads')
  s.select_in_list(frame,'My Library');s.click_named(frame,'Install queue');s.wait_for(frame,lambda root:'Queue One' in s.texts(root),'saved queue',time.monotonic()+s.TIMEOUT)
  resumes=s.wait_for(frame,lambda root:(buttons if len(buttons:=s.find_all(root,role='button',name='Resume'))==2 else None),'both saved Resume controls',time.monotonic()+s.TIMEOUT);s.check(len(resumes)==2,'both paused jobs can be resumed after restart')
  s.click(resumes[0]);wait(lambda:len(calls())==len(previous)+1,'restarted queue item')
  (home/('finish-'+ids[0])).touch();wait(lambda:len(queue())==1,'finished entry removed')
  # The other paused job stays paused, preserving the user's decision.
  time.sleep(.3);s.check(calls()[-1]==ids[0] and queue()[0]['status']=='paused','completion does not resume an explicitly paused job')
  s.click_named(frame,'Close');s.check(process.wait(timeout=10)==0,'queue fixture closes cleanly')
  s.check('panicked at' not in log.read_text(),'queue interactions did not panic')
  print('all persistent queue/pause UI checks passed',flush=True)
 finally:
  if process is not None and process.poll() is None:
   process.terminate()
   try:process.wait(timeout=5)
   except subprocess.TimeoutExpired:process.kill();process.wait()
  if args.keep:print('kept',home,flush=True)
  else:shutil.rmtree(home)

if __name__=='__main__':main()

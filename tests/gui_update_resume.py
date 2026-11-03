#!/usr/bin/env python3
"""Resume an existing paused update from Updates without an install dialog."""
import argparse,json,os,shutil,subprocess,tempfile,time
from pathlib import Path
import gui_smoke as s
from gui_launch import prepare
from gui_transfers import CLI
from gui_updater import write

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/debug/ferestre-gui');args=parser.parse_args()
 s.Atspi.init();process=app=None
 with tempfile.TemporaryDirectory(prefix='ferestre-update-resume-') as tmp:
  home=Path(tmp);ids,_,pad,env=prepare(home,args.binary)
  client=CLI.replace("id=sys.argv[2]", "id=sys.argv[2]\nwith (h/'arguments').open('a') as out:out.write(json.dumps(sys.argv[1:])+'\\n')")
  write(home/'bin/ferestre',client,True)
  jobs=[]
  for n,id in enumerate(ids[:2]):
   path=home/f'custom-library/game-{n}';path.mkdir(parents=True)
   write(path/'appxmanifest.xml','<Package><Identity Name="Resume.Fixture" Version="1.0.0.0" /></Package>')
   write(path/'.ferestre-update-incomplete-v1','pending')
   write(path/'keep-existing-data',f'checkpoint {n}')
   record={'product_id':id,'dir':str(path),'content_ids':['fixture-content'],'package_version':'1.0.0.0'}
   write(home/f'state/ferestre/installed/{id}.json',json.dumps(record))
   write(home/f'state/ferestre/updates/{id}.json',json.dumps({'schema':1,'record':record,'available':'2.0.0.0'}))
   jobs.append({'product_id':id,'name':f'Launch Game {n}','destination':str(path),'updating':True,'status':'paused'})
  write(home/'state/ferestre/install-queue.json',json.dumps({'schema':1,'jobs':jobs}))
  try:
   with (home/'gui.log').open('wb') as log:process=subprocess.Popen([home/'bin/ferestre-gui'],env=env,stdout=log,stderr=log)
   app=s.find_app(process.pid,time.monotonic()+20)
   def wait(test,what):return s.wait_for(app,test,what,time.monotonic()+20)
   frame=wait(lambda root:next(iter(s.find_all(root,role='frame')),None),'window')
   time.sleep(.5);s.select_in_list(frame,'Updates')
   button=wait(lambda root:s.row_button(root,'Launch Game 0','Resume'),'paused update Resume on grid')
   s.click(button)
   wait(lambda _: (home/'arguments').exists(),'saved download restarts directly')
   s.check(not s.find_all(app,role='button',name='Choose install folder'),'Resume opens no install confirmation')
   arguments=json.loads((home/'arguments').read_text().splitlines()[0])
   s.check(arguments==['install',ids[0],jobs[0]['destination']],'Resume keeps the exact saved destination')
   queue=json.loads((home/'state/ferestre/install-queue.json').read_text())['jobs']
   s.check(len(queue)==2 and queue[0]['updating'] and queue[1]['status']=='paused','Resume retains update kind and the other paused job')
   s.check((Path(jobs[0]['destination'])/'keep-existing-data').read_text()=='checkpoint 0','existing downloaded data is preserved')
   s.click_named(frame,'Stop Download')
   wait(lambda _: (home/'events').exists() and 'stop '+ids[0] in (home/'events').read_text(),'resumed transfer stops cooperatively')
   s.click_named(frame,'Close');s.check(process.wait(timeout=10)==0,'update-resume fixture closes cleanly')
   print('PASS: Updates Resume continues the saved update without reinstall confirmation',flush=True)
  except Exception:
   print((home/'gui.log').read_text(),flush=True)
   if app is not None:s.dump(app,'update resume failure')
   raise
  finally:
   pad()
   if process is not None and process.poll() is None:process.terminate();process.wait(timeout=5)
if __name__=='__main__':main()

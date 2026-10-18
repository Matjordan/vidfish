#!/usr/bin/env python3
"""Optional Linux native check with real pointer drag; not a hardware benchmark."""
import os,json,time,subprocess,tempfile,importlib.util,sys
from pathlib import Path
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('operator',str(Path(__file__).with_name('operator-smoke.py')));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
import argparse
parser=argparse.ArgumentParser(description='Optional Linux native eight-view check')
parser.add_argument('--binary',required=True);parser.add_argument('--fixture',required=True)
args=parser.parse_args();binary=str(Path(args.binary).resolve());fixture_binary=str(Path(args.fixture).resolve())
with tempfile.TemporaryDirectory(prefix='vidfish-eight-') as folder:
 root=Path(folder);config=root/'config.json';probe=root/'probe.json';log=(root/'run.log').open('w');app=None
 config.write_text(json.dumps({'output_directory':str(root/'media'),'auto_record':True,'preview_slots':8,'max_cameras':16}))
 fixture=subprocess.Popen([fixture_binary,'--small'],stdout=subprocess.PIPE,stderr=log,text=True)
 try:
  uri=fixture.stdout.readline().strip();ids=[]
  for i in range(9):
   r=subprocess.run([binary,'cameras','--inventory',str(root/'cameras.json'),'add','--alias',('A hidden camera' if i==8 else f'Camera {i+1:02}'),'--rtsp',uri],check=True,capture_output=True,text=True);ids.append(r.stdout.strip())
  (root/'views.json').write_text(json.dumps({'slots':ids[:8],'selected_slot':0}))
  app=subprocess.Popen([binary,'--no-update-check','--config',str(config)],env=dict(os.environ,VIDFISH_UI_PROBE=str(probe),VIDFISH_UI_SIZE='1280x800'),stdout=log,stderr=log)
  def state():
   try:return json.loads(probe.read_text())
   except:return {}
  mouse=m.Mouse();mouse.pid=app.pid
  def click(name):
   def find():
    s=state();b=next((b for b in s.get('buttons',[]) if b[0]==name),None);return (s,b) if b else None
   s,b=m.until(find);r=b[1];mouse.click((r[0]+r[2])/2*s['scale'],(r[1]+r[3])/2*s['scale']);time.sleep(.5)
  m.until(lambda:len(state().get('slots',[]))==8 and all(s['frame']>0 for s in state()['slots']))
  m.until(lambda:len(list((root/'acquisition').glob('*.json')))==9)
  runs={p.name:json.loads(p.read_text())['run_id'] for p in (root/'acquisition').glob('*.json')}
  before=[s['camera'] for s in state()['slots']]
  click('Max');m.until(lambda:state().get('fullscreen') is not None)
  click('Return to grid');m.until(lambda:state().get('fullscreen') is None)
  assert before==[s['camera'] for s in state()['slots']]
  click('Fullscreen');m.until(lambda:state().get('fullscreen') is not None)
  m.until(lambda:any(slot['camera']==ids[8] for slot in state()['slots']))
  click('Return to grid');m.until(lambda:before==[slot['camera'] for slot in state()['slots']])
  # Real native drag from sidebar onto an occupied viewport.
  value=state();start=next(b[1] for b in value['buttons'] if b[0]=='Camera A hidden camera');end=next(b[1] for b in value['buttons'] if b[0]=='Viewport 1')
  sx,sy=(start[0]+start[2])/2,(start[1]+start[3])/2;ex,ey=(end[0]+end[2])/2,(end[1]+end[3])/2
  mouse.test.XTestFakeMotionEvent(mouse.display,-1,int(sx),int(sy),0);mouse.test.XTestFakeButtonEvent(mouse.display,1,1,0);mouse.api.XFlush(mouse.display);time.sleep(.2)
  for n in range(1,15):
   mouse.test.XTestFakeMotionEvent(mouse.display,-1,int(sx+(ex-sx)*n/14),int(sy+(ey-sy)*n/14),0);mouse.api.XFlush(mouse.display);time.sleep(.04)
  mouse.test.XTestFakeButtonEvent(mouse.display,1,0,0);mouse.api.XFlush(mouse.display)
  m.until(lambda:state()['slots'][0]['camera']==ids[8])
  click('×');m.until(lambda:sum(s['camera'] is not None for s in state()['slots'])==7)
  assert all(json.loads(p.read_text())['run_id']==runs[p.name] for p in (root/'acquisition').glob('*.json'))
  assert state()['active_workers']<=8
  print('PASS: eight decoded views, fullscreen ordering, remove without recording restart, nine recorders')
 except:
  print((root/'run.log').read_text()[-5000:]);raise
 finally:
  for p in (root/'acquisition').glob('*.json'):
   try:p.with_suffix('.stop').write_text(json.loads(p.read_text())['run_id'])
   except:pass
  if app:app.terminate();app.wait(timeout=10)
  time.sleep(1);fixture.terminate();fixture.wait(timeout=5)

#!/usr/bin/env python3
"""Optional Linux native check with real pointer drag; not a hardware benchmark."""
import os,json,time,subprocess,tempfile,importlib.util,sys
from pathlib import Path
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('operator',str(Path(__file__).with_name('operator-smoke.py')));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
import argparse
parser=argparse.ArgumentParser(description='Optional Linux native eight-view check')
parser.add_argument('--binary',required=True);parser.add_argument('--fixture',required=True)
parser.add_argument('--drag-screenshot')
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
  # Change the temporary fullscreen composition, then restore all borrowed views.
  click('Show this group')
  m.until(lambda:all(not slot['pending'] and slot['frame']>0 for slot in state()['slots']))
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
  def body_drag_to_add():
   value=state();card=next(b[1] for b in value['buttons'] if b[0]=='Camera card A hidden camera');target=next(b[1] for b in value['buttons'] if b[0]=='Add view drop area')
   # Start on the thumbnail/status body, well below the alias.
   sx,sy=card[0]+24,card[1]+125;ex,ey=(target[0]+target[2])/2,(target[1]+target[3])/2
   mouse.test.XTestFakeMotionEvent(mouse.display,-1,int(sx),int(sy),0);mouse.test.XTestFakeButtonEvent(mouse.display,1,1,0);mouse.api.XFlush(mouse.display);time.sleep(.2)
   for n in range(1,15):
    mouse.test.XTestFakeMotionEvent(mouse.display,-1,int(sx+(ex-sx)*n/14),int(sy+(ey-sy)*n/14),0);mouse.api.XFlush(mouse.display);time.sleep(.04)
   m.until(lambda:state().get('dragging_camera'),seconds=5)
   if args.drag_screenshot:
    from PIL import ImageGrab
    ImageGrab.grab().save(args.drag_screenshot)
   mouse.test.XTestFakeButtonEvent(mouse.display,1,0,0);mouse.api.XFlush(mouse.display)
  def layout(count):
   value=state();rects=[b[1] for b in value.get('buttons',[]) if b[0].startswith('Viewport ')]
   if len(rects)!=count:return False
   for i,a in enumerate(rects):
    assert a[2]>a[0] and a[3]>a[1]
    for b in rects[i+1:]:assert min(a[2],b[2])-max(a[0],b[0])<=1 or min(a[3],b[3])-max(a[1],b[1])<=1
   # Each row spans the same workspace width, including incomplete final rows.
   rows={}
   for rect in rects:rows.setdefault(round(rect[1]),[]).append(rect)
   spans=[(min(r[0] for r in row),max(r[2] for r in row)) for row in rows.values()]
   assert all(abs(a-spans[0][0])<1 and abs(b-spans[0][1])<1 for a,b in spans)
   return True
  body_drag_to_add();m.until(lambda:sum(s['camera'] is not None for s in state()['slots'])==8)
  m.until(lambda:layout(8))
  for count in range(7,0,-1):
   click('×');m.until(lambda:sum(s['camera'] is not None for s in state()['slots'])==count)
   m.until(lambda:layout(count))
  body_drag_to_add();m.until(lambda:sum(s['camera'] is not None for s in state()['slots'])==2)
  m.until(lambda:layout(2))
  assert all(json.loads(p.read_text())['run_id']==runs[p.name] for p in (root/'acquisition').glob('*.json'))
  assert state()['active_workers']<=8
  print('PASS: whole-card drag with floating feedback, add/drop, automatic 1–8 tiling, fullscreen ordering, unchanged nine recorder runs')
 except:
  print((root/'run.log').read_text()[-5000:]);raise
 finally:
  for p in (root/'acquisition').glob('*.json'):
   try:p.with_suffix('.stop').write_text(json.loads(p.read_text())['run_id'])
   except:pass
  if app:app.terminate();app.wait(timeout=10)
  time.sleep(1);fixture.terminate();fixture.wait(timeout=5)

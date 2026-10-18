#!/usr/bin/env python3
"""Optional local native check; never run automatically in build-only CI."""
import os,json,time,subprocess,tempfile,importlib.util,sys
from pathlib import Path
from PIL import ImageGrab
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('operator',str(Path(__file__).with_name('operator-smoke.py')));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
import argparse
parser=argparse.ArgumentParser(description='Optional native UI check for operator refresh')
parser.add_argument('--binary',required=True);parser.add_argument('--fixture',required=True)
parser.add_argument('--screenshot');parser.add_argument('--size',default='1280x800')
args=parser.parse_args()
binary=str(Path(args.binary).resolve());fixture_binary=str(Path(args.fixture).resolve())
with tempfile.TemporaryDirectory(prefix='vidfish-events-') as folder:
 root=Path(folder);config=root/'config.json';probe=root/'probe.json';log=(root/'run.log').open('w');app=None
 config.write_text(json.dumps({'output_directory':str(root/'media'),'auto_record':True,'preview_slots':4,'max_cameras':16,'session_name':'Bench acquisition'}))
 fixture=subprocess.Popen([fixture_binary,'--small'],stdout=subprocess.PIPE,stderr=log,text=True)
 try:
  uri=fixture.stdout.readline().strip();ids=[]
  for label in ['Overview','Instrument bay','Port side','Science camera','Hidden camera']:
   r=subprocess.run([binary,'cameras','--inventory',str(root/'cameras.json'),'add','--alias',label,'--rtsp',uri],check=True,capture_output=True,text=True);ids.append(r.stdout.strip())
  (root/'views.json').write_text(json.dumps({'slots':ids[:4],'selected_slot':0}))
  app=subprocess.Popen([binary,'--no-update-check','--config',str(config)],env=dict(os.environ,VIDFISH_UI_PROBE=str(probe),VIDFISH_UI_SIZE=args.size),stdout=log,stderr=log)
  def state():
   try:return json.loads(probe.read_text())
   except:return {}
  mouse=m.Mouse();mouse.pid=app.pid
  def click(name):
   def find():
    s=state();b=next((b for b in s.get('buttons',[]) if b[0]==name),None);return (s,b) if b else None
   try: s,b=m.until(find,seconds=3)
   except RuntimeError:
    mouse.scroll(4,400,350);s,b=m.until(find,seconds=5)
   time.sleep(.7)
   current=state();b=next((v for v in current.get('buttons',[]) if v[0]==name),b);s=current
   r=b[1];mouse.click((r[0]+r[2])/2*s['scale'],(r[1]+r[3])/2*s['scale']);time.sleep(.5)
  def events():
   return [json.loads(p.read_text()) for p in (root/'media').glob('*/events/*/event.json')]
  m.until(lambda:len(state().get('slots',[]))==4 and all(s['frame']>0 for s in state()['slots']))
  m.until(lambda:state().get('roster_thumbnails',{}).get('cached',0)==5)
  click('Mark');m.until(lambda:len(events())==1)
  click('Show events');m.until(lambda:state().get('event_browser',{}).get('selected'))
  click('Edit note');click('Event note field');mouse.type_note('sample collected');click('Save note')
  m.until(lambda:events()[0]['note']=='sample collected')
  click('Event presets…');click('Add preset');click('Save event presets')
  m.until(lambda:any('New event' in json.loads(p.read_text()).get('event_presets',[]) for p in (root/'media').glob('*/manifest.json')))
  mouse.scroll(-20,400,250);time.sleep(.5);click('Close library')
  m.until(lambda:any(b[0]=='New event' for b in state().get('buttons',[])))
  click('New event');m.until(lambda:len(events())==2)
  assert any(e['note']=='New event' for e in events())
  click('Mark + Grab All');m.until(lambda:len(events())==3 and all(e['complete'] for e in events()))
  capture=next(e for e in events() if e['kind']=='Mark + Grab All')
  assert sum(c['image'] is not None for c in capture['captures'])==4
  assert all('Displayed frame' in c['result'] for c in capture['captures'] if c['image'])
  # Explicit user scrolling to newest keeps the documentation capture readable.
  buttons=[b for b in state().get('buttons',[]) if b[0].startswith('Event ') and b[0]!='Event presets…']
  if buttons:
   rect=buttons[-1][1];mouse.scroll(-30,int((rect[0]+rect[2])/2),min(int((rect[1]+rect[3])/2),int(state()['viewport'][1])-25))
  time.sleep(1)
  if args.screenshot: ImageGrab.grab().save(args.screenshot)
  click('Search events');mouse.type_note('sample');time.sleep(1)
  assert len([b for b in state().get('buttons',[]) if b[0].startswith('Event ') and b[0]!='Event presets…'])==1
  click('View snapshots');m.until(lambda:state().get('event_browser',{}).get('snapshots'))
  print('PASS: hidden thumbnail, persistent event details, note editing, session preset creation and mark, displayed-frame captures, separate snapshots')
 except:
  print((root/'run.log').read_text()[-5000:]);
  if probe.exists():print(probe.read_text())
  if args.screenshot: ImageGrab.grab().save(str(Path(args.screenshot).with_suffix('.failure.png')))
  raise
 finally:
  for p in (root/'acquisition').glob('*.json'):
   try:p.with_suffix('.stop').write_text(json.loads(p.read_text())['run_id'])
   except:pass
  if app:app.terminate();app.wait(timeout=10)
  time.sleep(1);fixture.terminate();fixture.wait(timeout=5)

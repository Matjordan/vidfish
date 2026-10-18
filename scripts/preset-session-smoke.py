#!/usr/bin/env python3
"""Optional native regression: fresh session -> presets, without cameras/recording."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('operator', Path(__file__).with_name('operator-smoke.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
parser = argparse.ArgumentParser()
parser.add_argument('--binary', required=True)
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='vidfish-empty-presets-') as folder:
    root = Path(folder)
    config, probe = root/'config.json', root/'probe.json'
    config.write_text(json.dumps({'output_directory':str(root/'media'), 'auto_record':False, 'camera_interface':'127.0.0.1'}))
    log = (root/'run.log').open('w')
    app = subprocess.Popen([str(Path(args.binary).resolve()), '--no-update-check', '--config', str(config)], env=dict(os.environ, VIDFISH_UI_PROBE=str(probe), VIDFISH_UI_SIZE='1280x800'), stdout=log, stderr=log)
    try:
        mouse = m.Mouse()
        mouse.pid = app.pid
        def state():
            try: return json.loads(probe.read_text())
            except (OSError, ValueError): return {}
        def click(name):
            def find():
                s = state()
                b = next((b for b in s.get('buttons',[]) if b[0]==name),None)
                return (s,b[1]) if b else None
            value, rect = m.until(find, seconds=10)
            mouse.click((rect[0]+rect[2])/2*value['scale'], (rect[1]+rect[3])/2*value['scale'])
            time.sleep(.7)
        m.until(lambda:state().get('viewport'))
        assert state().get('session_id') is None
        click('Sessions / recordings')
        click('Start new session')
        first = m.until(lambda:state().get('session_id'))
        assert json.loads((root/'sessions.json').read_text())['active']==first
        click('Add preset')
        click('Save event presets')
        m.until(lambda:any(json.loads(p.read_text()).get('event_presets')==['New event'] for p in (root/'media').glob('*/manifest.json')))
        mouse.scroll(-20,400,250)
        click('Close library')
        click('New event')
        m.until(lambda:any(json.loads(p.read_text()).get('note')=='New event' for p in (root/'media').glob('*/events/*/event.json')))
        click('Sessions / recordings')
        mouse.scroll(-20,400,250)
        click('Start new session')
        second = m.until(lambda:value if (value:=state().get('session_id')) and value!=first else None)
        assert json.loads((root/'sessions.json').read_text())['active']==second
        click('Add preset')
        assert all(not json.loads(p.read_text()).get('state')=='Recording' for p in (root/'acquisition').glob('*.json'))
        print('PASS: empty session creation exposes presets, saves/uses a preset, and switches to a new session without recording')
    except Exception:
        print((root/'run.log').read_text()[-5000:])
        print(state())
        raise
    finally:
        app.terminate()
        app.wait(timeout=10)
        log.close()

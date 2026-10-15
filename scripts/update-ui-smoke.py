#!/usr/bin/env python3
"""Native update prompt/Later/manual checks and version-confirmed restart under real media locks."""
import argparse,importlib.util,json,os,pathlib,signal,subprocess,sys,tempfile,time,shutil
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('operator',pathlib.Path(__file__).with_name('operator-smoke.py'));operator=importlib.util.module_from_spec(spec);spec.loader.exec_module(operator)

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--bundle',required=True);ap.add_argument('--fixture',required=True);args=ap.parse_args()
    bundle=pathlib.Path(args.bundle).resolve();fixture_binary=pathlib.Path(args.fixture).resolve();name='vidfish.exe' if os.name=='nt' else 'vidfish';helper=bundle/('vidfish-maintenance.exe' if os.name=='nt' else 'vidfish-maintenance');version=json.loads((bundle/'bundle.json').read_text())['version']
    # Verify the Windows default renderer; Linux also covers preserving an explicit override.
    renderer_args=[] if os.name=='nt' else ['--opengl']
    with tempfile.TemporaryDirectory(prefix='vidfish-update-ui-') as folder:
        base=pathlib.Path(folder);prefix=base/'prefix';root=prefix/'lib/vidfish';config=base/'config.json';probe=base/'ui.json';release=base/'release.json';log=(base/'smoke.log').open('w');fixture=None;app=None;restarted_pid=None
        subprocess.run([str(helper),'install','--prefix',str(prefix),'--bundle',str(bundle)],check=True,stdout=log,stderr=log)
        binary=root/'versions'/version/name
        config.write_text(json.dumps(dict(output_directory=str(base/'media'),auto_record=True,segment_seconds=2)))
        (base/'updates.json').write_text(json.dumps(dict(schema_version=1,automatic_checks=False)))
        asset=f'vidfish-999.0.0-{"windows" if os.name=="nt" else "linux"}-x86_64-update.zip';url='https://github.com/Matjordan/vidfish/releases/download/v999.0.0/'
        release.write_text(json.dumps(dict(tag_name='v999.0.0',draft=False,prerelease=False,body='Synthetic release used for UI validation. No download is started.',assets=[dict(name=asset,size=100,browser_download_url=url+asset),dict(name='SHA256SUMS',size=100,browser_download_url=url+'SHA256SUMS')])))
        environment=dict(os.environ,VIDFISH_UI_PROBE=str(probe),VIDFISH_UPDATE_PROBE_RELEASE=str(release),VIDFISH_UI_SIZE=operator.native_ui_size('1280x800'))
        mouse=operator.Mouse()
        def state():
            try:return json.loads(probe.read_text())
            except (OSError,ValueError):return {}
        def statuses():return [json.loads(p.read_text()) for p in (base/'acquisition').glob('*.json')]
        def click(label):
            operator.until(lambda:any(c[0]==label for c in state().get('controls',[])))
            for _ in range(25):
                control=next((c for c in state().get('controls',[]) if c[0]==label),None)
                if control:
                    rect,clip=control[1:];x=(max(rect[0],clip[0])+min(rect[2],clip[2]))/2;y=(max(rect[1],clip[1])+min(rect[3],clip[3]))/2
                    if min(rect[2],clip[2])-max(rect[0],clip[0])>8 and min(rect[3],clip[3])-max(rect[1],clip[1])>8:mouse.click(x,y);time.sleep(.7);return
                    mouse.scroll(-3 if rect[1]<clip[1] else 3,(clip[0]+clip[2])/2,(clip[1]+clip[3])/2);time.sleep(.7)
            raise RuntimeError('Update control unavailable: '+label)
        try:
            fixture=subprocess.Popen([str(fixture_binary),'--small'],stdout=subprocess.PIPE,stderr=log,text=True);uri=fixture.stdout.readline().strip();assert uri.startswith('rtsp://')
            for i in range(4):subprocess.run([str(binary),'cameras','--inventory',str(base/'cameras.json'),'add','--alias',f'Stream {i+1}','--rtsp',uri],check=True,stdout=log,stderr=log)
            app=subprocess.Popen([str(binary),'--config',str(config),'--no-update-check',*renderer_args],env=environment,stdout=log,stderr=log);mouse.pid=app.pid
            operator.until(lambda:len(statuses())==4 and all(s['state']=='Recording' and s['bytes_written']>0 for s in statuses()))
            before={s['camera_id']:(s['run_id'],s['bytes_written']) for s in statuses()}
            operator.until(lambda:state().get('updates'));assert not state()['updates']['open'] and not state()['updates']['checking']
            click('Settings');click('Check for updates');operator.until(lambda:state().get('updates',{}).get('version')=='999.0.0')
            operator.until(lambda:any(c[0]=='Download and defer until safe' for c in state().get('controls',[])))
            click('Later');operator.until(lambda:not state()['updates']['open']);assert not state()['updates']['downloading'] and not state()['updates']['deferred'];assert not (root/'staging').exists()
            operator.until(lambda:all(s['state']=='Recording' and s['run_id']==before[s['camera_id']][0] and s['bytes_written']>before[s['camera_id']][1] for s in statuses()))
            # The desktop's death must not release the independent recorder guard.
            app.kill();app.wait(timeout=10);app=None
            attempt=subprocess.run([str(helper),'apply','--root',str(root),'--bundle',str(bundle)],stdout=log,stderr=log,timeout=60);assert attempt.returncode!=0
            assert all(s['state']=='Recording' and s['run_id']==before[s['camera_id']][0] for s in statuses())
            for p in (base/'acquisition').glob('*.json'):p.with_suffix('.stop').write_text(json.loads(p.read_text())['run_id'])
            operator.until(lambda:all(s['state'] in ('Stopped','Failed') for s in statuses()));time.sleep(1)
            # Kill the actual helper after its durable journal is written, before UI launch.
            crashing=subprocess.Popen([str(helper),'apply','--root',str(root),'--bundle',str(bundle),'--relaunch',*renderer_args,'--config',str(config)],env=environment,stdout=log,stderr=log)
            deadline=time.monotonic()+40
            while time.monotonic()<deadline and not (root/'transaction.json').exists():
                if crashing.poll() is not None:raise RuntimeError('Helper ended before crash injection')
                time.sleep(.01)
            assert (root/'transaction.json').exists();crashing.kill();crashing.wait(timeout=10)
            subprocess.run([str(helper),'recover','--root',str(root)],check=True,stdout=log,stderr=log,timeout=60)
            assert not (root/'transaction.json').exists() and (root/'current.txt').read_text()==version
            # Version-confirmed actual desktop restart, without changing stored auto-record.
            saved=json.loads(config.read_text());probe.unlink(missing_ok=True)
            result=subprocess.run([str(helper),'apply','--root',str(root),'--bundle',str(bundle),'--relaunch',*renderer_args,'--config',str(config)],env=environment,stdout=log,stderr=log,timeout=90)
            assert result.returncode==0
            health=json.loads((root/'health.json').read_text());assert health['version']==version and health['desktop_startup_confirmed']
            operator.until(lambda:state().get('process_id') and not state()['updates']['startup_probation']);restarted_pid=state()['process_id'];assert json.loads(config.read_text())==saved;assert state()['opengl_override']==bool(renderer_args)
            operator.until(lambda:len(statuses())==4 and all(s['state']=='Recording' and s['run_id']!=before[s['camera_id']][0] and s['bytes_written']>0 for s in statuses()))
            os.kill(restarted_pid,signal.SIGTERM);restarted_pid=None
            for p in (base/'acquisition').glob('*.json'):p.with_suffix('.stop').write_text(json.loads(p.read_text())['run_id'])
            operator.until(lambda:all(s['state'] in ('Stopped','Failed') for s in statuses()));time.sleep(1)
            saved['auto_record']=False;config.write_text(json.dumps(saved))
            # A replacement UI that cannot start rolls back without touching operator data.
            failed_environment=dict(environment,DISPLAY=':9999') if os.name!='nt' else None
            if failed_environment:
                result=subprocess.run([str(helper),'apply','--root',str(root),'--bundle',str(bundle),'--relaunch',*renderer_args,'--config',str(config)],env=failed_environment,stdout=log,stderr=log,timeout=90);assert result.returncode!=0;assert not (root/'transaction.json').exists();assert (root/'current.txt').read_text()==version
            print('PASS: manual checks with auto disabled, real native offer/Later, four unchanged growing recorders, post-UI-exit guard, forced helper-crash recovery, confirmed desktop restart with restored recording policy and failed-desktop rollback')
        finally:
            if sys.exc_info()[0] is not None:
                log.flush();diagnostic=str(sys.exc_info()[1])+'\n'+(base/'smoke.log').read_text()[-7000:]+'\nProbe: '+json.dumps(state());print(diagnostic);print('::error title=Native update smoke failed::'+diagnostic.replace('%','%25').replace('\r','%0D').replace('\n','%0A'))
            if app and app.poll() is None:app.kill();app.wait(timeout=10)
            if restarted_pid:
                try:os.kill(restarted_pid,signal.SIGTERM)
                except ProcessLookupError:pass
            for p in (base/'acquisition').glob('*.json'):p.with_suffix('.stop').write_text(json.loads(p.read_text())['run_id'])
            if fixture:
                end=time.monotonic()+20
                while time.monotonic()<end and any(s['state'] in ('Starting','Recording','Stopping') for s in statuses()):time.sleep(.2)
                time.sleep(1);fixture.terminate();fixture.wait(timeout=10)
            log.close()
if __name__=='__main__':main()

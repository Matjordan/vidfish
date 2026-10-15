#!/usr/bin/env python3
"""Real bundle install/reinstall, coherent updates, rollback, crash recovery and data retention.
Windows execution is gated by windows_tests, as requested by the operator.
"""
import argparse, hashlib, json, os, pathlib, shutil, subprocess, tempfile, time

def run(*args, ok=True, **kw):
    p=subprocess.run([str(a) for a in args],capture_output=True,text=True,timeout=90,**kw)
    if (p.returncode==0)!=ok: raise RuntimeError(f'{args}: expected success={ok}, code={p.returncode}\n{p.stdout}\n{p.stderr}')
    return p

def until(predicate,seconds=40):
    end=time.monotonic()+seconds
    while time.monotonic()<end:
        value=predicate()
        if value:return value
        time.sleep(.1)
    raise RuntimeError('Installer test did not reach expected state')

def manifest(bundle,version):
    path=bundle/'bundle.json'; doc=json.loads(path.read_text());doc['version']=version
    doc['files']={str(p.relative_to(bundle)).replace('\\','/'):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(bundle.rglob('*')) if p.is_file() and p.name!='bundle.json'}
    path.write_text(json.dumps(doc))

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--bundle',required=True);ap.add_argument('--fixture',required=True);ap.add_argument('--native-installer');ap.add_argument('--previous-binary');args=ap.parse_args()
    bundle=pathlib.Path(args.bundle).resolve();fixture=pathlib.Path(args.fixture).resolve();exe='vidfish.exe' if os.name=='nt' else 'vidfish';maintenance=bundle/('vidfish-maintenance.exe' if os.name=='nt' else 'vidfish-maintenance');v=json.loads((bundle/'bundle.json').read_text())['version']
    with tempfile.TemporaryDirectory(prefix='vidfish-install-') as folder:
        base=pathlib.Path(folder);prefix=base/'prefix with spaces';root=prefix/'lib/vidfish';settings=base/'settings';settings.mkdir();config=settings/'config.json';media=base/'scientific-data';media.mkdir();config.write_text(json.dumps(dict(output_directory=str(media),auto_record=False,segment_seconds=2)))
        for name in ['cameras.credentials.json','views.json','events.json']:(settings/name).write_text('operator data\n')
        (media/'existing.mkv').write_bytes(b'precious scientific bytes');before={str(p):p.read_bytes() for p in [config,*settings.glob('*json'),media/'existing.mkv']}
        if args.previous_binary:
            previous=pathlib.Path(args.previous_binary).resolve();old=base/'previous-bundle';shutil.copytree(bundle,old);shutil.copy2(previous,old/exe);old_version=run(previous,'--version').stdout.strip().split()[-1];manifest(old,old_version);run(maintenance,'install','--prefix',prefix,'--bundle',old);assert (root/'current.txt').read_text()==old_version
        run(maintenance,'install','--prefix',prefix,'--bundle',bundle)
        installed=root/'versions'/v/exe
        assert run(installed,'--version').stdout.strip()==f'vidfish {v}'
        assert (prefix/'share/icons/hicolor/256x256/apps/vidfish.png').is_file()
        if os.name!='nt':
            assert (prefix/'share/applications/vidfish.desktop').is_file();run(prefix/'bin/vidfish','--check','--config',config,'--no-update-check')
        else:run('cmd.exe','/C',prefix/'bin/vidfish.cmd','--check','--config',config,'--no-update-check')
        run(maintenance,'install','--prefix',prefix,'--bundle',bundle)
        run(maintenance,'apply','--root',root,'--bundle',bundle)
        # A corrupt or incompatible payload must fail before the pointer is changed.
        bad=base/'bad';shutil.copytree(bundle,bad);(bad/exe).write_bytes(b'corrupt')
        run(maintenance,'apply','--root',root,'--bundle',bad,ok=False);assert (root/'current.txt').read_text()==v
        shutil.rmtree(bad);shutil.copytree(bundle,bad);doc=json.loads((bad/'bundle.json').read_text());doc['platform']='wrong-arch';(bad/'bundle.json').write_text(json.dumps(doc));run(maintenance,'apply','--root',root,'--bundle',bad,ok=False)
        # Wrong reported version passes checksums, fails post-switch health, and rolls back.
        shutil.rmtree(bad);shutil.copytree(bundle,bad);manifest(bad,'999.0.0');run(maintenance,'apply','--root',root,'--bundle',bad,ok=False);assert (root/'current.txt').read_text()==v;assert (root/'update-error.txt').exists()
        # Crash/power loss at the durable pointer transaction: launcher recovers old bundle.
        (root/'previous.txt').write_text(v);(root/'transaction.json').write_text(json.dumps(dict(previous=v,next='999.0.0')));(root/'current.txt').write_text('999.0.0')
        if os.name!='nt':run(prefix/'bin/vidfish','--check','--config',config,'--no-update-check')
        else:run('cmd.exe','/C',prefix/'bin/vidfish.cmd','--check','--config',config,'--no-update-check')
        assert (root/'current.txt').read_text()==v and not (root/'transaction.json').exists()
        # Exercise an actual independent RTSP recording, including after its owner is gone.
        log=(base/'fixture.log').open('w');server=subprocess.Popen([str(fixture),'--small'],stdout=subprocess.PIPE,stderr=log,text=True);recorder=None;recordlog=None
        try:
            uri=server.stdout.readline().strip();assert uri.startswith('rtsp://')
            target=media/'new-recording.mkv';recordlog=(base/'recording.log').open('w')
            recorder=subprocess.Popen([str(installed),'--record',uri,'--output',str(target),'--seconds','10','--config',str(config),'--no-update-check'],stdout=recordlog,stderr=recordlog)
            until(lambda:target.exists() and target.stat().st_size>0)
            size=target.stat().st_size
            run(maintenance,'apply','--root',root,'--bundle',bundle,ok=False)
            run(maintenance,'uninstall','--prefix',prefix,ok=False)
            until(lambda:target.stat().st_size>size);assert recorder.poll() is None
            assert recorder.wait(timeout=25)==0;recorder=None
            run(installed,'--inspect',target)
        finally:
            if recorder and recorder.poll() is None:recorder.terminate();recorder.wait(timeout=20)
            server.terminate();server.wait(timeout=15);log.close()
            if recordlog:recordlog.close()
        # Uninstall removes only owned files. Modified and extra data are retained.
        extra=root/'operator-note.txt';extra.write_text('retained');run(maintenance,'uninstall','--prefix',prefix);assert extra.read_text()=='retained';assert not installed.exists()
        for path,content in before.items():assert pathlib.Path(path).read_bytes()==content,path
        run(maintenance,'install','--prefix',prefix,'--bundle',bundle);run(maintenance,'uninstall','--prefix',prefix)
        print('PASS: install/reinstall, verified apply, corrupt/wrong-arch rejection, failed-health rollback, interrupted recovery, active RTSP guard and science retention')
        if args.native_installer:
            native_prefix=base/'native'
            run(pathlib.Path(args.native_installer).resolve(),'/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',f'/DIR={native_prefix}')
            assert (native_prefix/'bin/vidfish.cmd').exists()
            uninstaller=native_prefix/'unins000.exe';run(uninstaller,'/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART');assert not (native_prefix/'bin/vidfish.cmd').exists()
            print('PASS: native per-user Windows installer/uninstaller')
if __name__=='__main__':
    try:main()
    except Exception:
        import traceback
        text=traceback.format_exc().replace('%','%25').replace('\r','%0D').replace('\n','%0A')
        print('::error title=Installer smoke failed::'+text)
        raise

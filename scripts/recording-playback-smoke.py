#!/usr/bin/env python3
"""Optional Linux FFmpeg/ffprobe playback check; never part of build-only CI."""
import argparse
import json
import signal
import subprocess
import tempfile
import time
from pathlib import Path


def until(predicate):
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.1)
    raise RuntimeError("recording playback readiness timed out")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--fixture', required=True, type=Path)
    args = parser.parse_args()
    binary, fixture = str(args.binary.resolve()), str(args.fixture.resolve())
    for codec in ['h264', 'h265']:
        for container in ['mkv', 'mp4']:
            with tempfile.TemporaryDirectory(prefix='vidfish-playback-') as folder:
                root = Path(folder)
                with (root/'run.log').open('w') as log:
                    camera = subprocess.Popen([fixture, '--small'] + (['--h265'] if codec == 'h265' else []), stdout=subprocess.PIPE, stderr=log, text=True)
                    worker = None
                    try:
                        uri = camera.stdout.readline().strip()
                        config = root/'config.json'
                        config.write_text(json.dumps({'output_directory':str(root/'media'), 'segment_seconds':2, 'recording_container':container, 'disk_warning_bytes':0, 'disk_stop_bytes':0}))
                        camera_id = subprocess.check_output([binary,'cameras','--inventory',str(root/'cameras.json'),'add','--alias','Playback fixture','--rtsp',uri] + (['--h265'] if codec == 'h265' else []),text=True).strip()
                        worker = subprocess.Popen([binary,'--config',str(config),'--record-camera',camera_id], stdout=log,stderr=log)
                        def rows():
                            indices = list((root/'media').glob('*/recordings/*/segments.jsonl'))
                            if not indices:
                                return []
                            try:
                                return [json.loads(line) for line in indices[0].read_text().splitlines()]
                            except (ValueError, OSError):
                                return []
                        closed = until(lambda:next((row for row in rows() if row['event']=='closed'),None))
                        assert worker.poll() is None, 'recorder ended before independent playback'
                        def verify(path):
                            info = json.loads(subprocess.check_output([binary,'--inspect',str(path)],text=True,timeout=30))
                            probe = json.loads(subprocess.check_output(['ffprobe','-v','error','-show_entries','format=duration:stream=codec_name,pix_fmt','-of','json',str(path)],text=True,timeout=20))
                            duration = float(probe['format']['duration'])
                            assert abs(duration-info['duration_seconds']) < .15, (info,probe)
                            assert info['seekable'] and info['software_decode_and_seek'], info
                            assert probe['streams'][0]['codec_name'] == ('hevc' if codec == 'h265' else codec), probe
                            # Explicit software decode/seek using an independent media stack.
                            pixels = subprocess.check_output(['ffmpeg','-v','error','-hwaccel','none','-ss',str(duration/2),'-i',str(path),'-frames:v','1','-pix_fmt','rgb24','-f','rawvideo','-'],timeout=20)
                            assert pixels and any(r>g or b>g for r,g,b in zip(pixels[::3],pixels[1::3],pixels[2::3])), 'missing/solid-green decoded frame'
                        verify(Path(closed['location']))
                        worker.send_signal(signal.SIGTERM)
                        assert worker.wait(timeout=30) == 0, (root/'run.log').read_text()[-3000:]
                        events = rows()
                        opened = {row['location'] for row in events if row['event']=='opened'}
                        finalized = {row['location'] for row in events if row['event']=='closed'}
                        assert opened == finalized
                        for path in finalized:
                            verify(Path(path))
                        print(json.dumps({'codec':codec,'container':container,'finalized_segments':len(finalized),'software_playback_seek':'passed','completed_playback_while_recording':'passed'}),flush=True)
                    finally:
                        if worker and worker.poll() is None:
                            worker.kill()
                            worker.wait(timeout=10)
                        camera.terminate()
                        camera.wait(timeout=10)
                        camera.stdout.close()


if __name__ == '__main__':
    main()

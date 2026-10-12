"""Exercise real processes: RTSP -> MKV and preview restart/crash isolation."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--desktop", action="store_true")
    args = parser.parse_args()
    binary = str(args.binary.resolve())
    fixture_binary = str(args.fixture.resolve())
    env = {**os.environ, "RUST_LOG": "info"}
    children = []
    with tempfile.TemporaryDirectory(prefix="vidfish-smoke-") as directory:
        root = Path(directory)
        try:
            fixture_errors = (root / "fixture-errors.log").open("w")
            fixture = subprocess.Popen([fixture_binary], stdout=subprocess.PIPE, stderr=fixture_errors, text=True, env=env)
            children.append(fixture)
            uri = fixture.stdout.readline().strip()
            if not uri.startswith("rtsp://127.0.0.1:"):
                raise RuntimeError("fixture failed to start: " + (root / "fixture-errors.log").read_text())
            output = root / "camera.mkv"
            with (root / "recording.log").open("w") as logs:
                recorder = subprocess.Popen([binary, "--record", uri, "--output", str(output), "--seconds", "12"], stdout=logs, stderr=logs, env=env)
                children.append(recorder)
                if args.desktop:
                    with (root / "crash-preview.log").open("w") as preview_logs:
                        preview = subprocess.Popen([binary, "--preview", uri], stdout=preview_logs, stderr=preview_logs, env=env)
                        children.append(preview)
                        time.sleep(3)
                        if preview.poll() is not None:
                            raise RuntimeError("desktop failed to launch: " + (root / "crash-preview.log").read_text())
                        preview.kill()  # Simulate a desktop crash; the recorder must survive.
                        preview.wait(timeout=10)
                    if recorder.poll() is not None:
                        raise RuntimeError("recorder exited after desktop crash")
                    resumed = subprocess.run([binary, "--preview", uri, "--desktop-seconds", "3"], capture_output=True, text=True, env=env, timeout=40)
                    if resumed.returncode:
                        raise RuntimeError("preview restart failed: " + resumed.stdout + resumed.stderr)
                if recorder.wait(timeout=40) != 0:
                    raise RuntimeError("recording failed: " + (root / "recording.log").read_text())
            inspected = subprocess.run([binary, "--inspect", str(output)], capture_output=True, text=True, env=env, timeout=20, check=True)
            info = json.loads(inspected.stdout.splitlines()[-1])
            if info["width"] != 1920 or info["height"] != 1080 or info["duration_seconds"] < 8:
                raise RuntimeError("unexpected media timing or dimensions: " + str(info))
            if "video/x-h264" not in info["caps"]:
                raise RuntimeError("stream-copy output was not H.264")
            print(json.dumps({"recording": "passed", "desktop_crash_restart": "passed" if args.desktop else "unrun", "media": info}))
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    # Fixture has no scientific outputs; kill safely on all platforms.
                    child.kill()
                child.wait(timeout=10)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        if os.environ.get("GITHUB_ACTIONS") == "true":
            message = str(error).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
            print("::error title=Media process smoke failed::" + message, flush=True)
        raise

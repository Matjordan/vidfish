#!/usr/bin/env python3
"""Optional native check: camera -> shared recording -> viewer-only transitions."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

spec = importlib.util.spec_from_file_location("operator", Path(__file__).with_name("operator-smoke.py"))
operator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(operator)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--fixture", required=True)
    args = parser.parse_args()
    binary, fixture_binary = str(Path(args.binary).resolve()), str(Path(args.fixture).resolve())
    with tempfile.TemporaryDirectory(prefix="vidfish-shared-") as folder:
        root = Path(folder)
        config, inventory, probe = root / "config.json", root / "cameras.json", root / "probe.json"
        config.write_text(json.dumps({"output_directory": str(root / "media"), "auto_record": False, "max_cameras": 2, "preview_slots": 2, "segment_seconds": 60}))
        log = (root / "run.log").open("w")
        fixtures, app = [], None
        try:
            for number in range(2):
                fixture = subprocess.Popen([fixture_binary, "--small"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1)
                fixtures.append(fixture)
                uri = fixture.stdout.readline().strip()
                fixture.stdout.readline()
                fixture.stdout.readline()
                subprocess.run([binary, "cameras", "--inventory", str(inventory), "add", "--alias", f"Bench {number + 1}", "--rtsp", uri], check=True, stdout=log, stderr=log)
            app = subprocess.Popen([binary, "--no-update-check", "--config", str(config)], env=dict(os.environ, VIDFISH_UI_PROBE=str(probe), VIDFISH_UI_SIZE="1280x800"), stdout=log, stderr=log)
            mouse = operator.Mouse()
            mouse.pid = app.pid

            def state():
                try:
                    return json.loads(probe.read_text())
                except (OSError, ValueError):
                    return {}

            def click(name):
                def found():
                    value = state()
                    buttons = [b for b in value.get("buttons", []) if b[0] == name]
                    return (value, buttons[0][1]) if buttons else None
                value, rect = operator.until(found, seconds=15)
                mouse.click((rect[0] + rect[2]) / 2 * value["scale"], (rect[1] + rect[3]) / 2 * value["scale"])
                time.sleep(0.25)

            def ready(route):
                slots = state().get("slots", [])
                return len(slots) == 2 and all(s["frame"] > 0 and s.get("source") == route and not s["pending"] for s in slots)

            def counts():
                values = []
                for fixture in fixtures:
                    fixture.stdin.write("clients\n")
                    fixture.stdin.flush()
                    line = fixture.stdout.readline().strip()
                    if not line.startswith("CLIENTS "):
                        raise RuntimeError("Fixture connection counter unavailable")
                    values.append(int(line.split()[1]))
                return values

            def statuses():
                values = []
                for path in (root / "acquisition").glob("*.json"):
                    try:
                        values.append(json.loads(path.read_text()))
                    except (ValueError, OSError):
                        pass
                return values

            operator.until(lambda: state().get("camera_count") == 2)
            click("Show this group")
            operator.until(lambda: ready("camera"))
            if counts() != [1, 1]:
                raise RuntimeError("Initial previews opened duplicate camera sessions")
            click("Record all")
            operator.until(lambda: ready("recording"))
            original = operator.until(lambda: (values if len(values) == 2 and all(c["state"] == "Recording" for c in values) else None) if (values := statuses()) else None)
            original_runs = {c["camera_id"]: (c["run_id"], c["output"]) for c in original}
            if counts() != [2, 2]:
                raise RuntimeError("Viewing opened extra camera connections after recording started")
            click("Close")
            click("Show this group")
            operator.until(lambda: ready("recording"))
            click("Grab All")
            operator.until(lambda: state().get("events", 0) >= 1)
            def capture_complete():
                for path in (root / "media").glob("*/events/*/event.json"):
                    try:
                        event = json.loads(path.read_text())
                        if event.get("kind") == "Grab All" and event.get("complete"):
                            return len(event["captures"]) == 2 and all(c["image"] for c in event["captures"])
                    except (ValueError, OSError):
                        pass
                return False
            operator.until(capture_complete)
            if counts() != [2, 2]:
                raise RuntimeError("Reopening previews or capture opened a camera session")
            previous = [s["frame"] for s in state()["slots"]]
            fixtures[0].stdin.write("disconnect-last\n")
            fixtures[0].stdin.flush()
            operator.until(lambda: ready("recording") and all(s["frame"] > frame for s, frame in zip(state()["slots"], previous)))
            operator.until(lambda: counts() == [3, 2])
            operator.until(lambda: ready("recording") and all(s.get("decoded_frame_age_ms", 9999) < 300 for s in state()["slots"]))
            current = statuses()
            if any((c["run_id"], c["output"]) != original_runs[c["camera_id"]] for c in current):
                raise RuntimeError("Short TCP break replaced the active recording file")
            click("Stop all")
            operator.until(lambda: ready("camera"))
            operator.until(lambda: len(values) == 2 and all(c["state"] == "Stopped" for c in values) if (values := statuses()) else False)
            if counts() != [4, 3]:
                raise RuntimeError("Stopping recording did not restore one viewer-only connection")
            print(json.dumps({"cameras": 2, "recorded_viewing": "one RTSP connection per camera", "capture": "shared", "tcp_break": "same recording file", "recording_stop": "viewer-only connection restored"}))
        except Exception:
            log.flush()
            print((root / "run.log").read_text()[-12000:])
            raise
        finally:
            if app and app.poll() is None:
                app.terminate()
                try:
                    app.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait()
            acquisition = root / "acquisition"
            for path in acquisition.glob("*.json"):
                try:
                    status = json.loads(path.read_text())
                    (acquisition / f'{status["camera_id"]}.stop').write_text(status["run_id"])
                except (OSError, ValueError, KeyError):
                    pass
            time.sleep(0.5)
            for path in acquisition.glob("*.json"):
                try:
                    status = json.loads(path.read_text())
                    if status["state"] in ("Starting", "Recording", "Stopping") and status.get("process_id"):
                        os.kill(status["process_id"], signal.SIGTERM)
                except (OSError, ValueError, KeyError):
                    pass
            for fixture in fixtures:
                fixture.terminate()
                fixture.wait(timeout=5)
            log.close()


if __name__ == "__main__":
    main()

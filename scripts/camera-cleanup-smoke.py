#!/usr/bin/env python3
"""Exercise real touch cleanup controls while independent recording peers run."""
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
spec = importlib.util.spec_from_file_location("operator_smoke", Path(__file__).with_name("operator-smoke.py"))
operator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(operator)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--fixture", required=True)
    parser.add_argument("--size", default="1280x800")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    with tempfile.TemporaryDirectory(prefix="vidfish-cleanup-smoke-") as folder:
        root = Path(folder)
        config, inventory, probe = root / "config.json", root / "cameras.json", root / "probe.json"
        config.write_text(json.dumps({"output_directory": str(root / "media"), "auto_record": True}))
        log = (root / "run.log").open("w")
        fixture = subprocess.Popen([str(Path(args.fixture).resolve()), "--small"], stdout=subprocess.PIPE, stderr=log, text=True)
        app = None
        try:
            uri = fixture.stdout.readline().strip()
            if not uri.startswith("rtsp://"):
                raise RuntimeError("Cleanup fixture did not start")
            ids = []
            for index in range(4):
                result = subprocess.run([binary, "cameras", "--inventory", str(inventory), "add", "--alias", f"Bench {index + 1:02}", "--rtsp", uri], check=True, capture_output=True, text=True)
                ids.append(result.stdout.strip())
            result = subprocess.run([binary, "cameras", "--inventory", str(inventory), "add-onvif", "--alias", "Offline camera", "--device", "http://127.0.0.1:1/onvif"], check=True, capture_output=True, text=True)
            offline = result.stdout.strip()
            (root / "views.json").write_text(json.dumps({"slots": ids}))
            mouse = operator.Mouse()

            def state():
                try:
                    return json.loads(probe.read_text())
                except (OSError, ValueError):
                    return {}

            def statuses():
                return [json.loads(path.read_text()) for path in (root / "acquisition").glob("*.json")]

            def start():
                probe.unlink(missing_ok=True)
                process = subprocess.Popen([binary, "--no-update-check", "--config", str(config)], env=dict(os.environ, VIDFISH_UI_PROBE=str(probe), VIDFISH_UI_SIZE=args.size), stdout=log, stderr=log)
                mouse.pid = process.pid
                try:
                    operator.until(lambda: len(state().get("slots", [])) == 4 and all(slot["frame"] > 0 for slot in state()["slots"]))
                except Exception:
                    process.kill();process.wait(timeout=10)
                    raise
                return process

            def click(name, roster=False):
                for _ in range(48):
                    time.sleep(.7)
                    value = state()
                    controls=[control for control in value.get("controls",[]) if control[0]==name]
                    matches=[control for control in controls if control[2][0] < (control[1][0]+control[1][2])/2 < control[2][2] and control[2][1] < (control[1][1]+control[1][3])/2 < min(control[2][3],value["viewport"][1])]
                    if matches:
                        rect=matches[0][1]
                        if rect[3]-rect[1]<43:raise RuntimeError("Cleanup control is smaller than its touch target")
                        mouse.click((rect[0]+rect[2])/2*value["scale"],(rect[1]+rect[3])/2*value["scale"])
                        time.sleep(.7)
                        return
                    if roster:
                        clip=controls[0][2] if controls else [value["viewport"][0]-270,390,value["viewport"][0],value["viewport"][1]-90]
                        direction=-1 if controls and (controls[0][1][1]+controls[0][1][3])/2 < clip[1] else 1
                        mouse.scroll(direction,(clip[0]+clip[2])/2*value["scale"],(clip[1]+min(clip[3],value["viewport"][1]-90))/2*value["scale"])
                    else:
                        time.sleep(.2)
                raise RuntimeError("Cleanup control unavailable: " + name)

            app = start()
            operator.until(lambda: len(statuses()) == 4 and all(status["state"] == "Recording" and status["bytes_written"] > 0 for status in statuses()))
            operator.until(lambda: any(camera["id"] == offline and camera["state"] == "Offline" for camera in state().get("camera_states", [])))
            original_runs = {status["camera_id"]: status["run_id"] for status in statuses()}
            click("Camera actions " + ids[0], True)
            click("Forget camera…", True)  # disabled for an actual active recorder
            if state()["cleanup_confirmation"] or len(json.loads(inventory.read_text())["cameras"]) != 5:
                raise RuntimeError("Active recording exposed destructive cleanup")
            click("Hide from list", True)
            operator.until(lambda: ids[0] in state().get("hidden_cameras", []))
            click("Show hidden")
            click("Restore to list", True)
            operator.until(lambda: ids[0] not in state().get("hidden_cameras", []))
            click("Hide hidden")
            click("Clear offline cameras")
            operator.until(lambda: state()["cleanup_confirmation"])
            click("Confirm hide offline")
            operator.until(lambda: state().get("hidden_cameras") == [offline])
            operator.until(lambda: json.loads((root / "views.json").read_text())["hidden_cameras"] == [offline])
            if len(json.loads(inventory.read_text())["cameras"]) != 5:
                raise RuntimeError("Bulk hide forgot saved cameras")
            app.kill()
            app.wait(timeout=10)
            app = start()
            operator.until(lambda: state().get("hidden_cameras") == [offline] and not state()["show_hidden"])
            click("Show hidden")
            click("Camera actions " + offline, True)
            click("Restore to list", True)
            operator.until(lambda: not state()["hidden_cameras"])
            click("Forget camera…", True)
            operator.until(lambda: state()["cleanup_confirmation"])
            click("Cancel camera cleanup")
            if len(json.loads(inventory.read_text())["cameras"]) != 5:
                raise RuntimeError("Cancel forget changed inventory")
            click("Forget camera…", True)
            operator.until(lambda: state()["cleanup_confirmation"])
            click("Confirm forget camera")
            operator.until(lambda: state().get("camera_count") == 4)
            if any(camera["id"] == offline for camera in json.loads(inventory.read_text())["cameras"]):
                raise RuntimeError("Confirmed forget retained the obsolete camera")
            before = {status["camera_id"]: status["bytes_written"] for status in statuses()}
            operator.until(lambda: len(statuses()) == 4 and all(status["state"] == "Recording" and status["run_id"] == original_runs[status["camera_id"]] and status["bytes_written"] > before[status["camera_id"]] for status in statuses()))
            print(json.dumps({"viewport":state()["viewport"],"hide_restore": "passed", "bulk_offline": "hide only", "restart": "preferences persisted", "forget": "confirmed and cancelable", "active_recorder": "protected", "recording_peers": "same runs, continued progress"}))
        finally:
            if sys.exc_info()[0] is not None and probe.exists():
                value=json.loads(probe.read_text())
                print("Cleanup UI diagnostic: "+json.dumps({key:value.get(key) for key in ["camera_count","camera_states","hidden_cameras","show_hidden","cleanup_confirmation","buttons"]}),flush=True)
            # Stop the owner before recorder children so auto-record cannot restart
            # disposable fixture workers during temporary-directory cleanup.
            if app and app.poll() is None:
                app.kill()
                app.wait(timeout=10)
            for path in (root / "acquisition").glob("*.json"):
                status = json.loads(path.read_text())
                path.with_suffix(".stop").write_text(status["run_id"])
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                current = [json.loads(path.read_text()) for path in (root / "acquisition").glob("*.json")]
                if all(status["state"] in ("Stopped", "Failed") for status in current):
                    break
                time.sleep(.2)
            time.sleep(1)
            fixture.terminate()
            fixture.wait(timeout=10)
            fixture.stdout.close()
            log.close()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        message = str(error).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print("::error title=Camera cleanup UI smoke failed::" + message)
        raise

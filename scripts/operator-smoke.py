#!/usr/bin/env python3
"""Native mouse-input smoke: 16 choices, four workers, fullscreen, persistence, recorder isolation."""
import argparse
import ctypes as C
import ctypes.util
import json
import os
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path


DIAGNOSTICS = []

def until(predicate, seconds=60):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.1)
    raise RuntimeError("Operator UI did not reach the expected state")


class Mouse:
    def __init__(self):
        if os.name == "nt":
            self.api = C.windll.user32
            self.api.SetProcessDPIAware()
            self.api.FindWindowW.argtypes = [C.c_wchar_p, C.c_wchar_p]
            self.api.FindWindowW.restype = C.c_void_p
        else:
            self.api = C.CDLL(ctypes.util.find_library("X11"))
            self.test = C.CDLL(ctypes.util.find_library("Xtst"))
            self.api.XOpenDisplay.argtypes = [C.c_char_p]
            self.api.XOpenDisplay.restype = C.c_void_p
            self.display = self.api.XOpenDisplay(os.environ["DISPLAY"].encode())
            if not self.display:
                raise RuntimeError("Cannot open native X display")
            self.api.XFlush.argtypes = [C.c_void_p]
            self.test.XTestFakeMotionEvent.argtypes = [C.c_void_p, C.c_int, C.c_int, C.c_int, C.c_ulong]
            self.test.XTestFakeButtonEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]
            self.test.XTestFakeKeyEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]
            self.api.XKeysymToKeycode.argtypes = [C.c_void_p, C.c_ulong]
            self.api.XKeysymToKeycode.restype = C.c_uint

    def click(self, x, y):
        if os.name == "nt":
            class Point(C.Structure):
                _fields_ = [("x", C.c_long), ("y", C.c_long)]
            window = self.api.FindWindowW(None, "Vidfish")
            if not window:
                raise RuntimeError("Native Vidfish window unavailable")
            origin = Point()
            self.api.ClientToScreen.argtypes = [C.c_void_p, C.POINTER(Point)]
            self.api.ClientToScreen(window, C.byref(origin))
            self.api.SetForegroundWindow.argtypes = [C.c_void_p]
            self.api.SetForegroundWindow(window)
            target_x, target_y = int(x) + origin.x, int(y) + origin.y
            self.api.SetCursorPos(target_x, target_y)
            actual = Point()
            self.api.GetCursorPos(C.byref(actual))
            if actual.x != target_x or actual.y != target_y:
                self.api.PostMessageW.argtypes = [C.c_void_p, C.c_uint, C.c_size_t, C.c_ssize_t]
                packed = (int(x) & 0xFFFF) | ((int(y) & 0xFFFF) << 16)
                self.api.PostMessageW(window, 0x200, 0, packed)
                time.sleep(0.1)
                self.api.PostMessageW(window, 0x201, 1, packed)
                time.sleep(0.08)
                self.api.PostMessageW(window, 0x202, 0, packed)
                return
            time.sleep(0.1)
            self.api.mouse_event(2, 0, 0, 0, 0)
            time.sleep(0.08)
            self.api.mouse_event(4, 0, 0, 0, 0)
        else:
            # xvfb-run has no window manager: client coordinates start at the root origin.
            self.test.XTestFakeMotionEvent(self.display, -1, int(x), int(y), 0)
            self.api.XFlush(self.display)
            time.sleep(0.1)
            self.test.XTestFakeButtonEvent(self.display, 1, 1, 0)
            self.api.XFlush(self.display)
            time.sleep(0.08)
            self.test.XTestFakeButtonEvent(self.display, 1, 0, 0)
            self.api.XFlush(self.display)

    def escape(self):
        if os.name == "nt":
            window = self.api.FindWindowW(None, "Vidfish")
            self.api.PostMessageW.argtypes = [C.c_void_p, C.c_uint, C.c_size_t, C.c_ssize_t]
            self.api.PostMessageW(window, 0x100, 0x1B, 0x00010001)
            time.sleep(0.08)
            self.api.PostMessageW(window, 0x101, 0x1B, 0xC0010001)
        else:
            key = self.api.XKeysymToKeycode(self.display, 0xFF1B)
            self.test.XTestFakeKeyEvent(self.display, key, 1, 0)
            self.test.XTestFakeKeyEvent(self.display, key, 0, 0)
            self.api.XFlush(self.display)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--fixture", required=True)
    args = parser.parse_args()
    binary, fixture_binary = str(Path(args.binary).resolve()), str(Path(args.fixture).resolve())
    with tempfile.TemporaryDirectory(prefix="vidfish-operator-") as folder:
        root = Path(folder)
        config = root / "config.json"
        inventory = root / "cameras.json"
        probe = root / "probe.json"
        settings = {"output_directory": str(root / "media"), "max_cameras": 4, "auto_record": False}
        config.write_text(json.dumps(settings))
        fixture_log, app_log = (root / "fixture.log").open("w"), (root / "app.log").open("w")
        fixture = subprocess.Popen([fixture_binary], stdout=subprocess.PIPE, stderr=fixture_log, text=True)
        apps = []
        stalled_fixture = None
        recorders = []
        try:
            uri = fixture.stdout.readline().strip()
            if not uri.startswith("rtsp://"):
                raise RuntimeError("Operator fixture unavailable")
            stalled_fixture = subprocess.Popen([fixture_binary, "--small"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=fixture_log, text=True)
            stalled_uri = stalled_fixture.stdout.readline().strip()
            if not stalled_uri.startswith("rtsp://"):
                raise RuntimeError("Stall fixture unavailable")
            identifiers = []
            for number in range(16):
                result = subprocess.run([binary, "cameras", "--inventory", str(inventory), "add", "--alias", f"Bench {number + 1:02}", "--rtsp", uri if number < 4 else stalled_uri], check=True, capture_output=True, text=True)
                identifiers.append(result.stdout.strip())
            for identifier in identifiers[:4]:
                recorders.append(subprocess.Popen([binary, "--record-camera", identifier, "--config", str(config)], stdout=app_log, stderr=app_log))
            expected_sizes = {identifier: ([1920, 1080] if index < 4 else [640, 360]) for index, identifier in enumerate(identifiers)}
            mouse = Mouse()

            def start(size):
                probe.unlink(missing_ok=True)
                env = dict(os.environ, VIDFISH_UI_PROBE=str(probe), VIDFISH_UI_SIZE=size)
                app = subprocess.Popen([binary, "--config", str(config)], env=env, stdout=app_log, stderr=app_log)
                apps.append(app)
                return app

            def state():
                try:
                    value = json.loads(probe.read_text())
                except (OSError, ValueError):
                    return {}
                if value.get("peak_workers", 0) > 4:
                    raise RuntimeError("Preview swaps exceeded four worker pipelines")
                return value

            def ready():
                value = state()
                return value if value.get("camera_count") == 16 and all(s["frame"] > 0 and not s["pending"] for s in value["slots"]) else None

            def click(name, index=0):
                value = until(lambda: (v if any(b[0] == name for b in v.get("buttons", [])) else None) if (v := state()) else None)
                buttons = [b for b in value["buttons"] if b[0] == name]
                rect = buttons[index][1]
                if rect[3] - rect[1] < 43:
                    raise RuntimeError("Operator button is smaller than its 44-point touch target")
                mouse.click((rect[0] + rect[2]) / 2 * value["scale"], (rect[1] + rect[3]) / 2 * value["scale"])
                time.sleep(0.25)

            def statuses():
                values = []
                for path in (root / "acquisition").glob("*.json"):
                    try:
                        values.append(json.loads(path.read_text()))
                    except (OSError, ValueError):
                        pass
                return values

            app = start("1280x800")
            until(lambda: state().get("camera_count") == 16)
            click("Show this group")
            first = until(ready)
            original = until(lambda: (s if len(s) == 4 and all(r["state"] == "Recording" and r["bytes_written"] > 0 for r in s) else None) if (s := statuses()) else None)
            original_runs = {r["camera_id"]: (r["run_id"], r["bytes_written"]) for r in original}
            if any(s.get("size") != expected_sizes[s["camera"]] for s in first["slots"]):
                raise RuntimeError("Initial preview resolution was not displayed correctly")
            selected = set(s["camera"] for s in first["slots"])
            for page in range(1, 4):
                click("Next")
                until(lambda: state().get("page") == page)
                current = until(ready)
                if any(s.get("size") != expected_sizes[s["camera"]] for s in current["slots"]):
                    raise RuntimeError("Swapping to a different resolution did not update display textures")
                selected.update(s["camera"] for s in current["slots"])
            if selected != set(identifiers):
                raise RuntimeError("Paging did not expose all sixteen camera identities")
            before = [s["camera"] for s in state()["slots"]]
            click("Max")
            until(lambda: state().get("fullscreen") == 0)
            mouse.escape()
            until(lambda: state().get("fullscreen") is None)
            if [s["camera"] for s in state()["slots"]] != before:
                raise RuntimeError("Fullscreen changed selected cameras")
            stalled_fixture.stdin.write("pause\n")
            stalled_fixture.stdin.flush()
            until(lambda: all("STALE" in s["state"] for s in state().get("slots", []) if s["camera"] not in identifiers[:4]) and any(s["camera"] not in identifiers[:4] for s in state().get("slots", [])))
            click("Grab All")
            until(lambda: state().get("events", 0) >= 1)
            stale_records = list((root / "operator-events").glob("*/event.json"))
            if any(c["image"] for c in json.loads(stale_records[0].read_text())["captures"] if c["camera"] not in identifiers[:4]):
                raise RuntimeError("Stale preview frames were presented as fresh captures")
            stalled_fixture.stdin.write("resume\n")
            stalled_fixture.stdin.flush()
            click("Show this group")
            until(ready)
            click("Mark + Grab All")
            until(lambda: state().get("events", 0) >= 2)
            records = list((root / "operator-events").glob("*/event.json"))
            event = next(json.loads(p.read_text()) for p in records if json.loads(p.read_text())["kind"] == "Mark + Grab All")
            if len(event["captures"]) != 16 or sum(bool(c["image"]) for c in event["captures"]) != 4:
                raise RuntimeError("Preview grab results did not report four selected and twelve unviewed cameras")
            after = statuses()
            if len(after) != 4 or any(r["state"] != "Recording" or r["run_id"] != original_runs[r["camera_id"]][0] for r in after):
                raise RuntimeError("Viewer actions changed an active recorder's state or run ID")
            after_bytes = {r["camera_id"]: r["bytes_written"] for r in after}
            until(lambda: len(values) == 4 and all(r["state"] == "Recording" and r["run_id"] == original_runs[r["camera_id"]][0] and r["bytes_written"] > after_bytes[r["camera_id"]] for r in values) if (values := statuses()) else False)
            choices = until(lambda: (v if v["slots"] == before else None) if (v := json.loads((root / "views.json").read_text())) else None)
            click("Stop all")
            until(lambda: all(r["state"] in ("Stopped", "Failed") for r in statuses()))
            app.terminate()
            app.wait(timeout=20)
            settings["auto_record"] = False
            config.write_text(json.dumps(settings))
            app = start("800x600")
            restored = until(ready)
            if [s["camera"] for s in restored["slots"]] != choices["slots"] or abs(restored["viewport"][0] - 800) > 10:
                raise RuntimeError("Compact restart did not restore the selected views")
            click("Show events")
            click("Hide events")
            click("Max")
            until(lambda: state().get("fullscreen") == 0)
            mouse.escape()
            until(lambda: state().get("fullscreen") is None)
            app.terminate()
            app.wait(timeout=20)
            settings["preview_slots"] = 1
            config.write_text(json.dumps(settings))
            choices["slots"] = [identifiers[0], None, None, identifiers[1]]
            choices["selected_slot"] = 0
            (root / "views.json").write_text(json.dumps(choices))
            app = start("800x600")
            single = until(ready)
            if len(single["slots"]) != 1:
                raise RuntimeError("Configured single-preview limit was ignored")
            click("Next")
            until(lambda: state().get("page") == 1)
            single = until(ready)
            if single["slots"][0]["camera"] != identifiers[1] or single["peak_workers"] > 1:
                raise RuntimeError("A hidden preference exceeded the configured decoder limit")
            until(lambda: json.loads((root / "views.json").read_text())["slots"][3] is None)
            print(json.dumps({"resolutions": ["1280x800", "800x600"], "selectable_cameras": len(selected), "peak_preview_workers": restored["peak_workers"], "fullscreen": "returned", "view_choices": "restored", "stalled_frames": "visible and excluded from grabs", "recording": "uninterrupted by swaps/fullscreen/capture"}))
        finally:
            if sys.exc_info()[0] is not None:
                current = json.loads(probe.read_text()) if probe.exists() else {}
                summary = {key: current.get(key) for key in ("viewport", "scale", "page", "fullscreen", "camera_count", "slots", "active_workers", "peak_workers", "events")}
                if os.name == "nt":
                    summary["host_screen"] = [C.windll.user32.GetSystemMetrics(0), C.windll.user32.GetSystemMetrics(1)]
                DIAGNOSTICS.append("UI: " + json.dumps(summary))
                print("UI diagnostic:", json.dumps(summary), flush=True)
                print("App diagnostic:", "\n".join((root / "app.log").read_text(errors="replace").splitlines()[-20:]), flush=True)
            for app in apps:
                if app.poll() is None:
                    app.terminate()
                try:
                    app.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait()
            for path in (root / "acquisition").glob("*.json"):
                try:
                    value = json.loads(path.read_text())
                    path.with_suffix(".stop").write_text(value["run_id"])
                    until(lambda: json.loads(path.read_text()).get("state") in ("Stopped", "Failed"))
                except (OSError, ValueError, RuntimeError):
                    pass
            for recorder in recorders:
                try:
                    recorder.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    recorder.kill()
                    recorder.wait()
            time.sleep(1)
            if stalled_fixture:
                stalled_fixture.terminate()
                stalled_fixture.wait(timeout=10)
                stalled_fixture.stdin.close()
                stalled_fixture.stdout.close()
            fixture.terminate()
            fixture.wait(timeout=10)
            fixture.stdout.close()
            fixture_log.close()
            app_log.close()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        details = str(error) + "\n" + traceback.format_exc() + "\n" + "\n".join(DIAGNOSTICS)
        message = details.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print(f"::error title=Operator UI smoke failed::{message}")
        raise

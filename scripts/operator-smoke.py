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


def native_ui_size(size):
    if os.name != "nt":
        return size
    api = C.windll.user32
    api.SetProcessDPIAware()
    width, height = map(int, size.split("x"))
    # Leave room for native borders, title bar and the hosted desktop taskbar.
    return f"{min(width, api.GetSystemMetrics(0) - 64)}x{min(height, api.GetSystemMetrics(1) - 80)}"


class Mouse:
    def __init__(self):
        self.pid = None
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

    def window(self):
        found = []
        callback_type = C.WINFUNCTYPE(C.c_int, C.c_void_p, C.c_ssize_t)
        self.api.GetWindowThreadProcessId.argtypes = [C.c_void_p, C.POINTER(C.c_uint)]
        self.api.GetWindowTextW.argtypes = [C.c_void_p, C.c_wchar_p, C.c_int]
        def visit(handle, _):
            pid = C.c_uint()
            self.api.GetWindowThreadProcessId(handle, C.byref(pid))
            title = C.create_unicode_buffer(256)
            self.api.GetWindowTextW(handle, title, 256)
            if pid.value == self.pid and title.value == "Vidfish":
                found.append(handle)
            return True
        callback = callback_type(visit)
        self.api.EnumWindows.argtypes = [callback_type, C.c_ssize_t]
        self.api.EnumWindows(callback, 0)
        if not found:
            raise RuntimeError("Native window for the exact viewer process is unavailable")
        window = found[0]
        self.api.SetWindowPos.argtypes = [C.c_void_p, C.c_void_p, C.c_int, C.c_int, C.c_int, C.c_int, C.c_uint]
        self.api.SetWindowPos(window, None, 0, 0, 0, 0, 0x0015)  # preserve size/z-order/activation
        return window

    def click(self, x, y):
        if os.name == "nt":
            class Point(C.Structure):
                _fields_ = [("x", C.c_long), ("y", C.c_long)]
            window = self.window()
            if not window:
                raise RuntimeError("Native Vidfish window unavailable")
            origin = Point()
            self.api.ClientToScreen.argtypes = [C.c_void_p, C.POINTER(Point)]
            self.api.ClientToScreen(window, C.byref(origin))
            self.api.SetForegroundWindow.argtypes = [C.c_void_p]
            self.api.SetForegroundWindow(window)
            # Winit reads native cursor/button state; posting messages alone does
            # not keep that state consistent. Use actual OS input within the
            # client viewport reported by the application probe.
            self.api.SetCursorPos(int(origin.x + x), int(origin.y + y))
            self.api.mouse_event.argtypes = [C.c_uint, C.c_uint, C.c_uint, C.c_uint, C.c_size_t]
            time.sleep(0.1)
            self.api.mouse_event(0x0002, 0, 0, 0, 0)
            time.sleep(0.08)
            self.api.mouse_event(0x0004, 0, 0, 0, 0)
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

    def type_note(self, text):
        # Xvfb has no window manager to focus a restarted application. Focus the
        # native window under the pointer before sending real keyboard events.
        if os.name != "nt":
            self.api.XDefaultRootWindow.argtypes = [C.c_void_p]
            self.api.XDefaultRootWindow.restype = C.c_ulong
            root = self.api.XDefaultRootWindow(self.display)
            root_return, child = C.c_ulong(), C.c_ulong()
            rx, ry, wx, wy, mask = C.c_int(), C.c_int(), C.c_int(), C.c_int(), C.c_uint()
            self.api.XQueryPointer.argtypes = [C.c_void_p, C.c_ulong, C.POINTER(C.c_ulong), C.POINTER(C.c_ulong), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_uint)]
            self.api.XQueryPointer(self.display, root, C.byref(root_return), C.byref(child), C.byref(rx), C.byref(ry), C.byref(wx), C.byref(wy), C.byref(mask))
            self.api.XSetInputFocus.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_ulong]
            if child.value:
                self.api.XSetInputFocus(self.display, child.value, 1, 0)
                self.api.XFlush(self.display)
                time.sleep(0.1)
        if os.name == "nt":
            self.api.VkKeyScanW.argtypes = [C.c_wchar]
            self.api.VkKeyScanW.restype = C.c_short
            for char in text:
                key = self.api.VkKeyScanW(char)
                if key == -1 or key & 0xFF00:
                    raise RuntimeError("Fixture character requires an unsupported keyboard modifier")
                # TranslateMessage produces WM_CHAR from this key-down. A lone
                # WM_CHAR has no pending winit key event and is discarded.
                self.windows_key(key & 0xFF)
            return
        for char in text:
            key = self.api.XKeysymToKeycode(self.display, ord(char))
            self.test.XTestFakeKeyEvent(self.display, key, 1, 0)
            self.test.XTestFakeKeyEvent(self.display, key, 0, 0)
        self.api.XFlush(self.display)

    def scroll(self, amount, x, y):
        if os.name == "nt":
            class Point(C.Structure):
                _fields_ = [("x", C.c_long), ("y", C.c_long)]
            window = self.window()
            point = Point(int(x), int(y))
            self.api.ClientToScreen.argtypes = [C.c_void_p, C.POINTER(Point)]
            self.api.ClientToScreen(window, C.byref(point))
            self.api.SetForegroundWindow.argtypes = [C.c_void_p]
            self.api.SetForegroundWindow(window)
            self.api.SetCursorPos(point.x, point.y)
            self.api.mouse_event.argtypes = [C.c_uint, C.c_uint, C.c_uint, C.c_uint, C.c_size_t]
            for _ in range(abs(amount)):
                self.api.mouse_event(0x0800, 0, 0, C.c_uint(-120 if amount > 0 else 120).value, 0)
            time.sleep(0.2)
            return
        self.test.XTestFakeMotionEvent(self.display, -1, int(x), int(y), 0)
        self.api.XFlush(self.display)
        for _ in range(abs(amount)):
            button = 5 if amount > 0 else 4
            self.test.XTestFakeButtonEvent(self.display, button, 1, 0)
            self.test.XTestFakeButtonEvent(self.display, button, 0, 0)
        self.api.XFlush(self.display)
        time.sleep(0.2)

    def windows_key(self, key):
        window = self.window()
        self.api.PostMessageW.argtypes = [C.c_void_p, C.c_uint, C.c_size_t, C.c_ssize_t]
        scan = self.api.MapVirtualKeyW(key, 0)
        self.api.PostMessageW(window, 0x100, key, 1 | (scan << 16))
        time.sleep(0.16)
        self.api.PostMessageW(window, 0x101, key, 0xC0000001 | (scan << 16))

    def library_shortcut(self):
        if os.name == "nt":
            self.windows_key(0x78)  # F9
            return
        key = self.api.XKeysymToKeycode(self.display, 0xFFC6)  # F9
        self.test.XTestFakeKeyEvent(self.display, key, 1, 0)
        self.test.XTestFakeKeyEvent(self.display, key, 0, 0)
        self.api.XFlush(self.display)

    def mark_shortcut(self):
        if os.name == "nt":
            self.windows_key(0x75)  # F6
            return
        key = self.api.XKeysymToKeycode(self.display, 0xFFC3)  # F6
        self.test.XTestFakeKeyEvent(self.display, key, 1, 0)
        self.test.XTestFakeKeyEvent(self.display, key, 0, 0)
        self.api.XFlush(self.display)

    def escape(self):
        if os.name == "nt":
            window = self.window()
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
                env = dict(os.environ, VIDFISH_UI_PROBE=str(probe), VIDFISH_UI_SIZE=native_ui_size(size))
                app = subprocess.Popen([binary, "--no-update-check", "--config", str(config)], env=env, stdout=app_log, stderr=app_log)
                apps.append(app)
                mouse.pid = app.pid
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
                time.sleep(0.6)
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
            actual_resolutions = [first["viewport"]]
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
            def event_root():
                registry = json.loads((root / "sessions.json").read_text())
                session = next(entry for entry in registry["sessions"] if entry["id"] == registry["active"])
                return Path(session["directory"]) / "events"
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
            mouse.mark_shortcut()
            until(lambda: state().get("events", 0) >= 1)
            stale_records = list((event_root()).glob("*/event.json"))
            if any(c["image"] for c in json.loads(stale_records[0].read_text())["captures"] if c["camera"] not in identifiers[:4]):
                raise RuntimeError("Mark unexpectedly captured images")
            stalled_fixture.stdin.write("resume\n")
            stalled_fixture.stdin.flush()
            click("Show this group")
            until(ready)
            click("Mark + Grab All")
            mouse.mark_shortcut()
            def complete_events(kind):
                return [event for path in (event_root()).glob("*/event.json") if (event := json.loads(path.read_text()))["kind"] == kind and event["complete"]]
            until(lambda: len(complete_events("Mark")) >= 2, seconds=5)
            until(lambda: bool(complete_events("Mark + Grab All")))
            records = list((event_root()).glob("*/event.json"))
            event = next(json.loads(p.read_text()) for p in records if json.loads(p.read_text())["kind"] == "Mark + Grab All")
            if len(event["captures"]) != 16 or sum(bool(c["image"]) for c in event["captures"]) != 16:
                raise RuntimeError("All-camera grab did not capture all sixteen, including hidden cameras")
            click("Grab All")
            until(lambda: bool(complete_events("Grab All")))
            records = list((event_root()).glob("*/event.json"))
            grab = next(json.loads(p.read_text()) for p in records if json.loads(p.read_text())["kind"] == "Grab All")
            if not all(c["image"] and c["capture_unix_ms"] is not None for c in grab["captures"]):
                raise RuntimeError("Separate Grab All did not capture every camera")
            click("Show events")
            click("Event " + grab["id"])
            click("Edit note")
            click("Event note field")
            mouse.type_note("reviewed")
            click("Save note")
            until(lambda: json.loads((event_root() / grab["id"] / "event.json").read_text())["note"] == "reviewed")
            until(lambda: '"reviewed"' in (event_root() / grab["id"] / "event.csv").read_text())
            click("Hide events")
            after = statuses()
            if len(after) != 4 or any(r["state"] != "Recording" or r["run_id"] != original_runs[r["camera_id"]][0] for r in after):
                raise RuntimeError("Viewer actions changed an active recorder's state or run ID")
            after_bytes = {r["camera_id"]: r["bytes_written"] for r in after}
            until(lambda: len(values) == 4 and all(r["state"] == "Recording" and r["run_id"] == original_runs[r["camera_id"]][0] and r["bytes_written"] > after_bytes[r["camera_id"]] for r in values) if (values := statuses()) else False)
            click("Hide cameras")
            until(lambda: state()["cameras_collapsed"])
            until(lambda: json.loads((root / "views.json").read_text())["cameras_collapsed"])
            click("Show cameras")
            until(lambda: not state()["cameras_collapsed"])
            original_session = json.loads((root / "sessions.json").read_text())["active"]
            click("Sessions / recordings")
            until(lambda: state().get("library", {}).get("open") and not state()["library"]["loading"] and (state()["library"].get("events") or 0) >= 4)
            def library_scroll(amount):
                rect = state()["library"]["rectangle"]
                mouse.scroll(amount, (rect[0] + rect[2]) / 2, rect[1] + 80)
            def library_click(name):
                for _ in range(20):
                    value = state()
                    rect = value["library"]["rectangle"]
                    matches = [button for button in value["buttons"] if button[0] == name and rect[1] + 35 < (button[1][1] + button[1][3]) / 2 < rect[3] - 15]
                    if matches:
                        click(name)
                        return
                    library_scroll(3)
                raise RuntimeError("Library control unavailable: " + name)
            library_click("Start new session")  # disabled while recorders own the session
            if json.loads((root / "sessions.json").read_text())["active"] != original_session:
                raise RuntimeError("Session rollover interrupted active acquisition")
            library_click("Library event " + grab["id"])
            library_click("Export event")
            until(lambda: not state()["library"]["busy"] and state()["library"]["message"].startswith("Export "))
            export_files = list((root / "media" / "exports").glob("*/export.json"))
            if not export_files:
                raise RuntimeError("Native event export produced no manifest")
            export = json.loads(export_files[-1].read_text())
            if not all((Path(export["directory"]) / item["relative"]).exists() for item in export["items"] if item["status"] == "Copied"):
                raise RuntimeError("Export manifest contains unresolved copied paths")
            mouse.library_shortcut()
            until(lambda: not state()["library"]["open"])
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
            until(lambda: state().get("events", 0) >= 2)
            actual_resolutions.append(restored["viewport"])
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
            click("Sessions / recordings")
            until(lambda: state()["library"]["open"] and not state()["library"]["loading"])
            library_scroll(-30)
            library_click("New session name")
            mouse.type_note("nextbench")
            time.sleep(0.7)
            library_click("Start new session")
            until(lambda: json.loads((root / "sessions.json").read_text())["active"] != original_session)
            registry = json.loads((root / "sessions.json").read_text())
            next_session = next(entry for entry in registry["sessions"] if entry["id"] == registry["active"])
            if json.loads((Path(next_session["directory"]) / "manifest.json").read_text())["label"] != "nextbench":
                raise RuntimeError("Native named session creation did not persist")
            print(json.dumps({"requested_resolutions": ["1280x800", "800x600"], "actual_viewports": actual_resolutions, "selectable_cameras": len(selected), "peak_preview_workers": restored["peak_workers"], "fullscreen": "returned", "view_choices": "restored", "sessions": "persistent, safe rollover and native export", "stalled_frames": "visible without blocking Mark", "recording": "uninterrupted by swaps/fullscreen/capture"}))
        finally:
            if sys.exc_info()[0] is not None:
                current = json.loads(probe.read_text()) if probe.exists() else {}
                summary = {key: current.get(key) for key in ("viewport", "scale", "page", "fullscreen", "camera_count", "slots", "active_workers", "peak_workers", "events", "session_id", "library")}
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

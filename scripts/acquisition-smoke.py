#!/usr/bin/env python3
"""Prove an actual recorder survives owner process death and reattaches without duplication."""
import argparse
import json
import os
import signal
import subprocess
import tempfile
import time
from pathlib import Path


def until(predicate, seconds=40):
    deadline = time.monotonic() + seconds
    while True:
        result = predicate()
        if result:
            return result
        if time.monotonic() > deadline:
            raise RuntimeError("Managed recording did not reach the expected state")
        time.sleep(0.1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--fixture", required=True)
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    fixture_binary = str(Path(args.fixture).resolve())
    with tempfile.TemporaryDirectory(prefix="vidfish-acquisition-") as folder:
        root = Path(folder)
        config = root / "config.json"
        inventory = root / "cameras.json"
        config.write_text(json.dumps({"output_directory": str(root / "media"), "segment_seconds": 2}))
        fixture_errors = (root / "fixture-errors.log").open("w")
        owner_logs = (root / "owner.log").open("w")
        fixture = subprocess.Popen([fixture_binary], stdout=subprocess.PIPE, stderr=fixture_errors, text=True)
        owners = []
        status = None
        identifier = None
        try:
            uri = fixture.stdout.readline().strip()
            if not uri.startswith("rtsp://"):
                raise RuntimeError("RTSP fixture failed to start")
            result = subprocess.run([binary, "cameras", "--inventory", str(inventory), "add", "--alias", "Owner survival", "--rtsp", uri], check=True, capture_output=True, text=True)
            identifier = result.stdout.strip()
            status_file = root / "acquisition" / f"{identifier}.json"

            def read_status():
                try:
                    return json.loads(status_file.read_text())
                except (OSError, ValueError):
                    return {}

            def start_owner():
                owner = subprocess.Popen([binary, "--service", "--config", str(config)], stdout=owner_logs, stderr=owner_logs)
                owners.append(owner)
                return owner

            owner = start_owner()
            status = until(lambda: (s if s.get("state") == "Recording" and s.get("bytes_written", 0) > 0 else None) if (s := read_status()) else None)
            run_id = status["run_id"]
            before = status["bytes_written"]
            owner.kill()
            owner.wait(timeout=10)
            status = until(lambda: (s if s.get("state") == "Recording" and s.get("run_id") == run_id and s.get("bytes_written", 0) > before else None) if (s := read_status()) else None)
            owner = start_owner()
            before = status["bytes_written"]
            status = until(lambda: (s if s.get("state") == "Recording" and s.get("run_id") == run_id and s.get("bytes_written", 0) > before else None) if (s := read_status()) else None)
            def closed_segments():
                index = Path(status["output"]).parent / "segments.jsonl"
                try:
                    return [json.loads(line) for line in index.read_text().splitlines() if json.loads(line)["event"] == "closed"]
                except (OSError, ValueError):
                    return []

            finalized_before_crash = until(lambda: (events if len(events) >= 2 else None) if (events := closed_segments()) else None)
            if len(list((root / "media").iterdir())) != 1:
                raise RuntimeError("Owner restart created a duplicate recorder run")
            # Kill the recorder itself as well as the owner: previous closed segments
            # must survive; the restarted recorder reserves a different run directory.
            current = read_status()
            if current["run_id"] != run_id or not current.get("process_id"):
                raise RuntimeError("Cannot identify the exact fixture recorder process")
            os.kill(current["process_id"], signal.SIGTERM if os.name == "nt" else signal.SIGKILL)
            status = until(lambda: (s if s.get("state") == "Recording" and s.get("run_id") != run_id and s.get("bytes_written", 0) > 0 else None) if (s := read_status()) else None)
            run_id = status["run_id"]
            for event in finalized_before_crash:
                subprocess.run([binary, "--inspect", event["location"]], capture_output=True, text=True, check=True)
            events = (root / "acquisition" / f"{identifier}.events.jsonl").read_text()
            if "gap_started_after_worker_loss" not in events:
                raise RuntimeError("Abrupt recorder termination did not leave a gap record")
            until(lambda: read_status().get("bytes_written", 0) > status["bytes_written"])
            owner.kill()
            owner.wait(timeout=10)
            (root / "acquisition" / f"{identifier}.stop").write_text(run_id)
            status = until(lambda: (s if s.get("state") == "Stopped" else None) if (s := read_status()) else None)
            info = subprocess.run([binary, "--inspect", status["output"]], capture_output=True, text=True, check=True)
            media = json.loads(info.stdout)
            if media["width"] != 1920 or media["height"] != 1080 or media["duration_seconds"] < 1:
                raise RuntimeError("Recorder did not finalize valid 1080p media")
            if len(list((root / "media").iterdir())) != 2:
                raise RuntimeError("Recorder recovery created unexpected run directories")
            print(json.dumps({"owner_crash": "survived", "reattached_run": True, "duplicate_recordings": False, "recorder_crash": "recovered", "closed_segments_preserved": len(finalized_before_crash), "gap_logged": True, "finalized_media": media}))
        finally:
            for owner in owners:
                if owner.poll() is None:
                    owner.kill()
                owner.wait(timeout=10)
            if identifier:
                try:
                    current = json.loads((root / "acquisition" / f"{identifier}.json").read_text())
                    (root / "acquisition" / f"{identifier}.stop").write_text(current["run_id"])
                    until(lambda: json.loads((root / "acquisition" / f"{identifier}.json").read_text()).get("state") in ("Stopped", "Failed"), 60)
                    # Status is written just before child exit; let its log handles close.
                    time.sleep(0.5)
                except (OSError, ValueError, RuntimeError):
                    pass
            fixture.terminate()
            try:
                fixture.wait(timeout=5)
            except subprocess.TimeoutExpired:
                fixture.kill()
                fixture.wait()
            fixture.stdout.close()
            fixture_errors.close()
            owner_logs.close()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        message = str(error).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print(f"::error title=Managed recording smoke failed::{message}")
        raise

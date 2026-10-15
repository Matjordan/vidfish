"""Preserve Cargo's exit code and expose runtime test failures to the checks API."""
from collections import deque
import os
import re
import subprocess
import sys

arguments = sys.argv[1:] or ["test", "--locked", "--workspace", "--all-features"]
if not sys.argv[1:] and os.name == "nt":
    # Each capacity test already creates up to sixteen concurrent recorders.
    # Avoid running several such independent workloads on the same hosted runner.
    arguments += ["--", "--test-threads=1"]
process = subprocess.Popen(
    ["cargo", *arguments],
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    text=True,
    encoding="utf-8",
    errors="replace",
)
tail = deque(maxlen=80)
for line in process.stdout:
    print(line, end="", flush=True)
    tail.append(line)
code = process.wait()
if code and os.environ.get("GITHUB_ACTIONS") == "true":
    # Split diagnostics so GitHub does not discard large recorder state dumps.
    detail = re.sub(r"\x1b\[[0-9;]*m", "", "".join(tail))
    for index, offset in enumerate(range(0, len(detail), 2500), 1):
        message = detail[offset:offset + 2500].replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print(f"::error title=Cargo check failed (part {index})::" + message, flush=True)
raise SystemExit(code)

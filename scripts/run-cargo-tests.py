"""Preserve Cargo's exit code and expose runtime test failures to the checks API."""
from collections import deque
import os
import subprocess
import sys

process = subprocess.Popen(
    ["cargo", *(sys.argv[1:] or ["test", "--locked", "--workspace", "--all-features"])],
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
    message = "".join(tail).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print("::error title=Cargo check failed::" + message, flush=True)
raise SystemExit(code)

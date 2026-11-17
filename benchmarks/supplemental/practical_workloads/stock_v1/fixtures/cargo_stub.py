"""Closed fixture tool: never invokes Cargo or publishes a crate."""
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
stdout = ""
stderr = ""
code = 0
if args == ["metadata", "--format-version", "1", "--no-deps"]:
    stdout = json.dumps({
        "workspace_members": ["already-id", "fresh-id"],
        "packages": [
            {"id": "already-id", "name": "already", "version": "1.0", "publish": None},
            {"id": "fresh-id", "name": "fresh", "version": "2.0", "publish": None},
        ],
    }) + "\n"
elif args == ["publish", "--workspace", "--exclude", "already", "--no-verify"]:
    Path("published.txt").write_text(json.dumps(args) + "\n", encoding="utf-8")
else:
    code = 2
    stderr = "unexpected fixture cargo arguments\n"
event = {"argv": args, "cwd": os.getcwd(), "env": dict(sorted(os.environ.items())),
         "exit": code, "stdout": stdout, "stderr": stderr}
with open(os.environ["CARGO_LOG"], "a", encoding="utf-8") as log:
    log.write(json.dumps(event, sort_keys=True) + "\n")
sys.stdout.write(stdout)
sys.stderr.write(stderr)
raise SystemExit(code)

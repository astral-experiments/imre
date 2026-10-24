"""Controlled uv and ty executables for the memory reporter's offline CI test."""

import json
import os
import shlex
import sys
from pathlib import Path

args = sys.argv[1:]
name = Path(sys.argv[0]).name
if name == "uv":
    assert args[:6] == [
        "run",
        "--locked",
        "--python",
        os.environ["RELAR_PYTHON"],
        "--script",
        os.environ["MEMORY_SETUP_SCRIPT"],
    ], args
    project, path = args[-2:]
    if args[6] == "--print-ty-command":
        assert len(args) == 9
        print(
            shlex.join(
                [
                    "{ty}",
                    "check",
                    project,
                    "path with spaces",
                    "",
                    "a'b",
                    "literal#hash",
                ]
            )
        )
    else:
        assert len(args) == 8
        Path(path).mkdir(parents=True)
        print(f"setup stdout {project}", flush=True)
        print(f"setup stderr {project}", file=sys.stderr)
else:
    assert args == [
        "check",
        Path.cwd().name,
        "path with spaces",
        "",
        "a'b",
        "literal#hash",
    ], args
    assert (
        os.environ["TY_MEMORY_REPORT"] == "json"
        and os.environ["TY_MAX_PARALLELISM"] == "1"
    )
    if os.environ["MEMORY_FAIL"] == "1" and name == "ty-new":
        print("analyzer failed", file=sys.stderr)
        sys.exit(3)
    print(
        json.dumps(
            {
                "total_bytes": 99 if name == "ty-old" else 20480,
                "struct_metadata_bytes": 0,
                "struct_fields_bytes": 0,
                "memo_metadata_bytes": 0,
                "memo_fields_bytes": 0,
            }
        )
    )

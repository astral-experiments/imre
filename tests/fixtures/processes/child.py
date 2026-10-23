"""Independent CPython child used by both sides of the process differential."""

import json
import os
from pathlib import Path
import signal
import sys

mode = sys.argv[1]
with open("launched.txt", "a", encoding="utf-8") as marker:
    marker.write(mode + "\n")

if mode == "echo":
    os.write(1, json.dumps(sys.argv[2:], ensure_ascii=False).encode())
    os.write(2, "stderr:λ\r\n".encode())
elif mode == "empty":
    pass
elif mode == "newlines":
    os.write(1, "a\r\nb\rc\nd\r\r\n\0é🙂\x85tail".encode())
    os.write(2, "first\rlast\r\n".encode())
elif mode == "large":
    for _ in range(4):
        os.write(1, b"O" * 65536)
        os.write(2, b"E" * 65536)
elif mode == "stdin":
    os.write(1, sys.stdin.buffer.read())
elif mode == "environment":
    print(Path.cwd().name)
    for key in ["RELAR_PROCESS_PARENT", "RELAR_PROCESS_OVERRIDE", "RELAR_PROCESS_EMPTY", ""]:
        print(key + "=" + repr(os.environ.get(key)))
elif mode == "status":
    os.write(1, "out:λ\r\n".encode())
    os.write(2, "err:δ\r".encode())
    raise SystemExit(int(sys.argv[2]))
elif mode == "invalid":
    os.write(int(sys.argv[2]), b"prefix\xffsuffix")
    Path("completed.txt").write_text("child finished", encoding="utf-8")
    raise SystemExit(7)
elif mode == "signal":
    os.kill(os.getpid(), signal.SIGUSR1)
elif mode == "descriptor":
    try:
        os.fstat(int(sys.argv[2]))
    except OSError:
        print("closed")
    else:
        print("inherited unexpectedly")
elif mode == "write":
    Path(sys.argv[2]).write_text("child ran", encoding="utf-8")
else:
    raise AssertionError(mode)

import io
import os
import sys


if sys.argv[1] == "broken-pipe":
    # The text wrapper is empty, but its binary buffer must still flush on exit.
    sys.stdout.buffer.write(b"pending")
    reader, writer = os.pipe()
    os.close(reader)
    os.dup2(writer, 1)
    os.close(writer)
else:
    raw = sys.stdout.detach()
    if sys.argv[1] == "rewrapped":
        sys.stdout = io.TextIOWrapper(raw, encoding="ascii", errors="backslashreplace")
        sys.stdout.write("new:é")

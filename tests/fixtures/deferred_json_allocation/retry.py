"""Retry a deferred expansion, then read the original array length."""
import json
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text()
try:
    left = json.loads(source)
    Path(sys.argv[2]).write_text("parsed")
    for attempt in range(2):
        try:
            if sys.argv[3] == "equality":
                print(left == json.loads(source))
            else:
                print(len(repr(left["x"])))
        except MemoryError:
            print("refused " + str(attempt))
    print("length " + str(len(left["x"])))
except MemoryError:
    print("parse refused")
print("source " + str(len(source)))

import json
import sys

sys.path.append("")
with open(sys.argv[1], "rb") as stream:
    raw = stream.read()
data = raw.decode() if sys.argv[2] == "str" else raw
with open("/dev/stdin", "rb") as gate:
    print("ready", flush=True)
    gate.read(1)
try:
    print(len(json.loads(data)))
except MemoryError:
    print("refused")
print(len(json.loads("[]")))

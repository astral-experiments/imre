import json
from pathlib import Path

from imre import struct


@struct
class Point:
    x: float


point = Point(1.0)
for case in range(10):
    try:
        with open("out.json", "w") as stream:
            if case == 0:
                json.dump([1, 2], stream, indent=None, separators=(False, ":"), allow_nan=False)
            elif case == 1:
                json.dump([[1], [2]], stream, indent=None, separators=(False, ":"), allow_nan=False)
            elif case == 2:
                json.dump({"x": 1}, stream, indent=None, separators=(",", point), allow_nan=False)
            elif case == 3:
                json.dump({"x": 1, "y": 2}, stream, indent=None, separators=(point, ":"), allow_nan=False)
            elif case == 4:
                json.dump({"x": 1}, stream, indent=2, separators=(point, ":"), allow_nan=False)
            elif case == 5:
                json.dump([1, 2], stream, indent=None, separators=([7], ":"), allow_nan=False)
            elif case == 6:
                json.dump([1, 2], stream, indent=None, separators=((7, 8), ":"), allow_nan=False)
            elif case == 7:
                json.dump([1, 2], stream, indent=None, separators=(b"x", ":"), allow_nan=False)
            elif case == 8:
                json.dump([1], stream, indent=None, separators=(point, ":"), allow_nan=False)
            elif case == 9:
                json.dump([1.0, float("nan")], stream, indent=None, separators=(False, ":"), allow_nan=False)
        print("ok")
    except TypeError as error:
        print("TypeError: " + str(error))
    except ValueError as error:
        print("ValueError: " + str(error))
    print(repr(Path("out.json").read_bytes()))

# The text writer and the C encoder both call a None argument "None".
try:
    with open("out.json", "w") as stream:
        json.dump([[1], [2]], stream, separators=(None, ":"))
except TypeError as error:
    print(str(error))
print(repr(Path("out.json").read_bytes()))
try:
    print(json.dumps(17, separators=(None, ":")))
except TypeError as error:
    print(str(error))

import json

from imre import struct


@struct
class Point:
    x: float
    b: bool


# The C encoder's 50-byte type name ends inside the final character.
@struct
class RecordWithANameThatCrossesTheJsonEncoderByteLimitΩ:
    flag: bool


p = Point(1.0, True)
rows = [p, p]
print("pair")
try:
    print(json.dumps(17, separators=rows))
except TypeError as error:
    print(str(error))

print("item")
try:
    print(json.dumps(17, separators=(p, ":")))
except TypeError as error:
    print(str(error))

print("key")
try:
    print(json.dumps(17, separators=(",", p)))
except TypeError as error:
    print(str(error))

print("direct")
try:
    print(json.dumps(17, separators=p))
except TypeError as error:
    print(str(error))

print("nested")
try:
    print(json.dumps({"k": [1, 2]}, indent=2, separators=(p, ":")))
except TypeError as error:
    print(str(error))

print("good")
try:
    print(json.dumps({"k": [1, 2]}, separators=[",", ":"]))
except TypeError as error:
    print(str(error))

long_name = RecordWithANameThatCrossesTheJsonEncoderByteLimitΩ(True)
try:
    print(json.dumps(17, separators=(",", long_name)))
except UnicodeDecodeError as error:
    print(str(error))

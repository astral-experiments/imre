import json


class Log:
    entries: list[str]

    def __init__(self) -> None:
        self.entries = []

    def number(self, token: str) -> str:
        self.entries.append(token)
        if token == "4":
            raise ValueError("four")
        return "number:" + token


def truth(token: str) -> bool:
    return token != "-0"


def nothing(token: str) -> None:
    return None


log = Log()
print(json.dumps(json.loads('{"a":[-0,1.20e+03,NaN], "b":2}',
                            parse_float=log.number, parse_constant=log.number, parse_int=log.number)))
print(repr(log.entries))
try:
    json.loads('[3,4,bad]', parse_int=log.number)
except ValueError as exc:
    print(str(exc))
print(repr(log.entries))
print(json.dumps(json.loads(b'[3,1.5,Infinity]', parse_int=lambda s: int(s) + 7,
                            parse_float=float, parse_constant=str.upper)))
print(json.dumps(json.loads('[0,-0,1.2,NaN]', parse_int=truth, parse_float=nothing,
                            parse_constant=str.lower)))
print(json.dumps(json.loads('{"a\t":"b\n"}', strict=False)))
try:
    json.loads('[1, ]', parse_int=str, strict=False)
except json.JSONDecodeError as exc:
    print(exc.msg)
    print(exc.pos)
    print(exc.colno)

depth = 257
root = json.loads("[" * depth + "1.20e+03" + "]" * depth, parse_float=log.number)
leaf = root[0]
for _ in range(1, depth):
    leaf = leaf[0]
result: str = leaf
print(result)
print(log.entries[-1])

try:
    json.loads("[" * 500_000 + "7" + "]" * 500_000, parse_int=log.number)
except RecursionError:
    print("RecursionError")
print(log.entries[-1])

source = '["é\nx", 6, ]'
try:
    json.loads(source, parse_int=log.number, strict=False)
except json.JSONDecodeError as error:
    print(error.msg)
    print(error.pos)
    print(error.lineno)
    print(error.colno)
    print(error.doc == source)
print(log.entries[-1])
print(json.dumps(json.loads(r'["a\\b","c\"d"]', strict=False)))

rows = json.loads('[{"a_key":2},{"a_\\u006bey":3}]', parse_int=str)
keys: list[str] = []
for row in rows:
    for key in row:
        name: str = key
        keys.append(name)
print(keys[0] is keys[1])

data = b'\x80'
try:
    json.loads(data, parse_int=str)
except UnicodeDecodeError as error:
    print(str(error))
    print(repr(error))

with open("json-numbers", "wb") as out:
    out.write(b'\xff\xfe[\x001\x00.\x005\x00]\x00')
with open("json-numbers", "rb") as stream:
    print(json.dumps(json.load(stream, parse_float=log.number)))

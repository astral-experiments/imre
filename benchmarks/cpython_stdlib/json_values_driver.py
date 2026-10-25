"""Typed native operations for the pinned JSON value and text-stream tests."""
import json
import sys


def run() -> None:
    method = sys.argv[1]
    shape = sys.argv[2]
    ascii_only = sys.argv[3] == "1"
    allow_nan = sys.argv[4] == "1"
    indent: int | None = None if sys.argv[5] == "None" else int(sys.argv[5])
    if method == "load" and shape == "binary":
        with open("input.bin", "rb") as stream:
            loaded = json.load(stream)
        print("OK")
        print(repr(loaded))
        return
    if method == "load":
        with open("input.bin") as source:
            loaded = json.load(source)
        print("OK")
        print(repr(loaded))
        return
    with open("input.bin", "rb") as binary:
        data = binary.read()
    if method == "strict" or method == "parse-int-float" or method == "parse-constant-upper":
        if method == "strict":
            loaded = json.loads(data, strict=False) if shape == "bytes" else json.loads(data.decode(), strict=False)
        elif method == "parse-int-float":
            loaded = json.loads(data, parse_int=float) if shape == "bytes" else json.loads(data.decode(), parse_int=float)
        else:
            loaded = json.loads(data, parse_constant=str.upper) if shape == "bytes" else json.loads(data.decode(), parse_constant=str.upper)
        print("OK")
        print(repr(loaded))
        return
    if method == "loads":
        loaded = json.loads(data) if shape == "bytes" else json.loads(data.decode())
        print("OK")
        print(repr(loaded))
        return

    if shape == "dict":
        words = data.decode().splitlines()
        mapping: dict[int, int] = {}
        for i in range(0, len(words), 2):
            mapping[int(words[i])] = int(words[i + 1])
        if method == "dump":
            with open("output.txt", "w") as out:
                result = json.dump(mapping, out, indent=indent, ensure_ascii=ascii_only, allow_nan=allow_nan)
            print("OK")
            print(result)
            return
        encoded = json.dumps(mapping, indent=indent, ensure_ascii=ascii_only, allow_nan=allow_nan)
    elif shape == "int":
        encoded = json.dumps(int(data.decode()), ensure_ascii=ascii_only, allow_nan=allow_nan)
    elif shape == "float":
        encoded = json.dumps(float(data.decode()), ensure_ascii=ascii_only, allow_nan=allow_nan)
    elif shape == "list-float":
        encoded = json.dumps([float(data.decode())], ensure_ascii=ascii_only, allow_nan=allow_nan)
    elif shape == "str":
        encoded = json.dumps(data.decode(), ensure_ascii=ascii_only, allow_nan=allow_nan)
    elif shape == "list-str":
        encoded = json.dumps([data.decode()], ensure_ascii=ascii_only, allow_nan=allow_nan)
    elif shape == "bytes":
        encoded = json.dumps(data)
    elif shape == "list-bytes":
        encoded = json.dumps([data])
    else:
        raise RuntimeError("unknown native JSON transport")
    print("OK")
    print(encoded)


try:
    run()
except json.JSONDecodeError as error:
    print("JSONDecodeError")
    print(str(error))
    if len(sys.argv) > 6 and sys.argv[6] == "fields":
        print(repr((error.msg, error.doc, error.pos, error.lineno, error.colno)))
except TypeError as error:
    print("TypeError")
    print(str(error))
except UnicodeDecodeError as error:
    print("UnicodeDecodeError")
    print(str(error))
except ValueError as error:
    print("ValueError")
    print(str(error))

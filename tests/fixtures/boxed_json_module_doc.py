import hashlib
import json


def current():
    original = json.__doc__
    print(None if original is None else hashlib.sha256(original.encode()).hexdigest())
    try:
        json.__doc__ = "replacement"
        print(vars(json)["__doc__"])
        del json.__doc__
        print("__doc__" in vars(json))
    finally:
        json.__doc__ = original

    for name in ("dump", "dumps", "loads", "JSONDecodeError"):
        exported = getattr(json, name)
        try:
            delattr(json, name)
            assert not hasattr(json, name)
        finally:
            setattr(json, name, exported)


if __name__ == "__main__":
    current()

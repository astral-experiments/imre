from typing import Callable, TextIO


def temporary_source() -> dict[str, TextIO]:
    stream = open("source.txt", "w", encoding="utf-8")
    stream.write("source retained through keys")
    return {"beta": stream, "alpha": stream}


def inspect_key(name: str) -> str:
    with open("source.txt", "r", encoding="utf-8") as stream:
        print(f"key file {name} {stream.read()}")
    return name


def failing_key(name: str) -> str:
    if name == "alpha":
        raise ValueError("key failed")
    return inspect_key(name)


def read_source() -> None:
    with open("source.txt", "r", encoding="utf-8") as stream:
        print(f"after keys {stream.read()}")


def identity(name: str) -> str:
    return name


def add_key(values: dict[str, int]) -> Callable[[str], str]:
    values["a"] = 1
    return identity


names = sorted(temporary_source(), key=inspect_key)
print(f"names {','.join(names)}")
read_source()
try:
    failed = sorted(temporary_source(), key=failing_key)
    print(f"unexpected {len(failed)}")
except ValueError:
    print("key failed")
read_source()

mapping = {"b": 2}
print(f"ordinary {','.join(sorted(mapping, key=add_key(mapping)))}")
mapping = {"b": 2}
try:
    result = sorted((name for name in mapping), key=add_key(mapping))
    print(f"unexpected generator {len(result)}")
except RuntimeError:
    print("generator acquired before keyword")

print(abs(-4611686018427387904))
print(abs(-1267650600228229401496703205376))
print(f"{abs(True)} {abs(False)}")
for value in [-0.0, -2.675, float("-inf"), -float("nan")]:
    print(f"{abs(value):.2f}")

class Box:
    value: str | bytes
    maybe: str | bytes | None

    def __init__(self, value: str | bytes) -> None:
        self.value = value
        self.maybe = None


def show(value: str | bytes) -> str:
    if isinstance(value, bytes):
        return value.decode()
    return value


def snapshot(box: Box) -> str:
    value = box.value
    if isinstance(value, str):
        box.value = b"replaced"
        return value + " snapshot"
    return value.decode()


def exception_cleanup(source: list[str | bytes]) -> None:
    try:
        discarded: list[str | bytes] = ["kept until failure", *source, failure()]
    except ValueError:
        print("cleaned union owners")


def failure() -> str | bytes:
    raise ValueError("failure")


values: list[str | bytes] = ["first", b"second"]
values.append(b"third")
values[0] = b"replacement"
copied: list[str | bytes | None] = [*values, None]
mapping: dict[str, str | bytes] = {"first": "one", "second": b"two"}
mapping["first"] = b"changed"
expanded: dict[str, str | bytes | None] = {**mapping, "third": None}
for value in values:
    print(show(value))
for optional_value in copied:
    if optional_value is not None:
        print(show(optional_value))
for key in expanded:
    mapped_value = expanded[key]
    if mapped_value is not None:
        print(show(mapped_value))
first, *rest = values
print(show(first))
for value in rest:
    print(show(value))
box = Box("original")
print(snapshot(box))
print(show(box.value))
box.maybe = "optional"
current = box.maybe
if current is not None:
    print(show(current))
items = [show(value) for value in values if isinstance(value, (str, bytes))]
for item in items:
    print(item)
exception_cleanup(values)

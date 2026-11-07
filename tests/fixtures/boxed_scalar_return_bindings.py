import builtins as b


class Text(b.str):
    def value(self) -> str:
        return self


class Blob(b.bytes):
    pass


events = []
str = b.bytes
Copied = str


def copied(value: Copied) -> str:
    events.append("body")
    return value


def qualified(value) -> b.str:
    return value


def reject(function, value):
    try:
        function(value)
    except TypeError:
        return
    raise AssertionError("invalid scalar was admitted")


blob = Blob(b"wire")
text = Text("wire")
assert copied(blob) is blob
reject(copied, text)
assert events == ["body"]
assert qualified(text) is text
reject(qualified, blob)

str = b.str
assert text.value() is text
str = Text


def header(value) -> str:
    return value


assert header.__annotations__["return"] is Text
assert header(text) is text
reject(header, b"wire")
reject(header, "wire")


def opaque(format):
    events.append(format)
    return {"return": b.str}


header.__annotate__ = opaque
reject(header, object())
assert events == ["body"]
assert header.__annotations__["return"] is b.str
assert header(text) is text
assert events == ["body", 1]


class Key:
    def __hash__(self):
        return hash("return")

    def __eq__(self, other):
        events.append("key equality")
        return active and other == "return"


active = False
annotations = {Key(): b.bytes, "return": b.str}
active = True
header.__annotations__ = annotations
assert annotations["return"] is b.bytes
events.clear()
reject(header, text)
assert events == []


def nothing(factory) -> None:
    return factory()


globals()["None"] = b.str
reject(nothing, lambda: text)
assert nothing.__annotations__["return"] is None
reject(nothing, lambda: text)


def debug_annotation(value) -> __debug__:
    return value


globals()["__debug__"] = b.str
assert debug_annotation(23) == 23
debug_annotation.__annotations__
assert debug_annotation(23) == 23

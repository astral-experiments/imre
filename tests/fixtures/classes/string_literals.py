from dataclasses import dataclass
from typing import Literal as Choices


@dataclass
class Label:
    text: Choices["added", "removed"] = "added"


def label(value: Choices["added", "removed"] = "removed") -> str:
    return f"{value}:{len(value)}"


def select(added: bool) -> Choices["added", "removed"]:
    return "added" if added else "removed"


def widen(value: Choices["added"]) -> Choices["removed", "added", "added"]:
    return value


def checked(text: str) -> Choices["added", "removed"]:
    if text == "added":
        return text
    if text != "removed":
        raise ValueError("unknown verb")
    return text


def optional_label(value: Choices["added", "removed"] | None) -> str:
    if isinstance(value, str):
        return label(value)
    return "missing"


def as_text_or_bytes(value: Choices["added"] | None) -> str | bytes | None:
    return value


record = Label()
print(label(record.text))
record.text = select(False)
print(label(record.text))
print(label())
print(widen("added"))
print(optional_label(None))
print(optional_label("added"))
print(as_text_or_bytes(None) is None)
print(isinstance(as_text_or_bytes("added"), str))
print(checked("removed"))
try:
    print(checked("typo"))
except ValueError as error:
    print(str(error))
labels: list[Choices["é", ""]] = ["", "é"]
print(",".join(labels))
possible: Choices["é", ""] | None = None
print(possible is None)
possible = labels[1]
if possible is not None:
    print(possible + "!")

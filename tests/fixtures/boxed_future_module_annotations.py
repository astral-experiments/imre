from __future__ import annotations

from typing import Final

initial = __annotations__
assert initial == {} and __conditional_annotations__ == set()
name: Final[str] = "saved"
if False:
    skipped: Unknown
for _ in range(2):
    later: list[Missing]
name: bytes

events = []


class Sink:
    def __setitem__(self, key, spelling):
        events.append((key, spelling, assigned))


__annotations__ = Sink()
assigned: tuple[Missing, ...] = 42
del __annotations__
try:
    unrecorded: Missing = 7
except NameError:
    assert unrecorded == 7
else:
    raise AssertionError("a deleted annotation mapping was recreated")

__annotations__ = initial


def current():
    local: int
    assert list(__annotations__.items()) == [("name", "bytes"), ("later", "list[Missing]")]
    assert events == [("assigned", "tuple[Missing, ...]", 42)]
    assert __annotations__ is initial

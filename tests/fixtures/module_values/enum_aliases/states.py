from __future__ import annotations

from enum import StrEnum as FixedEnum, auto as member


def ready(value: State | None = None) -> bool:
    return value is None


READY = ready()


def current() -> State:
    return State.ALIAS


class State(FixedEnum):
    ACTIVE = member()
    ALIAS = ACTIVE
    DONE = "finished"


CURRENT: State = current()

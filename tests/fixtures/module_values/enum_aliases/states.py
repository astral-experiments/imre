from enum import StrEnum as FixedEnum, auto as member


class State(FixedEnum):
    ACTIVE = member()
    ALIAS = ACTIVE
    DONE = "finished"


CURRENT: State = State.ALIAS

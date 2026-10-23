from enum import StrEnum, auto
from typing import assert_never as exhausted


class Action(StrEnum):
    SAME = auto()
    ADD = "add"
    ADD_ALIAS = ADD
    REMOVE = auto()


def markers(values: list[Action | None], upper: bool) -> str:
    output = ""
    for action in values:
        if action is None:
            continue
        if action == Action.SAME:
            continue
        if upper:
            if action == Action.ADD_ALIAS:
                output += "A"
            elif action == Action.REMOVE:
                output += "R"
            else:
                exhausted(action)
        elif Action.ADD != action:
            output += "-"
        elif action == Action.ADD:
            output += "+"
        else:
            exhausted(action)
    return output


values: list[Action | None] = [Action.REMOVE, None, Action.SAME, Action.ADD_ALIAS, Action.ADD]
print(markers(values, True))
print(markers(values, False))

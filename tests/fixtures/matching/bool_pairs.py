from typing import assert_never as unreachable


def classify(left: bool, right: bool) -> int:
    match (left, right):
        case (True, True):
            return 3
        case (True, False):
            return 2
        case (False, True):
            return 1
        case (False, False):
            return 0
        case _:
            unreachable((left, right))


def observed(value: bool, events: list[bool]) -> bool:
    events.append(value)
    return value


def equal(left: bool, right: bool, events: list[bool]) -> bool:
    match observed(left, events), observed(right, events):
        case (True, True) | (False, False):
            return True
        case (True, False) | (False, True):
            return False
        case _:
            pass


events: list[bool] = []
for left in [False, True]:
    for right in [False, True]:
        print(classify(left, right))
        print(equal(left, right, events))
for event in events:
    print(event)

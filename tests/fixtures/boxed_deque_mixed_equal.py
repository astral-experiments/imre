from collections import deque


def build(depth, container):
    value = 1
    for _ in range(depth):
        value = deque([0, container((value, 1))])
    return value


class Key:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        print("eq", self.value)
        return self.value == other.value

    def __ne__(self, other):
        print("ne", self.value)
        return self.value != other.value


def current():
    for container in (list, tuple):
        print(build(1200, container) == build(1200, container))
    for right in (2, 3):
        left = deque([[Key(1), Key(2)]])
        other = deque([[Key(1), Key(right)]])
        print(left == other)
        print(left != other)
    left = deque()
    right = deque()
    left.append([left])
    right.append([right])
    try:
        print(left == right)
    except RecursionError:
        print("recursive")

from collections import deque


def nested(depth, leaf):
    for _ in range(depth):
        leaf = deque([leaf])
    return leaf


def current():
    print(nested(1200, 1) == nested(1200, 1))
    print(nested(1200, 1) != nested(1200, 1))
    left, right = deque(), deque()
    left.append(left)
    right.append(right)
    try:
        print(left == right)
    except RecursionError:
        print("recursive")
    print(left == left, left != left)

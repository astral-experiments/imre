from unittest import TestCase


case = TestCase()
compare = getattr(case, "assertCountEqual")
compare(iter(["x", "x", "y"]), ["y", "x", "x"])
compare([[1], {"a": 2}, [1]], [{"a": 2}, [1], [1]])

for first, second in [(["b", "a", "b"], ["c", "a"]), ([[1], [], [1]], [[], [2]])]:
    try:
        compare(iter(first), iter(second), "mismatch")
    except AssertionError as exc:
        print(str(exc))
    else:
        raise RuntimeError("different multiplicities passed")

case.maxDiff = 0
try:
    compare([1], [2])
except AssertionError as exc:
    print(str(exc))
else:
    raise RuntimeError("truncated difference passed")

class CountFailure(Exception):
    pass

case.failureException = CountFailure
case.longMessage = False
message = ["preserve identity"]
try:
    compare([], [1], message)
except CountFailure as exc:
    assert exc.args[0] is message
else:
    raise RuntimeError("custom failure lost")

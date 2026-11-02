import sys


def descend(count):
    if count:
        return descend(count - 1)
    return "done"


def check_depth():
    frame = sys._getframe()
    depth = 0
    while frame is not None:
        depth += 1
        frame = frame.f_back
    try:
        sys.setrecursionlimit(1)
    except RecursionError as error:
        assert str(error) == (
            f"cannot set the recursion limit to 1 at the recursion depth {depth}: "
            "the limit is too low"
        )
    else:
        raise AssertionError("lowered the recursion limit below active Python frames")


def current():
    original = sys.getrecursionlimit()
    assert original == 1000
    check_depth()

    class Limit:
        def __index__(self):
            return 50

    try:
        sys.setrecursionlimit(Limit())
        assert sys.getrecursionlimit() == 50
        assert descend(3) == "done"
        try:
            descend(100)
        except RecursionError:
            pass
        else:
            raise AssertionError("recursive calls ignored the lowered limit")
        sys.setrecursionlimit(1100)
        assert descend(1005) == "done"
        for invalid, kind in [(0, ValueError), (None, TypeError), (1 << 90, OverflowError)]:
            try:
                sys.setrecursionlimit(invalid)
            except kind:
                assert sys.getrecursionlimit() == 1100
            else:
                raise AssertionError("accepted an invalid recursion limit")
    finally:
        sys.setrecursionlimit(original)
    assert sys.getrecursionlimit() == original

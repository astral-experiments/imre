from functools import total_ordering


@total_ordering
class Rank:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        if isinstance(other, Rank):
            return self.value < other.value
        return NotImplemented

    def __eq__(self, other):
        if isinstance(other, Rank):
            return self.value == other.value
        return NotImplemented


class First(Rank):
    def __gt__(self, other):
        return "reflected"


class Foreign:
    def __lt__(self, other):
        return "fallback"


@total_ordering
class Reverse:
    def __gt__(self, other):
        return False


def current():
    low, high = Rank(1), Rank(2)
    assert low < high and high > low and low <= high and high >= low
    assert low <= low and low >= low and not low > low
    assert low.__gt__(Foreign()) is NotImplemented
    assert (low > Foreign()) == "fallback"
    assert (low < First(0)) == "reflected"
    try:
        low <= Foreign()
    except TypeError:
        pass
    else:
        raise AssertionError("both comparison slots declined")

    assert Reverse() <= Reverse()
    try:
        total_ordering(object)
    except ValueError:
        pass
    else:
        raise AssertionError("object has no user-defined ordering")

    print("ordering ok")


if __name__ == "__main__":
    current()

class Rank:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        return self.value < other.value


def current():
    print(min(len('health'), len('healt')))
    first = int("12345")
    equal = int("12345")
    negative_zero = float("-0.0")
    positive_zero = float("0.0")
    nan = float("nan")
    for choose in (min, max):
        assert choose(first, equal) is first
        assert choose(equal, first) is equal
        assert choose(negative_zero, positive_zero) is negative_zero
        assert choose(nan, positive_zero) is nan
        assert choose(positive_zero, nan) is positive_zero
    print(min(equal, -first), max(-first, equal))
    print(min(-2.5, 1.5), max(-2.5, 1.5))
    print(min(10**40, 1), max(1, 10**40), min(True, 1) is True)
    rules = [Rank(7), Rank(3), Rank(3)]
    calls = []

    def score(rule):
        calls.append(rule.value)
        return Rank(rule.value)

    sentinel = object()
    print(min(iter(rules), key=score, default=sentinel) is rules[1], calls)
    print(min([], key=score, default=sentinel) is sentinel)
    print(min(rules[0], rules[1], key=None) is rules[1])
    for invoke in (lambda: min(), lambda: min((), key=2), lambda: min(1, 2, default=0)):
        try:
            invoke()
        except (TypeError, ValueError) as exc:
            print(type(exc).__name__, str(exc))

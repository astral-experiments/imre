class Rank:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        return self.value < other.value


def current():
    print(min(len('health'), len('healt')))
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

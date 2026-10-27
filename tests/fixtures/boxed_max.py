class Rule:
    def __init__(self, endpoint):
        self.endpoint = endpoint


def current():
    rules = [Rule("visits"), Rule("health"), Rule("hello")]
    calls = []

    def score(rule):
        calls.append(rule.endpoint)
        return -abs(len(rule.endpoint) - 1)

    print(max((rule for rule in rules), key=score) is rules[2], calls)
    print(max(*rules[:2], key=score) is rules[0])
    print(max(1, 3.0, 2), max("213", key=None))
    print(max([], default=rules[0], key=1) is rules[0])

    class Sequence:
        def __getitem__(self, index):
            return rules[index]

    print(max(Sequence(), key=score) is rules[2])

    class FailedSequence:
        def __getitem__(self, index):
            raise ValueError("index")

    def stream():
        for x in (1, 3, 2):
            calls.append(("next", x))
            yield x

    def stop(x):
        calls.append(("key", x))
        if x == 3:
            raise ValueError("key")
        return x

    calls.clear()
    for operation in (
        lambda: max(stream(), key=stop),
        lambda: max(FailedSequence()),
        lambda: max(),
        lambda: max((), key=1),
        lambda: max([0], key=1),
        lambda: max(1, 2, default=None),
        lambda: max([], unknown=1),
    ):
        try:
            operation()
        except (TypeError, ValueError) as error:
            print(type(error).__name__, str(error))
    print(calls)

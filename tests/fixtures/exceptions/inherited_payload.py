"""A second payload extends the parent record without changing its args slot."""


class Problem(Exception):
    pass


class RunFailure(Problem):
    def __init__(self, message: str, errors: str, failures: str, *, stats: int | None = None) -> None:
        self.message = message
        self.stats = stats
        super().__init__(message, errors, failures)

    def __str__(self) -> str:
        return self.message


class DetailedFailure(RunFailure):
    def __init__(self, message: str, errors: str, failures: str, stats: int) -> None:
        self.errors = errors
        self.failures = failures
        super().__init__(message, errors, failures, stats=stats)


class Leaf(DetailedFailure):
    pass


class Unrelated(Problem):
    pass


def exercise() -> None:
    exc = Leaf("failed", "setup", "body", 7)
    try:
        raise exc
    except Unrelated:
        raise AssertionError("crossed a sibling exception handler")
    except DetailedFailure as child:
        print(child is exc)
        print(child.errors, child.failures, child.stats, child.message)
        print(str(child), repr(child), repr(child.args))
    try:
        raise exc
    except RunFailure as parent:
        print(parent is exc, isinstance(parent, Leaf), isinstance(parent, Unrelated))
        print(parent.message, parent.stats, repr(parent.args))
    try:
        raise exc
    except Problem as marker:
        print(marker is exc, str(marker), repr(marker.args))
    print(repr(RunFailure("root", "one", "two").args))


exercise()

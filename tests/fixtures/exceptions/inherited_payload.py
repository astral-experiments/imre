from collections import namedtuple


Pair = namedtuple("Pair", "first second")


class Display(Exception):
    def __init__(self, msg, *args):
        self.msg = msg
        super().__init__(msg, *args)

    def __str__(self):
        return self.msg


class Family(ValueError):
    def __init__(self, message: str, code: int) -> None:
        self.message = message
        self.code = code
        super().__init__(message, code)

    def __str__(self) -> str:
        return f"{self.message}: {self.code}"


class Extra(Family):
    def __init__(self, message: str, code: int, details: str) -> None:
        self.details = details
        super().__init__(message, code)


class Passive(Extra):
    pass


class Left(Passive):
    pass


class Right(Passive):
    pass


class Joined(Left, Right):
    pass


def exercise() -> None:
    problem = Passive("☃", 17, "detail")
    print(str(problem))
    print(repr(problem))
    print(repr(problem.args))
    print(problem.message)
    print(problem.code)
    print(problem.details)
    try:
        raise problem
    except Extra as caught:
        print(caught is problem)
        print(caught.details)
    try:
        raise problem
    except Family as parent:
        print(parent.message)
        print(parent.code)
        print(repr(parent.args))
        print(repr(parent))


class Original(Exception):
    def __init__(receiver, first, *rest, summary=None):
        receiver.value = first
        super().__init__(first, *rest)


class Replaced(Original):
    def __init__(self, before, after="after", *, detail):
        self.value = after
        super().__init__(before, after, summary=detail)
        self.value = detail


exercise()
try:
    print(str(Display(Pair("a", "b"))))
except TypeError as caught:
    print(str(caught))
joined = Joined("shared", 3, "detail")
print(isinstance(joined, Left))
print(isinstance(joined, Right))
print(str(joined))
print(joined.details)
changed = Replaced("before", detail="final")
print(str(changed))
print(repr(changed.args))
print(changed.value)
try:
    Replaced("a", "b", "c", detail="d")
except TypeError as caught:
    print(str(caught))
try:
    Replaced("only")
except TypeError as caught:
    print(str(caught))

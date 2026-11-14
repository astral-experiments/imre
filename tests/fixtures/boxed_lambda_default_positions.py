"""Live caller and traceback positions during lambda default evaluation."""


def current():
    import inspect
    import sys
    import traceback

    def position():
        caller = sys._getframe(1)
        return caller.f_lineno, tuple(inspect.getframeinfo(caller, context=0).positions)

    def make(n):
        return lambda where=(
            {"é": position()}["é"]
        ), value=(
            100 + (2 + n)
        ): (where, value)

    print("bound", make(3)())
    try:
        make("bad")
    except TypeError as exc:
        frame = traceback.extract_tb(exc.__traceback__)[-1]
        print(frame.name, (frame.lineno, frame.end_lineno, frame.colno, frame.end_colno))
    else:
        raise AssertionError("expected the nested addition to fail")

    class FailingPredicate:
        def __bool__(self):
            raise ValueError("truth")

    def choose(flag):
        return lambda result=(
            3
            if flag
            else 4
        ): result

    print("conditional", choose(True)())
    try:
        choose(FailingPredicate())
    except ValueError as exc:
        frame = traceback.extract_tb(exc.__traceback__)[-2]
        print(frame.name, (frame.lineno, frame.end_lineno, frame.colno, frame.end_colno))

    def defined(n):
        def read(where=(
            position()
        ), *, value=(
            100 + (2 + n)
        )):
            return where, value
        return read

    print("def bound", defined(3)())
    try:
        defined("bad")
    except TypeError as exc:
        frame = traceback.extract_tb(exc.__traceback__)[-1]
        print(frame.name, (frame.lineno, frame.end_lineno, frame.colno, frame.end_colno))

    def invoke(fn):
        return sys._getframe(1).f_lineno, fn()

    def enclosing():
        return invoke(
            lambda arg=(
                position()
            ): arg
        )

    print("restored", enclosing())

    class Method:
        @property
        def K(self):
            caller = sys._getframe(1)
            positions = inspect.getframeinfo(caller, context=0).positions
            print("lookup", caller.f_lineno, tuple(positions))
            return position

        # Each three-byte Roman numeral normalizes to four letters.
        ⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷ = K

    def continued(receiver):
        return lambda where=(
            receiver
            .K()
        ): where

    def expanded(receiver):
        return (
            receiver
            .ⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷⅷ(
            )
        )

    print("method", continued(Method())())
    print("no call columns", expanded(Method()))

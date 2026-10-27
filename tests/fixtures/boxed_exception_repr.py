class Missing(AttributeError):
    pass


class MissingKey(KeyError):
    pass


def current():
    for error in (ZeroDivisionError("divide"), UnicodeError("decode")):
        try:
            raise error
        except (ArithmeticError, ValueError) as caught:
            print(type(caught).__name__, isinstance(caught, ArithmeticError), isinstance(caught, ValueError))
    for error in (Exception(), ValueError("a'b"), TypeError(None), Missing("x", 7),
                  KeyError("missing"), MissingKey(""), KeyError(17)):
        print(repr(error), f"{error!r}", str(error), error)
    payload = [2]
    error = RuntimeError(payload)
    many = ValueError("x", payload)
    payload.append(3)
    print(repr(error), str(error), str(many))
    payload.append(error)
    print(repr(error))
    try:
        {}["missing"]
    except KeyError as caught:
        print(repr(caught))
    try:
        open("missing-exception-repr-fixture/file", "w")
    except FileNotFoundError as caught:
        print(repr(caught), caught.filename)

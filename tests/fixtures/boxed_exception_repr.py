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

    class Declared(ValueError):
        label = "class"

        @property
        def token(self):
            return self.label

        @token.setter
        def token(self, value):
            self.label = value

    error = Declared()
    print(error.label, error.token)
    saved = ["instance"]
    error.token = saved
    print(error.label is saved, error.token is saved)
    del error.label
    print(error.label, error.token)

    class Done(StopIteration):
        pass

    payload = ["original"]
    stopped = Done(payload, 42)
    print(stopped.value is payload, stopped.args[0] is payload)
    replacement = ["replacement"]
    stopped.value = replacement
    print(stopped.value is replacement, stopped.args[0] is payload)
    del stopped.value
    print(stopped.value is None, stopped.args[0] is payload)

    class Masked(Done):
        value = "class value"

    masked = Masked(payload)
    print(masked.value)
    masked.value = replacement
    print(masked.value is replacement)
    del masked.value
    print(masked.value)

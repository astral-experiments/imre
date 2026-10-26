class Missing(AttributeError):
    pass


def current():
    for error in (Exception(), ValueError("a'b"), TypeError(None), Missing("x", 7)):
        print(repr(error), f"{error!r}")
    payload = [2]
    error = RuntimeError(payload)
    payload.append(3)
    print(repr(error))
    payload.append(error)
    print(repr(error))
    try:
        {}["missing"]
    except KeyError as caught:
        print(repr(caught))
    try:
        open("missing-exception-repr-fixture/file")
    except FileNotFoundError as caught:
        print(repr(caught), caught.filename)

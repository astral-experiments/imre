def maybe(choose: bool) -> str | None:
    return "present" if choose else None


def flow(choose: bool) -> None:
    saved = 10
    print(choose and (saved := 20) > 0)
    print(saved)
    print(choose or (saved := 30) > 0)
    print(saved)
    print((saved := 40) if choose else (saved := 50))
    print(saved)
    print(5 < 4 < (saved := 60))
    print(saved)
    print(1 < 2 < (saved := 70))
    print(saved)
    if choose and (bound := 4) > 0:
        print(bound)
    if not choose or (other := 6) > 0:
        print("maybe bound")
    else:
        print(other)
    if (text := maybe(choose)) is not None:
        print(text + "!")
    else:
        print(text)
    optional: str | None = None
    if (optional := maybe(choose)) is None:
        print(optional)
    else:
        print(optional + "?")
    if (optional := maybe(True)) is not None and len(optional) > 0:
        print(optional)
    after: str | None = "ok"
    print(choose and (after := None) is None)
    try:
        print(after + "!")
    except TypeError:
        print("TypeError")
    count = 0
    while (count := count + 1) < 4:
        print(count)
    print(count)
    assert (verified := 4) > 0, str(saved := 99)
    print(verified)
    print(saved)
    try:
        assert False, str(saved := 100)
    except AssertionError as error:
        print(str(error))
    print(saved)


flow(False)
flow(True)

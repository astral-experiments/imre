def current():
    import sys
    from sys import intern

    assert intern is sys.intern
    assert sys._is_interned("jinja_token")
    letter = "é"
    assert intern(chr(233)) is letter

    fresh = "".join(["jinja_future_", "identifier"])
    known = intern(fresh)
    assert known is not fresh
    literal = "jinja_future_identifier"
    assert known is literal

    def word(n):
        return "live spelling " + str(n)

    first, second = word(721), word(721)
    assert first == second and first is not second
    assert not sys._is_interned(first)
    assert intern(first) is first and sys._is_interned(first)
    assert intern(second) is first and not sys._is_interned(second)
    aliases = [first]
    del first
    assert intern(second) is aliases[0]
    del aliases
    assert intern(second) is second and sys._is_interned(second)

    class Name(str):
        def __hash__(self):
            raise AssertionError("called subclass __hash__")

    subclass = Name("jinja_token")
    assert not sys._is_interned(subclass)
    for call in (lambda: intern(subclass), lambda: intern(b"word"), lambda: intern(None),
                 lambda: intern(), lambda: intern("one", "two"), lambda: intern(value="x")):
        try:
            call()
        except TypeError as error:
            print(str(error))
        else:
            raise AssertionError("accepted invalid sys.intern arguments")
    print("released canonical string and adopted a live equal string")

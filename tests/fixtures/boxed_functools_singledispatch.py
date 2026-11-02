from functools import singledispatch


def current():
    @singledispatch
    def describe(value):
        return "directory"

    class Link(str):
        pass

    @describe.register
    def text(value: str):
        return "text"

    @describe.register
    def raw(value: bytes):
        return "bytes"

    @describe.register
    def link(value: Link):
        return "link"

    assert (describe({}), describe(""), describe(b""), describe(Link(""))) == (
        "directory", "text", "bytes", "link"
    )
    assert describe.dispatch(Link) is link
    assert describe.registry[bytes] is raw
    try:
        describe.registry[bytes] = text
    except TypeError:
        pass
    else:
        raise AssertionError("mutable dispatch registry")

    class Child(Link):
        pass

    assert describe(Child("")) == "link"

    def updated(value):
        return "updated link"

    describe.register(Link, updated)
    assert describe(Child("")) == "updated link"

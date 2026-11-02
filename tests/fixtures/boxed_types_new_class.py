import types


def helpers():
    events = []

    class Expand:
        def __mro_entries__(self, bases):
            events.append(("resolve", bases))
            return (int, str)

    class Factory:
        def __prepare__(self, name, bases, **kwds):
            events.append(("prepare", name, bases, kwds))
            return {"prepared": True}

        def __call__(self, name, bases, namespace, **kwds):
            events.append(("factory", name, bases, kwds))
            return namespace

    def body(namespace):
        assert "__orig_bases__" not in namespace
        events.append(("body", namespace["prepared"]))
        namespace["payload"] = 42

    factory = Factory()
    kwds = {"metaclass": factory, "flag": 7}
    original = (Expand(),)
    namespace = types.new_class("Example", original, kwds, body)
    assert kwds == {"metaclass": factory, "flag": 7}
    assert namespace == {"prepared": True, "payload": 42, "__orig_bases__": original}
    assert events == [
        ("resolve", original), ("prepare", "Example", (int, str), {"flag": 7}),
        ("body", True), ("factory", "Example", (int, str), {"flag": 7}),
    ]
    bases = (int, str)
    assert types.resolve_bases(bases) is bases
    assert types.prepare_class("Example") == (type, {}, {})

    class Bad:
        def __mro_entries__(self, bases):
            return [int]

    events.clear()
    try:
        types.new_class("Bad", (Bad(),), kwds, body)
    except TypeError as exc:
        assert str(exc) == "__mro_entries__ must return a tuple"
    else:
        raise AssertionError("accepted invalid MRO entries")
    assert not events

    def failing_body(namespace):
        raise LookupError("body")

    try:
        types.new_class("Failed", (), kwds, failing_body)
    except LookupError as exc:
        assert exc.args == ("body",)
    else:
        raise AssertionError("class body error disappeared")
    assert events == [("prepare", "Failed", (), {"flag": 7})]


def current():
    helpers()

    class First:
        def first(self):
            return 1

    class Second:
        def second(self):
            return 2

    def fill(namespace):
        namespace["answer"] = 42

    result = types.new_class("Result", (First, Second), exec_body=fill)
    assert result.__bases__ == (First, Second)
    assert result().answer == 42 and result().first() + result().second() == 3
    assert types.new_class("Empty").__module__ == "types"

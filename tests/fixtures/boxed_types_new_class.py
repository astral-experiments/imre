import types
from types import get_original_bases


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


def metaclass_preparation():
    # The native type slot must take precedence even on the first lookup.
    class Later:
        @classmethod
        def __prepare__(cls, name, bases):
            raise AssertionError("looked past type in metaclass MRO")

    class Ordered(type, Later): pass

    assert types.prepare_class("Probe", (), {"metaclass": Ordered}) == (Ordered, {}, {})

    class Plain: pass
    class Default(type): pass
    class Child(metaclass=Default): pass

    assert type(Child) is Default
    assert Plain.__prepare__.__self__ is type
    assert Default.__prepare__.__self__ is Default
    assert Child.__prepare__.__self__ is Default
    assert super(Default, Default).__prepare__.__self__ is Default
    assert not hasattr(super(Plain, Plain), "__prepare__")
    first = type.__prepare__()
    assert first == {} and type.__prepare__("N", (), None, extra=True) is not first

    events = []

    class Meta(type):
        @classmethod
        def __prepare__(mcls, name, bases, **kw):
            ns = super().__prepare__(name, bases, **kw)
            events.append(("prepare", ns))
            return ns

        def __new__(mcls, name, bases, ns, **kw):
            events.append(("new", ns))
            return super().__new__(mcls, name, bases, ns)

    def body(ns):
        events.append(("body", ns))
        ns["answer"] = 42

    created = types.new_class("Created", (), {"metaclass": Meta, "tag": True}, body)
    assert type(created) is Meta and created.answer == 42
    assert [name for name, ns in events] == ["prepare", "body", "new"]
    assert events[0][1] is events[1][1] is events[2][1]

    class Disabled(type):
        __prepare__ = None

    try:
        types.new_class("Invalid", (), {"metaclass": Disabled}, body)
    except TypeError:
        pass
    else:
        raise AssertionError("called body after a non-callable preparation hook")
    assert len(events) == 3


def prepared_class_statements():
    events = []
    namespace = {"__name__": "prepared.module", "seed": 7}

    class Factory:
        def __prepare__(self, name, bases, *, marker):
            events.append(("prepare", name, bases, marker))
            return namespace

        def __call__(self, name, bases, ns, *, marker):
            assert ns is namespace and ns["value"] == 8
            assert events[0][1] is name and events[0][2] is bases
            events.append(("new", marker))
            return marker

    class Alias:
        def __hash__(self):
            return hash("deletable")
        def __eq__(self, other):
            return other == "deletable"

    namespace[Alias()] = 1
    marker = object()
    class Result(object, metaclass=Factory(), marker=marker):
        assert deletable == 1
        del deletable
        assert locals() is namespace and __module__ == __name__ == "prepared.module"
        value = seed + 1

    assert Result is marker and events[1] == ("new", marker)

    class Meta(type):
        @classmethod
        def __prepare__(meta, name, bases):
            return {"seed": 9}

    class Product(metaclass=Meta):
        value = seed
        field: (value, lambda: marker)
        def owner(self):
            return __class__

        def record(self):
            self.value: [0 for self.never in ()] = 1

        def ordered(self, other):
            ignored: {[self for other in ()]: [other for self in ()] for _ in ()}

    assert type(Product) is Meta and Product().owner() is Product and Product.value == 9
    assert Product.record.__code__.co_cellvars == ("self",)
    assert Product.ordered.__code__.co_cellvars == ("other",)
    assert Product.__static_attributes__ == ("value",)
    assert Product.__annotate__.__code__.co_freevars == ("__classdict__", "marker")
    value, read = Product.__annotations__["field"]
    assert value == 9 and read() is marker

    prepared = {"__name__": "local", "x": "prepared"}
    x = "outer"
    class ScopeMeta(type):
        @classmethod
        def __prepare__(meta, name, bases):
            return prepared

    # Synthetic metadata STOREs honor the same symbol table as the class suite.
    module = globals()
    missing = object()
    old = {key: module.get(key, missing) for key in ("__module__", "__static_attributes__")}
    try:
        class Scoped(metaclass=ScopeMeta):
            global __name__, __module__, __static_attributes__
            nonlocal x
            read = x
            x = "updated"
        assert Scoped.read == "prepared" and x == "updated"
        assert module["__module__"] == module["__name__"] and module["__static_attributes__"] == ()
        assert "__module__" not in prepared and "__static_attributes__" not in prepared
    finally:
        for key, previous in old.items():
            if previous is missing:
                module.pop(key, None)
            else:
                module[key] = previous

    annotations = {"seed": "retained"}
    prepared.clear()
    prepared.update(__name__="prepared", __annotations__=annotations)
    env = {"__name__": "source.module", "ScopeMeta": ScopeMeta}
    exec("class FromSource(metaclass=ScopeMeta):\n"
         "    'source docstring'\n"
         "    global __name__, __module__\n"
         "    recorded = locals()\n", env)
    source = env["FromSource"]
    assert source.recorded is prepared and prepared["__doc__"] == "source docstring"
    assert source.__annotations__ is annotations and "__module__" not in prepared
    assert env["__module__"] == env["__name__"]

    def returning_int(name, bases, namespace):
        return int

    try:
        class MissingCell(metaclass=returning_int):
            def owner(self):
                return __class__
    except RuntimeError as caught:
        assert "__class__ not set" in str(caught)
    else:
        raise AssertionError("accepted a type without its required class cell")

    error = RuntimeError("key collision")
    class Key:
        def __hash__(self):
            return hash("__module__")
        def __eq__(self, other):
            raise error

    class Failing(type):
        @classmethod
        def __prepare__(meta, name, bases):
            return {Key(): 1}

    try:
        class Failed(metaclass=Failing):
            raise AssertionError("ran a class body after a namespace error")
    except RuntimeError as caught:
        assert caught is error
    else:
        raise AssertionError("lost prepared dictionary exception")


def current():
    metaclass_preparation()
    prepared_class_statements()
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


    class Sequence(list[int]):
        pass

    class Child(Sequence):
        pass

    assert get_original_bases(Sequence) == (list[int],)
    assert get_original_bases(Child) == (Sequence,)
    assert get_original_bases(int) == (object,)
    assert get_original_bases(bool) == (int,)
    try:
        get_original_bases(object())
    except TypeError as exc:
        assert str(exc) == "Expected an instance of type, not 'object'"
    else:
        raise AssertionError("accepted a non-class")

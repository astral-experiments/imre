import gc
import types
from _weakref import ref

External = bytes
format = complex


class Owner:
    local = float

    def method(self, x: External, y: local, z: format):
        pass

    def constant(self, x: 7):
        pass

    def generic[T](self, x: T):
        pass

    def class_reference(self, x: __class__):
        return super()


class DuringDefinition:
    before = 1
    removed = 2

    def method(x: added):
        pass

    body = method.__annotate__.__closure__[0].cell_contents
    body["added"] = 3
    body["before"] = 4
    del body["removed"]
    seen = before


def module_function(x: External):
    pass


def enclosing(captured):
    def function(x: captured):
        pass

    class Nested:
        def method(self, x: captured):
            pass

    return function.__annotate__, Nested.method.__annotate__


def nested_expressions(External):
    class Local:
        External = "class"

        def method[T: External](x: [External for _ in (1,)], y: [x for x in [External]]):
            pass

    class Global:
        global External

        def method(x: External, y: (lambda: External)):
            pass

    bound = Local.method.__type_params__[0].evaluate_bound
    assert bound.__code__.co_freevars == ("__classdict__",)
    annotation = Local.method.__annotate__
    classdict = annotation.__code__.co_freevars.index("__classdict__")
    assert bound.__closure__[0] is annotation.__closure__[classdict]
    assert bound() == "class"
    Global.External = "shadow"
    return Local.method.__annotate__, Global.method.__annotate__


def no_enclosing_name():
    class Missing:
        only_here = 1

        def method(x: [only_here for _ in (1,)]):
            pass
    return Missing.method.__annotate__


def make_cell(value):
    def read():
        return value
    return read.__closure__[0]


def current():
    assert Owner.generic.__code__.co_flags == 19
    assert Owner.generic.__annotate__.__code__.co_flags == 19
    during = DuringDefinition.method.__annotate__
    assert DuringDefinition.seen == DuringDefinition.before == 4
    assert during(1) == {"x": 3}
    assert not hasattr(DuringDefinition, "removed")
    assert during.__closure__[0].cell_contents is not DuringDefinition.body
    assert DuringDefinition.body["__classdictcell__"] is during.__closure__[0]

    provider = Owner.method.__annotate__
    names = provider.__code__.co_freevars
    assert names == ("__classdict__",)
    assert provider.__code__.co_flags == 3
    cell = provider.__closure__[0]
    assert type(cell.cell_contents) is dict
    cell.cell_contents["from_annotation_dict"] = 7
    assert Owner.from_annotation_dict == 7
    del Owner.from_annotation_dict
    assert "from_annotation_dict" not in cell.cell_contents
    Owner.release_me = Owner()
    saved = ref(Owner.release_me)
    del cell.cell_contents["release_me"]
    gc.collect()
    assert saved() is None
    assert Owner.constant.__annotate__.__closure__[0] is cell
    assert provider(1) == {"x": bytes, "y": float, "z": complex}
    Owner.local = str
    assert provider(2)["y"] is str

    globals_copy = dict(provider.__globals__, External=tuple, local=int, format=bool, NotImplementedError=ValueError)
    copied = types.FunctionType(provider.__code__, globals_copy, closure=provider.__closure__)
    assert copied(2) == {"x": tuple, "y": str, "z": bool}
    replaced = types.FunctionType(provider.__code__, globals_copy,
                                  closure=(make_cell({"local": list}),))
    assert replaced(2) == {"x": tuple, "y": list, "z": bool}
    assert provider(2) == {"x": bytes, "y": str, "z": complex}

    for invalid in ((), (1,)):
        try:
            types.FunctionType(provider.__code__, globals_copy, closure=invalid)
        except (TypeError, ValueError):
            pass
        else:
            raise AssertionError("accepted invalid annotation closure")
    for unsupported in (3, 4):
        try:
            copied(unsupported)
        except NotImplementedError:
            pass
        else:
            raise AssertionError("accepted unsupported annotation format")

    self_ref = Owner.class_reference.__annotate__
    assert self_ref.__code__.co_freevars == ("__class__", "__classdict__")
    assert self_ref(2) == {"x": Owner}
    assert self_ref.__closure__[1] is cell
    class_cell = self_ref.__closure__[0]
    class_cell.cell_contents = 42
    try:
        Owner().class_reference(None)
    except RuntimeError:
        pass
    else:
        raise AssertionError("super accepted a non-type __class__ cell")
    class_cell.cell_contents = Owner

    module_provider = module_function.__annotate__
    assert module_provider.__closure__ is None
    assert module_provider.__code__.co_freevars == ()
    copied_module = types.FunctionType(module_provider.__code__, globals_copy)
    assert copied_module(2) == {"x": tuple}

    nested, method = enclosing(set)
    assert nested.__code__.co_freevars == ("captured",)
    assert nested.__code__.co_flags == 19
    assert method.__code__.co_freevars == ("__classdict__", "captured")
    assert method.__closure__[1] is nested.__closure__[0]
    assert nested(2) == method(2) == {"x": set}
    change = types.FunctionType(method.__code__, globals_copy,
                                closure=(make_cell({}), make_cell(dict)))
    assert change(2) == {"x": dict}
    missing = no_enclosing_name()
    assert missing.__code__.co_freevars == ("__classdict__",)
    try:
        missing(2)
    except NameError:
        pass
    else:
        raise AssertionError("comprehension captured a class field")

    local, declared_global = nested_expressions("outer")
    assert local(2) == {"x": ["outer"], "y": ["class"]}
    values = declared_global(2)
    assert values["x"] is bytes and values["y"]() == "outer"
    callback = values["y"]
    assert callback.__qualname__ == "nested_expressions.<locals>.Global.<lambda>"
    assert callback.__code__.co_qualname == callback.__qualname__
    assert callback.__code__.co_flags & 0x10  # CO_NESTED
    class Variadic:
        options = (float,)

        def select(self, *items: *(lambda values=[v for v in options]: values)()):
            pass

    assert Variadic.select.__annotations__ == {"items": float}
    print("annotation closures", names, copied(2)["x"].__name__, replaced(2)["y"].__name__)


if __name__ == "__main__":
    current()

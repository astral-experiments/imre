from typing import TypedDict, is_typeddict


def check_inheritance():
    from annotationlib import ForwardRef as AnnotationRef
    from operator import getitem
    from typing import Annotated, ForwardRef, TypeVarTuple, get_args, get_origin

    nested = Annotated[Annotated[str, "inner"], "outer"]
    assert nested.__args__ == (str,) and nested.__metadata__ == ("inner", "outer")
    assert get_origin(nested) is Annotated and get_args(nested) == (str, "inner", "outer")
    assert ForwardRef is AnnotationRef
    assert Annotated["Target", 1].__origin__ == ForwardRef("Target", is_class=True)
    assert nested.copy_with((int,)) == Annotated[int, "inner", "outer"]
    assert nested.__reduce__() == (getitem, (Annotated, (str, "inner", "outer")))
    pack = TypeVarTuple("Ts")
    packed = Annotated[pack, "unit"]
    assert packed.__parameters__ == (pack,)
    try:
        packed[int]
    except TypeError as exc:
        assert str(exc) == "Substitution of bare TypeVarTuple is not supported"
    else:
        raise AssertionError("substituted a bare TypeVarTuple")

    class Left(TypedDict):
        left: bool
        shared: int

    Right = TypedDict("Right", {"shared": str}, total=False)

    class Both(Left, Right):
        pass

    class Child(Both):
        shared: bytes

    assert Both.__bases__ == Child.__bases__ == (dict,)
    assert Both.__orig_bases__ == (Left, Right) and Child.__orig_bases__ == (Both,)
    assert Both.__required_keys__ == {"left"} and Both.__optional_keys__ == {"shared"}
    assert Both.__annotate__(1) == {"left": bool, "shared": str}
    assert Child.__required_keys__ == {"left", "shared"} and not Child.__optional_keys__
    assert Child.__annotations__ == {"left": bool, "shared": bytes}

    class ObserveNamespace:
        def __set_name__(self, owner, name):
            assert "__orig_bases__" not in owner.__dict__
            assert "__annotations__" not in owner.__dict__
            assert owner.__annotations__ == {"own": float}

    class Observed(Both):
        own: float
        descriptor = ObserveNamespace()

    assert Observed.__orig_bases__ == (Both,)
    assert Observed.__annotations__ == {"left": bool, "shared": str, "own": float}

    formats = []

    def annotate(format):
        formats.append(format)
        if len(formats) == 1:
            raise ValueError("try again")
        return {"late": format}

    Left.__annotate__ = annotate
    try:
        Both.__annotations__
    except ValueError as exc:
        assert str(exc) == "try again"
    else:
        raise AssertionError("ignored the parent annotator's error")

    cached = Both.__annotations__
    assert cached == {"late": 1, "shared": str}
    assert Both.__annotate__(3) == {"late": 3, "shared": str}
    assert Both.__annotate__(4) == {"late": 4, "shared": "str"}
    assert Both.__annotations__ is cached
    assert formats == [1, 1, 3, 4]


def check_required_keys():
    from typing import NotRequired, Required, get_args, get_origin

    nested = "(" * 200 + "T" + ")" * 200
    assert Required[nested].__args__[0].__forward_arg__ == nested
    try:
        Required["(" + nested + ")"]
    except SyntaxError:
        pass
    else:
        raise AssertionError("accepted 201 nested delimiters in a forward reference")

    for form in (Required, NotRequired):
        alias = form[int]
        assert alias is form[int] and get_origin(alias) is form
        assert alias.__args__ == get_args(alias) == (int,)
        assert form[None].__args__ == (type(None),)
        assert repr(alias) == f"typing.{form.__name__}[int]"
        for invalid in (Required, (int,), (int, str)):
            try:
                form[invalid]
            except TypeError:
                pass
            else:
                raise AssertionError("accepted an invalid qualifier argument")

    class ExplicitKeys:
        def __bool__(self):
            raise AssertionError("qualified fields consulted total")

    Parent = TypedDict("Parent", {"must": NotRequired[Required[int]], "may": NotRequired[str]},
                       total=ExplicitKeys())
    assert Parent.__required_keys__ == {"must"} and Parent.__optional_keys__ == {"may"}
    assert Parent.__annotations__["must"] is NotRequired[Required[int]]
    assert Parent.__annotate__(4)["may"] == "typing.NotRequired[str]"

    class Child(Parent, total=False):
        must: NotRequired[bytes]
        may: Required[None]
        extra: int

    assert Child.__required_keys__ == {"may"} and Child.__optional_keys__ == {"must", "extra"}
    assert Child.__annotations__ == {"must": NotRequired[bytes], "may": Required[None], "extra": int}
    assert Child() == {}


def check_generic_bases():
    from typing import Generic, TypeVar

    T = TypeVar("T")
    seen = []

    class Descriptor:
        def __set_name__(self, owner, name):
            seen.append("__parameters__" in owner.__dict__)

    class Row(TypedDict, Generic[T]):
        value: T
        descriptor = Descriptor()

    class Fixed(Row[int], total=False):
        extra: str

    assert seen == [False] and Row.__parameters__ == (T,)
    assert Fixed.__mro__ == (Fixed, Generic, dict, object)
    assert Fixed.__orig_bases__ == (Row[int],) and Fixed.__parameters__ == ()
    assert Fixed.__annotations__ == {"value": T, "extra": str}
    assert Fixed.__required_keys__ == {"value"} and Fixed.__optional_keys__ == {"extra"}
    assert type(Fixed()) is dict

    class Pair[K, V](TypedDict):
        first: K

    class Reversed[V, K](Pair[K, V]):
        second: V

    parent_key, _ = Pair.__type_params__
    value, key = Reversed.__type_params__
    assert (value.__name__, key.__name__) == ("V", "K")
    assert Reversed.__parameters__ == (value, key)
    assert Reversed.__orig_bases__ == (Pair[key, value], Generic[value, key])
    assert Reversed.__annotations__ == {"first": parent_key, "second": value}
    assert parent_key is not key


def check_deferred_fields():
    from annotationlib import Format
    from typing import Final, NotRequired, get_args

    class Row(TypedDict):
        field: NotRequired[Later]

    assert Row.__optional_keys__ == {"field"} and not Row.__required_keys__
    assert Row.__annotate__(Format.STRING) == {"field": "NotRequired[Later]"}
    ref, = get_args(Row.__annotate__(Format.FORWARDREF)["field"])
    assert ref.__forward_arg__ == "Later" and ref.__owner__ is Row
    assert ref.evaluate(globals={"Later": bytes}, locals={"Later": str}) is str
    try:
        Row.__annotations__
    except NameError:
        pass
    else:
        raise AssertionError("VALUE accepted an unbound closure cell")

    Later = Final
    try:
        Row.__annotations__
    except TypeError:
        pass
    else:
        raise AssertionError("VALUE accepted a bare Final")

    Later = int
    saved = Row.__annotations__
    assert saved == {"field": NotRequired[int]}
    Later = bytes
    assert Row.__annotations__ is saved
    assert Row.__annotate__(Format.VALUE) == {"field": NotRequired[bytes]}
    assert ref.evaluate(locals={"Later": str}) is bytes

    class Expressions(TypedDict):
        left: (a ** b) ** c
        right: a ** b ** -c
        unary: ~(a - b) & 1 << c

    expected = {
        "left": "(a ** b) ** c",
        "right": "a ** b ** (-c)",
        "unary": "~(a - b) & 1 << c",
    }
    assert Expressions.__annotate__(Format.STRING) == expected
    refs = Expressions.__annotate__(Format.FORWARDREF)
    for key, expr in expected.items():
        assert refs[key].__forward_arg__ == expr
        assert refs[key].__owner__ is Expressions


def check_exec_fields():
    parent_ns = {"TypedDict": TypedDict, "Alias": bytes}
    exec("from __future__ import annotations\nclass Parent(TypedDict):\n    __key: Alias", parent_ns)
    parent = parent_ns["Parent"]
    child_ns = {"Parent": parent, "Alias": str}
    exec("class Child(Parent):\n    value: Alias = 3", child_ns)
    child = child_ns["Child"]

    ref = parent.__annotations__["_Parent__key"]
    assert ref.__owner__ is parent and ref.evaluate(globals=parent_ns) is bytes
    assert child.value == 3 and child.__annotations__ == {"_Parent__key": ref, "value": str}
    child_ns["Alias"] = int
    assert child.__annotate__(1) == {"_Parent__key": ref, "value": int}


def current():
    check_inheritance()
    check_required_keys()
    check_generic_bases()
    check_deferred_fields()
    check_exec_fields()

    class Film(TypedDict):
        title: str
        year: int

    Alias = int

    class Late(TypedDict):
        field: Alias
        nothing: None

    Alias = str
    assert Late.__annotations__ == {"field": str, "nothing": type(None)}
    Alias = bytes
    assert Late.__annotations__["field"] is str
    assert Late.__annotate__(1)["field"] is bytes

    class Lookalike(dict):
        __required_keys__ = Film.__required_keys__
        __optional_keys__ = Film.__optional_keys__
        __total__ = True

    class Partial(TypedDict, total=False):
        __total__ = True
        year: int

    assert Partial.__total__ is False
    assert Partial.__optional_keys__ == {"year"} and not Partial.__required_keys__
    assert Partial(year=1979) == {"year": 1979}

    class Receiver:
        def __init_subclass__(cls, *, total):
            cls.received = total

    class Child(Receiver, total=None): pass
    assert Child.received is None

    namespace = {"TypedDict": TypedDict}
    exec("class Loaded(TypedDict, total=False): pass", namespace)
    assert namespace["Loaded"].__total__ is False

    import warnings
    with warnings.catch_warnings(record=True) as captured:
        warnings.simplefilter("always", DeprecationWarning)
        omitted = TypedDict("Omitted")
        explicit_none = TypedDict("ExplicitNone", None)
        explicit_empty = TypedDict("ExplicitEmpty", {})
    assert omitted.__annotations__ == explicit_none.__annotations__ == explicit_empty.__annotations__ == {}
    omitted_warning, none_warning = captured
    assert omitted_warning.category is none_warning.category is DeprecationWarning
    assert str(omitted_warning.message).startswith("Failing to pass a value for the 'fields' parameter")
    assert str(none_warning.message).startswith("Passing `None` as the 'fields' parameter")

    Draft = TypedDict("Draft", [("title", str)], total=False)
    assert is_typeddict(Film) and is_typeddict(tp=Draft)
    assert type(Film) is type(Draft) and type(Film).__name__ == "_TypedDictMeta"
    assert Film.__bases__ == Draft.__bases__ == (dict,)
    assert Film.__orig_bases__ == Draft.__orig_bases__ == (TypedDict,)
    assert Film.__annotations__ == {"title": str, "year": int}
    assert Film.__required_keys__ == frozenset(("title", "year"))
    assert Draft.__optional_keys__ == Draft.__mutable_keys__ == frozenset(("title",))
    assert not Draft.__required_keys__ and not Draft.__readonly_keys__
    assert Draft.__total__ is False

    class Name(str): pass
    name = Name("Nullable")
    Nullable = TypedDict(name, {"first": None, "second": type(None)})
    assert Nullable.__name__ is name and Nullable.__qualname__ is name
    assert Nullable.__annotations__ == {"first": type(None), "second": type(None)}
    assert Nullable.__annotate__(4) == {"first": "None", "second": "NoneType"}
    try:
        TypedDict("Bad\0Name", {})
    except ValueError:
        pass
    else:
        raise AssertionError("accepted null in type name")

    def make():
        return TypedDict("Created", {})

    for module in (None, "", False, "custom", 42):
        make.__module__ = module
        assert make().__module__ == (module or "__main__")

    class Alternating:
        calls = 0

        def __bool__(self):
            self.calls += 1
            return self.calls % 2 == 1

    total = Alternating()
    TypedDict("Empty", {}, total=total)
    assert total.calls == 0
    Mixed = TypedDict("Mixed", {"first": int, "second": int}, total=total)
    assert total.calls == 2
    assert Mixed.__required_keys__ == {"first"} and Mixed.__optional_keys__ == {"second"}

    sample = Film([("extra", 7)], title="Alien")
    assert type(sample) is dict and sample == {"extra": 7, "title": "Alien"}
    for value in (sample, Draft(), TypedDict, dict, Lookalike, Lookalike()):
        assert not is_typeddict(value)
    assert type(Lookalike()) is Lookalike
    for check in (lambda: isinstance(sample, Film), lambda: issubclass(dict, Draft)):
        try:
            check()
        except TypeError as exc:
            assert str(exc) == "TypedDict does not support instance and class checks"
        else:
            raise AssertionError("TypedDict accepted an instance or subclass check")
    observed = []

    class Observe:
        def __set_name__(self, owner, name):
            observed.append((is_typeddict(owner), type(owner()), owner.__call__))

    class Watched(TypedDict):
        field = Observe()

    assert observed == [(True, dict, dict)]
    meta = type(Watched)

    class NonData:
        def __get__(self, instance, owner):
            return instance, owner

    meta.probe = NonData()
    try:
        assert Watched.probe == (Watched, meta)
        Watched.probe = 7
        assert Watched.probe == 7
        meta.probe = property(lambda cls: cls)
        assert Watched.probe is Watched
    finally:
        del meta.probe
    print("TypedDict classes")

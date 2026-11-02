from typing import TypedDict, is_typeddict


def current():
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

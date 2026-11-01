import typing as t
from dataclasses import MISSING, asdict, dataclass, field, fields, replace


@dataclass(order=True)
class Model:
    count: int
    tags: "t.ClassVar[list]" = field(default=[])
    unset: "t.ClassVar[int]"
    dotted: " t. ClassVar.[int]"
    suffix: "t.ClassVar+"


class Other:
    Alias = t.ClassVar


class OriginLookalike:
    __origin__ = t.ClassVar


def current():
    first = Model(1)
    assert first.__dict__ == {"count": 1}
    assert [f.name for f in fields(first)] == ["count"]
    assert Model.__match_args__ == ("count",)
    assert repr(first) == "Model(count=1)"
    assert asdict(first) == {"count": 1}
    assert not hasattr(Model, "unset")
    tags = Model.__dataclass_fields__["tags"]
    assert tags.default is Model.tags and tags.kw_only is MISSING
    assert repr(tags._field_type) == "_FIELD_CLASSVAR"
    first.tags = ["private"]
    assert first == Model(1) and first < Model(2)
    try:
        Model(1, tags=[])
    except TypeError as exc:
        assert "unexpected keyword argument 'tags'" in str(exc)
    else:
        raise AssertionError("class variable unexpectedly accepted by initializer")

    @dataclass
    class Child(Model):
        count: "t.ClassVar[int]" = 9
        unset: int = 2

    child = Child()
    assert child.__dict__ == {"unset": 2}
    assert child.count == 9 and child.tags is Model.tags
    changed = replace(child, unset=3)
    assert changed.unset == 3 and child.unset == 2 and changed.tags is Model.tags
    try:
        replace(child, count=11)
    except TypeError as exc:
        assert "unexpected keyword argument 'count'" in str(exc)
    else:
        raise AssertionError("ClassVar was accepted by replace")
    assert [f.name for f in fields(Child)] == ["unset"]

    @dataclass
    class Ordinary:
        value: "t.ClassVarx[int]"
        unrelated: "Other.Alias[int]"
        lookalike: OriginLookalike

    assert asdict(Ordinary(3, 4, 5)) == {"value": 3, "unrelated": 4, "lookalike": 5}

    for options, message in (({"default_factory": list}, "field x cannot have a default factory"),
                             ({"kw_only": False}, "field x is a ClassVar but specifies kw_only")):
        try:
            @dataclass
            class Invalid:
                x: "t.ClassVar[int]" = field(**options)
        except TypeError as exc:
            assert str(exc) == message
        else:
            raise AssertionError("invalid ClassVar field options accepted")

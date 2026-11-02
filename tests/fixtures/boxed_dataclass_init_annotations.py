from dataclasses import InitVar, dataclass, field
from typing import ClassVar


class Target:
    pass


@dataclass
class Base:
    inherited: int = 1


@dataclass
class Model(Base):
    label: ClassVar[str] = "model"
    flag: bool = field(init=False, default=True)
    seed: InitVar[int] = 4
    required: "Target" = field(kw_only=True)
    items: list = field(default_factory=list)

    def __post_init__(self, seed):
        self.seed_value = seed


@dataclass(frozen=True)
class Simple:
    value: "Target"


@dataclass
class Mutable:
    """Keep the annotation callback unevaluated during decoration."""
    a: int
    b: bytes


@dataclass
class Custom:
    x: str

    def __init__(self, x: "Target"):
        self.x = x


@dataclass
class SelfField:
    self: int


def current():
    obj = Target()
    instance = Model(required=obj)
    assert instance.inherited == 1 and instance.flag and instance.seed_value == 4
    assert instance.items == [] and instance.required is obj
    init = Model.__init__
    assert init.__globals__ is globals()
    assert init.__module__ == __name__
    hints = init.__annotations__
    assert list(hints) == ["inherited", "seed", "required", "items", "return"]
    assert hints["inherited"] is int and hints["seed"] is Model.__annotations__["seed"]
    assert hints["required"] == "Target" and hints["items"] is list and hints["return"] is None
    assert Simple(obj).value is obj and Simple.__init__.__annotations__ == {"value": "Target", "return": None}
    assert Simple.__init__.__globals__["Target"] is Target
    Mutable.__annotations__ = {"a": str}
    assert Mutable.__init__.__annotations__ == {"a": str, "return": None}
    assert Custom(obj).x is obj and Custom.__init__.__annotations__ == {"x": "Target"}
    assert SelfField(self=8).self == 8
    from annotationlib import Format, ForwardRef
    from inspect import signature

    @dataclass
    class Deferred:
        value: Later
        count: int = field(kw_only=True, default=1)

    doc = "Deferred(value: Later, *, count: int = 1)"
    assert Deferred.__doc__ == doc
    forward = signature(Deferred, annotation_format=Format.FORWARDREF)
    assert isinstance(forward.parameters["value"].annotation, ForwardRef)
    assert forward.return_annotation is None
    try:
        signature(Deferred)
    except NameError:
        pass
    else:
        raise AssertionError("a missing annotation was cached as a value")
    globals()["Later"] = Target
    try:
        assert signature(Deferred).parameters["value"].annotation is Target
        assert Deferred.__doc__ == doc
    finally:
        del globals()["Later"]
    print("initializer annotations passed")

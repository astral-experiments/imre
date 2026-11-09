from annotationlib import Format, ForwardRef, get_annotations
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
    @dataclass
    class Deferred:
        value: Later
        category: ClassVar[Later]

    assert Deferred.__annotate__.__code__.co_flags == 19
    init = Deferred.__init__
    assert isinstance(get_annotations(init, format=Format.FORWARDREF)["value"], ForwardRef)
    assert get_annotations(init, format=Format.STRING) == {"value": "Later", "return": "None"}
    Later = int
    assert get_annotations(init) == {"value": int, "return": None}

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
    print("initializer annotations passed")

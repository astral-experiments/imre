from dataclasses import MISSING, InitVar, dataclass as record, field, make_dataclass
from inspect import getattr_static, signature


def dataclass(cls):
    cls.decorated = True
    return cls


@dataclass
class Ordinary:
    units: int


@record(frozen=True, slots=True)
class Invoice:
    self: int
    adjustment: InitVar[int] = 0
    entries: list = field(default_factory=list, kw_only=True)

    def __post_init__(instance, adjustment):
        instance.entries.append(instance.self + adjustment)


def current():
    assert Ordinary.decorated and "__init__" not in vars(Ordinary)
    create = Invoice.__init__
    assert list(signature(create).parameters) == [
        "__dataclass_self__", "self", "adjustment", "entries"
    ]
    assert create.__code__.co_varnames == ("__dataclass_self__", "self", "adjustment", "entries")
    assert create.__code__.co_cellvars == ()
    assert getattr_static(create.__code__, "co_cellvars") is type(create.__code__).co_cellvars
    one = Invoice(self=7)
    two = Invoice(8, 3)
    assert one.entries == [7] and two.entries == [11]
    supplied = []
    assert Invoice(4, entries=supplied).entries is supplied and supplied == [4]
    Row = make_dataclass("Row", [(key, int) for key in ("value", "count")])
    assert Row(12, count=2).count == 2
    # Field options are evaluated by the decorator. The finalized signature
    # can differ from what was visible during its first default-order check.
    final_field = field(default=2)

    class ChangeDefault:
        calls = 0

        def __bool__(self):
            self.calls += 1
            if self.calls == 2:
                final_field.default = MISSING
            return True

    try:
        @record
        class Invalid:
            first: int = field(default=1, init=ChangeDefault())
            last: int = final_field
    except (TypeError, SyntaxError):
        pass
    else:
        raise AssertionError("accepted a positional default before a required argument")
    print("dataclass constructors passed")

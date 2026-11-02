import typing
from typing import dataclass_transform


def current():
    assert typing.dataclass_transform is dataclass_transform

    def model(): ...

    class Base: ...

    class Meta(type): ...

    marker = object()
    fields = (model, Base)
    decorate = dataclass_transform(eq_default=marker, order_default=True,
                                   frozen_default=True, field_specifiers=fields, custom=marker)
    assert decorate(model) is model
    assert decorate(cls_or_fn=Base) is Base
    assert decorate(Meta) is Meta
    first = model.__dataclass_transform__
    assert first == {
        "eq_default": marker, "order_default": True, "kw_only_default": False,
        "frozen_default": True, "field_specifiers": fields, "kwargs": {"custom": marker},
    }
    assert first["field_specifiers"] is fields
    assert first is not Base.__dataclass_transform__
    assert first["kwargs"] is Base.__dataclass_transform__["kwargs"]
    assert dataclass_transform()(model) is model
    assert model.__dataclass_transform__ == {
        "eq_default": True, "order_default": False, "kw_only_default": False,
        "frozen_default": False, "field_specifiers": (), "kwargs": {},
    }

    try:
        dataclass_transform(False)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a positional option")

    try:
        decorate(int)
    except TypeError as exc:
        assert str(exc) == "cannot set '__dataclass_transform__' attribute of immutable type 'int'"
    else:
        raise AssertionError("wrote an attribute on an immutable class")

    class Reject:
        def __setattr__(self, name, value):
            assert name == "__dataclass_transform__"
            raise ValueError("read only")

    try:
        decorate(Reject())
    except ValueError as exc:
        assert str(exc) == "read only"
    else:
        raise AssertionError("suppressed attribute setter error")
    print("dataclass_transform")

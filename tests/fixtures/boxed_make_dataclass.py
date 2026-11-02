from dataclasses import field, fields, make_dataclass


def create():
    return make_dataclass("Item", [("value", int)])


def current():
    annotation_cache()
    namespace = {"label": "compiled", "value": 9}
    cls = make_dataclass("Item", [("value", int, field(default=3))],
                         namespace=namespace, kw_only=True)
    assert cls.__module__ == __name__ and cls.__bases__ == (object,)
    assert namespace["value"] == 9 and cls.value == 3
    assert cls().value == 3 and cls(value=8).value == 8
    assert fields(cls)[0].name == "value" and cls.label == "compiled"

    create.__module__ = "custom"
    made = create()
    assert made.__module__ == "custom"
    assert made.__annotations__ == {"value": int}
    fresh = made.__annotate__(1)
    fresh.clear()
    assert made.__annotate__(3) == {"value": int}
    print("factory class and callback passed")


def annotation_cache():
    supplied = {'label': str}
    cls = make_dataclass('Override', [('value', int)], namespace={'__annotations__': supplied})
    assert cls.__annotations__ is supplied
    assert cls.__annotate__(1) == {'value': int}
    assert cls('abc').label == 'abc'
    cls.__annotations__ = {'new': float}
    assert cls.__annotations__ == {'new': float} and cls.__annotate__ is None
    del cls.__annotations__
    assert cls.__annotations__ == {}

    def decorator(cls, **options):
        assert '__annotations__' not in cls.__dict__
        original = cls.__annotations__
        assert original == {'value': int}
        cls.__annotate__ = lambda format: {'name': str}
        assert cls.__annotations__ == {'name': str}
        assert cls.__annotations__ is not original
        return cls

    assert make_dataclass('Changed', [('value', int)], decorator=decorator).__annotations__ == {'name': str}

from dataclasses import field, fields, make_dataclass


def create():
    return make_dataclass("Item", [("value", int)])


def current():
    implicit_annotations()
    annotation_cache()
    factory_bases()
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


def implicit_annotations():
    import sys
    from annotationlib import Format, ForwardRef

    assert 'typing' not in sys.modules
    cls = make_dataclass('Record', ['value', ('nullable', None)])
    ref = ForwardRef('Any', module='typing')
    assert [f.type for f in fields(cls)] == [ref, None]
    assert cls.__annotate__(Format.FORWARDREF) == {'value': ref, 'nullable': None}
    assert cls.__annotate__(Format.STRING) == {'value': 'typing.Any', 'nullable': 'None'}
    assert 'typing' not in sys.modules
    value = cls.__annotations__['value']
    assert value is sys.modules['typing'].Any
    assert cls.__annotate__(Format.FORWARDREF)['value'] is value


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


def factory_bases():
    events = []

    class Marker:
        def __set_name__(self, owner, name):
            events.append((owner, name))

    class Base:
        def __init_subclass__(cls):
            events.append((cls, "hook"))

    class Other:
        pass

    cls = make_dataclass("Derived", [("n", int)], bases=(Base, Other),
                         namespace={"marker": Marker()})
    assert events == [(cls, "marker"), (cls, "hook")]
    assert cls.__bases__ == (Base, Other)
    assert cls.__mro__ == (cls, Base, Other, object)
    assert isinstance(cls(3), Base) and cls(3).n == 3

    class Alias:
        def __mro_entries__(self, bases):
            return (Other,)

    alias = Alias()
    child = make_dataclass("Child", [], bases=(alias,))
    assert child.__orig_bases__ == (alias,) and isinstance(child(), Other)

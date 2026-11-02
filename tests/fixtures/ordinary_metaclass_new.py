from typing import get_type_hints


events = []
token = object()


class Meta(type):
    def __new__(meta, name, bases, namespace, *, marker):
        events.append((meta, name, bases, namespace))
        namespace["value"] = marker
        return super().__new__(meta, name, bases, namespace)


class Model(metaclass=Meta, marker=token):
    value: object

    def owner(self):
        return __class__


assert type(Model) is Meta and Model().owner() is Model
assert Model.value is token and get_type_hints(Model) == {"value": object}
assert len(events) == 1
meta, name, bases, namespace = events[0]
assert meta is Meta and name == "Model" and bases == ()
assert namespace["value"] is token
namespace["value"] = None
assert Model.value is token
assert not hasattr(Model, "_abc_impl")


class Derived(Model, marker=token):
    pass

assert events[-1][:3] == (Meta, "Derived", (Model,))
assert type(Derived) is Meta


class Replacing(type):
    def __new__(meta, name, bases, namespace):
        return token


class Replaced(metaclass=Replacing):
    pass

assert Replaced is token

failure = RuntimeError("construction failed")


class Raising(type):
    def __new__(meta, name, bases, namespace):
        raise failure


try:
    class Failed(metaclass=Raising):
        pass
except RuntimeError as caught:
    assert caught is failure
else:
    raise AssertionError("ignored a metaclass exception")

try:
    class MissingKeyword(metaclass=Meta):
        pass
except TypeError:
    pass
else:
    raise AssertionError("ignored a missing constructor argument")

for args in [(), (type,), (int, "Bad", (), {}), (type, "Bad", [], {}), (type, "Bad", (), [])]:
    try:
        type.__new__(*args)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted an invalid type.__new__ call")

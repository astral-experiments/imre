import sys

events = []


def mark(value):
    events.append(value)
    return value


def configure():
    original = sys.platform
    sys.platform = "runtime-platform"
    return original


class Service:
    original = configure()
    if sys.platform == "runtime-platform":
        def label(self, value=mark("selected")):
            return value
    else:
        def other(self, value=mark("inactive")):
            return value


def make_spec():
    from spec_factory import create
    return create("example")


sys.platform = Service.original
assert Service().label() == "selected"
assert not hasattr(Service, "other")
assert events == ["selected"]
print(make_spec().name)

from abc import ABC
from functools import singledispatch, singledispatchmethod


def current():
    @singledispatch
    def describe(value):
        return "directory"

    class Link(str):
        pass

    @describe.register
    def text(value: str):
        return "text"

    @describe.register
    def raw(value: bytes):
        return "bytes"

    @describe.register
    def link(value: Link):
        return "link"

    assert (describe({}), describe(""), describe(b""), describe(Link(""))) == (
        "directory", "text", "bytes", "link"
    )
    assert describe.dispatch(Link) is link
    assert describe(None) == "directory"
    assert describe.dispatch(type(None)) is describe.registry[object]
    assert describe.registry[bytes] is raw
    try:
        describe.registry[bytes] = text
    except TypeError:
        pass
    else:
        raise AssertionError("mutable dispatch registry")

    class Child(Link):
        pass

    assert describe(Child("")) == "link"

    def updated(value):
        return "updated link"

    describe.register(Link, updated)
    assert describe(Child("")) == "updated link"

    class Event(ABC):
        pass

    class PluginEvent:
        pass

    class Renderer:
        @singledispatchmethod
        def render(self, value):
            return "unknown"

        @render.register(Event)
        def render_event(self, value):
            return "event"

    render = Renderer().render
    value = PluginEvent()
    assert render(value) == "unknown"
    Event.register(PluginEvent)
    assert render(value) == "event"

    assert Event.__subclasses__() == []  # Registration does not create a nominal subclass.

    class First(PluginEvent):
        pass

    class Second(PluginEvent):
        pass

    class ChildEvent(First):
        pass

    type.__subclasses__(PluginEvent).clear()  # Each call returns a fresh list.
    assert PluginEvent.__subclasses__() == [First, Second]
    try:
        First.__bases__ = (ChildEvent,)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted an inheritance cycle")
    assert PluginEvent.__subclasses__() == [First, Second]

    class Other:
        pass

    First.__bases__ = (PluginEvent, Other)
    assert PluginEvent.__subclasses__() == [Second, First]
    assert Other.__subclasses__() == [First]
    First.__bases__ = (Other,)
    assert PluginEvent.__subclasses__() == [Second]

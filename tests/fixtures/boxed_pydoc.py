import inspect
import pydoc
import types


class DispatchWindow:
    """A range of dispatch times."""


def route_label(route, width=4):
    """Format a route number for the dispatch board."""
    return f"R-{route:0{width}d}"


def current():
    assert pydoc.locate(__name__ + ".DispatchWindow") is DispatchWindow
    function = pydoc.locate(__name__ + ".route_label")
    assert function is route_label
    print(function(12))
    assert pydoc.locate(__name__ + ".unassigned") is None
    descriptor = DispatchWindow.__dict__["__dict__"]
    assert type(descriptor) is types.GetSetDescriptorType
    assert inspect.isgetsetdescriptor(descriptor)
    assert not inspect.isgetsetdescriptor(function)
    print(pydoc.render_doc(function, renderer=pydoc.plaintext))


if __name__ == "__main__":
    current()

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
    kind = types.GetSetDescriptorType
    doc = kind.__doc__
    assert type(doc) is kind and vars(kind)["__doc__"] is doc
    assert doc.__objclass__ is kind and doc.__name__ == "__doc__"
    assert doc.__doc__ is None and doc.__get__(None, kind) is doc
    assert doc.__get__(descriptor) == pydoc.getdoc(descriptor) == "dictionary for instance variables"
    assert doc.__get__(int.numerator) == int.numerator.__doc__
    from datetime import date
    assert doc.__get__(date.year) is date.year.__doc__ is None
    import struct
    for name, expected in (("format", "struct format string"), ("size", "struct size in bytes")):
        field = vars(struct.Struct)[name]
        assert field is getattr(struct.Struct, name) and type(field) is kind
        assert doc.__get__(field) == field.__doc__ == expected

    class Aliases:
        first = second = doc

    assert Aliases.first is Aliases.second is doc
    for operation in (lambda: doc.__get__(object()),
                      lambda: doc.__set__(object(), 1),
                      lambda: doc.__delete__(object()),
                      lambda: Aliases().first):
        try:
            operation()
        except TypeError as exc:
            assert "descriptor '__doc__' for 'getset_descriptor' objects" in str(exc)
        else:
            raise AssertionError("accepted an unrelated descriptor receiver")
    for operation in (lambda: doc.__set__(descriptor, "new"), lambda: doc.__delete__(doc)):
        try:
            operation()
        except AttributeError as exc:
            assert str(exc) == "attribute '__doc__' of 'getset_descriptor' objects is not writable"
        else:
            raise AssertionError("writable getset descriptor documentation")
    assert inspect.isgetsetdescriptor(descriptor)
    assert not inspect.isgetsetdescriptor(function)
    print(pydoc.render_doc(function, renderer=pydoc.plaintext))


if __name__ == "__main__":
    current()

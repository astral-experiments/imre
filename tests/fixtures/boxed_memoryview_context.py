from array import array


def current():
    data = array('i', [3, 5])
    view = memoryview(data)
    with view as entered:
        assert entered is view and entered.nbytes == 2 * data.itemsize
    data.append(7)
    try:
        with view:
            raise AssertionError("entered a released view")
    except ValueError as error:
        assert str(error) == "operation forbidden on released memoryview object"

    other = memoryview(b"abc")
    copy = memoryview(other)
    try:
        with other:
            raise RuntimeError("inside view")
    except RuntimeError as error:
        assert str(error) == "inside view"
    assert bytes(copy) == b"abc"
    assert memoryview.__exit__(other) is None
    assert memoryview.__exit__(copy, 1, 2, 3, 4) is None
    try:
        bytes(other)
    except ValueError:
        pass
    else:
        raise AssertionError("view survived exceptional exit")
    python_buffer()
    print("memoryview context")


def python_buffer():
    import weakref

    class Exporter:
        def __init__(self, child):
            self.child = child
            self.flags = []

        def __buffer__(self, flags):
            self.flags.append(flags)
            return self.child

    data = array("h", [11, 22, 33, 44])
    child = memoryview(data)[1::2].toreadonly()
    exporter = Exporter(child)
    ref = weakref.ref(exporter)
    view = memoryview(exporter)
    token = view.obj
    assert view is not child and token is not exporter and token is not child and token is not data
    assert view.format == child.format and view.strides == child.strides and view.readonly
    data[3] = 55
    assert (view[0], view[1]) == (22, 55)
    other = memoryview(exporter)
    assert other.obj is not token and exporter.flags == [284, 284]
    derived = view[1:]
    del exporter
    assert ref() is not None and derived.obj is token
    view.release()
    other.release()
    assert derived[0] == 55
    try:
        child.release()
    except BufferError as exc:
        assert str(exc) == "memoryview has 1 exported buffer"
    else:
        raise AssertionError("released a view exported by a live child")
    derived.release()
    assert type(token).__name__ == "_buffer_wrapper"
    assert ref() is None and child[0] == 22
    child.release()

    for invalid, error, message in (
        (b"bad", TypeError, "__buffer__ returned non-memoryview object"),
        (child, ValueError, "operation forbidden on released memoryview object"),
    ):
        try:
            memoryview(Exporter(invalid))
        except error as exc:
            assert str(exc) == message
        else:
            raise AssertionError("accepted an invalid __buffer__ result")

    class Refuse:
        def __buffer__(self, flags):
            raise LookupError("export failed")

    try:
        memoryview(Refuse())
    except LookupError as exc:
        assert str(exc) == "export failed"
    else:
        raise AssertionError("lost the exporter's exception")

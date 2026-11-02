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
    print("memoryview context")

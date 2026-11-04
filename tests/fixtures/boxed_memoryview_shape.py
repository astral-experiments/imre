import base64
from array import array
from collections import namedtuple


def current():
    data = bytearray(b"<abcdefghijkl>")
    with memoryview(data)[1:-1] as raw:
        view = raw.cast("B", (3, 4))
    assert view.obj is data and view.shape == (3, 4)
    assert (len(view), view.ndim, view.itemsize, view.nbytes) == (3, 2, 1, 12)
    assert view.strides == (4, 1) and view.c_contiguous and not view.f_contiguous
    assert view.tobytes("F") == b"aeibfjcgkdhl"
    assert base64.b64encode(view) == b"YWJjZGVmZ2hpamts"
    coordinate = namedtuple("Coordinate", "row column")
    assert view[coordinate(1, 2)] == ord("g")
    view[coordinate(-1, -1)] = ord("!")
    assert data[-2] == ord("!")
    view[-1, -1] = ord("L")
    assert data == b"<abcdefghijkL>"
    with view[::-2] as rows:
        assert rows.shape == (2, 4) and rows.strides == (-8, 1)
        assert not rows.contiguous and rows[1, -2] == ord("c")
        assert rows.tobytes() == b"ijkLabcd" and rows.tobytes("F") == b"iajbkcLd"
        try:
            base64.b64encode(rows)
        except BufferError:
            pass
        else:
            raise AssertionError("encoded non-contiguous rows")
        try:
            rows.cast("B")
        except TypeError:
            pass
        else:
            raise AssertionError("cast non-contiguous rows")

    with view.cast("I") as words:
        assert words.shape == (3,) and words.tobytes() == bytes(data[1:-1])
        words[0] = 1234
        assert data[1:5] == array("I", [1234]).tobytes()
    readonly = view.toreadonly()
    view.release()
    assert readonly[-1, -1] == ord("L")
    try:
        readonly[0, 0] = 0
    except TypeError:
        pass
    else:
        raise AssertionError("wrote through a readonly shaped view")
    readonly.release()
    data.append(0)

    with memoryview(b"abcdefghijkl").cast("B", [2, 2, 3]) as cube:
        assert cube.strides == (6, 3, 1) and cube[-1, 0, 2] == ord("i")
        assert cube.tobytes("F") == b"agdjbhekcifl"
        for shape in ((12, 1), (1, 0), (5,)):
            try:
                cube.cast("B", shape)
            except TypeError:
                pass
            else:
                raise AssertionError("accepted incompatible cast shape")

    scalar = memoryview(bytearray(b"x")).cast("B", ())
    assert scalar.shape == scalar.strides == () and scalar.ndim == 0
    for operation in (len, iter):
        try:
            operation(scalar)
        except TypeError:
            pass
        else:
            raise AssertionError("treated a scalar as a sequence")
    scalar[()] = ord("y")
    assert scalar[()] == ord("y") and scalar.tobytes() == b"y"
    exporter = bytearray(b"ab")
    with memoryview(exporter) as flat:
        items = iter(flat)
        assert type(items).__iter__(items) is items
        assert items.__next__() == ord("a")
        exporter[1] = ord("z")
        assert next(items) == ord("z")
    assert next(items, None) is None
    exporter.append(0)

    with memoryview(exporter) as flat:
        items = memoryview.__iter__(flat)
        assert type(items).__next__(items) == ord("a")
    try:
        next(items)
    except ValueError:
        pass
    else:
        raise AssertionError("read another element from a released view")
    print("shaped memoryview")

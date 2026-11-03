import _tracemalloc
import gc
import sys
import tracemalloc


def make():
    line = sys._getframe().f_lineno + 1
    obj = bytearray(32768)
    return obj, line


def nested():
    call_line = sys._getframe().f_lineno + 1
    obj, line = make()
    return obj, call_line, line


def literal_origins():
    lookup = tracemalloc.get_object_traceback
    assert lookup(chr(229)) is None
    assert lookup(int("256")) is None
    assert lookup(b"x") is None
    for empty in (bytes(), bytes(0), bytes([]), tuple(), tuple(iter([]))):
        assert lookup(empty) is None
    sample = b"ABC"
    canonical = b"A"
    for cached in (bytes([65]), bytes(iter([65])), sample[:1], "A".encode("ascii")):
        assert cached is canonical and lookup(cached) is None
    cached_zero = bytes([0])
    line = sys._getframe().f_lineno + 1
    allocated_zero = bytes(1)
    assert cached_zero == allocated_zero and cached_zero is not allocated_zero
    assert lookup(cached_zero) is None and lookup(allocated_zero)[-1].lineno == line
    assert allocated_zero[:] is allocated_zero

    class Packet(bytes):
        pass

    short = Packet(allocated_zero)
    assert bytes(short) is cached_zero and short[:] is cached_zero
    long = Packet(b"AB")
    line = sys._getframe().f_lineno + 1
    left, right = long[:], long[:]
    assert type(left) is bytes and left == right and left is not right
    assert lookup(left)[-1].lineno == line and lookup(right)[-1].lineno == line
    first, second = bytes(long), bytes(long)
    assert type(first) is bytes and first == second and first is not second

    class Custom(Packet):
        def __bytes__(self):
            return allocated_zero

    assert bytes(Custom()) is allocated_zero
    line = sys._getframe().f_lineno + 1
    from_tuple = bytes((65,))
    assert lookup(from_tuple)[-1].lineno == line
    line = sys._getframe().f_lineno + 1
    from_view = bytes(memoryview(b"A"))
    assert lookup(from_view)[-1].lineno == line
    line = sys._getframe().f_lineno + 1
    stepped = sample[0:1:2]
    assert lookup(stepped)[-1].lineno == line
    assert lookup("late materialization: literal") is None
    assert lookup(93172654) is None
    assert lookup((93172654, "constant tuple")) is None
    line = sys._getframe().f_lineno + 1
    text = ": ".join(["late materialization", "literal"])
    assert text == "late materialization: literal"
    assert lookup(text)[-1].lineno == line
    line = sys._getframe().f_lineno + 1
    number = int("109317265400000000000000000000000000000000000000001")
    assert lookup(number)[-1].lineno == line
    gc.collect()
    line = sys._getframe().f_lineno + 1
    compiled = compile("(93172654321, ('runtime literal text', b'xyz'))", "created_during_tracing.py", "eval")
    constants = eval(compiled)
    assert lookup(constants)[-1].lineno == line
    assert lookup(constants[0])[-1].lineno == line
    assert lookup(constants[1])[-1].lineno == line


def current():
    tracemalloc.stop()
    before = object()
    earlier = compile("(70024833, 'pre-existing')", "compiled_before_tracing.py", "eval")
    tracemalloc.start(2)
    try:
        assert tracemalloc.start is _tracemalloc.start
        assert tracemalloc.is_tracing()
        assert tracemalloc.get_object_traceback(eval(earlier)) is None
        literal_origins()
        assert tracemalloc.get_object_traceback(before) is None
        assert tracemalloc.get_object_traceback(()) is None
        obj, call_line, line = nested()
        trace = tracemalloc.get_object_traceback(obj)
        assert isinstance(trace, tracemalloc.Traceback)
        assert all(isinstance(frame, tracemalloc.Frame) for frame in trace)
        assert [(frame.filename, frame.lineno) for frame in trace] == [
            (__file__, call_line), (__file__, line)
        ]
        assert trace[-1].lineno == line and trace.total_nframe is None
        assert trace.format(1) == [f'  File "{__file__}", line {line}', '    obj = bytearray(32768)']
        assert _tracemalloc._get_object_traceback(obj) == (
            (__file__, line), (__file__, call_line)
        )
        tracemalloc.start(1)
        assert tracemalloc.get_traceback_limit() == 2
        assert tracemalloc.get_object_traceback(obj) == trace
        tracemalloc.clear_traces()
        assert tracemalloc.is_tracing() and tracemalloc.get_object_traceback(obj) is None
    finally:
        tracemalloc.stop()
    assert not tracemalloc.is_tracing() and tracemalloc.get_traceback_limit() == 2
    for depth in (0, 65536):
        try:
            tracemalloc.start(depth)
        except ValueError as exc:
            assert str(exc) == "the number of frames must be in range [1; 65535]"
        else:
            raise AssertionError("invalid traceback depth accepted")

from inspect import getinnerframes, trace


def read_row(row: dict[str, int]):
    return row["value"]


def report():
    try:
        read_row({})
    except KeyError as error:
        frames = trace()
        assert [frame.function for frame in frames] == ["report", "read_row"]
        for frame, line in zip(frames, ("read_row({})", 'return row["value"]')):
            assert frame.code_context is not None and frame.index is not None
            assert frame.code_context[frame.index].strip() == line

        tb = error.__traceback__
        assert tb is not None
        for frame in getinnerframes(tb, context=0):
            assert tb is not None
            assert frame.frame is tb.tb_frame
            assert frame.lineno == tb.tb_lineno == frame.positions.lineno
            assert frame.code_context is None and frame.index is None
            tb = tb.tb_next
        assert tb is None


report()
assert trace() == []

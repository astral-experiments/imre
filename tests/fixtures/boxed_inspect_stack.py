import inspect


def snapshot():
    return inspect.stack(3)


def current():
    # The non-ASCII literal distinguishes byte offsets from character offsets.
    label, records = "é", snapshot(
    )
    import dis

    caller = inspect.currentframe()
    inner, outer = records[:2]
    assert label == "é"
    assert isinstance(inner, inspect.FrameInfo) and len(inner) == 6
    assert inner.frame.f_back is caller and outer.frame is caller
    assert inner.frame.f_globals is globals()
    assert inner.function == "snapshot" and outer.function == "current"
    assert inner.filename == outer.filename == __file__

    line = inner.code_context[inner.index]
    assert line.strip() == "return inspect.stack(3)"
    assert isinstance(inner.positions, dis.Positions)
    assert inner.positions == (inner.lineno, inner.lineno, line.index("inspect"), len(line.rstrip()))
    first = outer.code_context[outer.index].encode()
    last = outer.code_context[outer.index + 1].rstrip().encode()
    assert outer.positions == (outer.lineno, outer.lineno + 1, first.index(b"snapshot("), len(last))
    assert last.strip() == b")"

    no_context = inspect.getframeinfo(inner.frame, context=0)
    assert len(no_context) == 5 and no_context.code_context is no_context.index is None
    assert no_context.function == "snapshot"
    namespace = {"inspect": inspect}
    exec(compile("record = inspect.stack()[0]", "<inspect-stack-no-source>", "exec"), namespace)
    assert namespace["record"].code_context is namespace["record"].index is None

    try:
        1 / 0
    except ZeroDivisionError as exc:
        trace = inspect.getframeinfo(exc.__traceback__)
    assert isinstance(trace, inspect.Traceback)
    assert trace.code_context[trace.index].strip() == "1 / 0"
    assert trace.positions.lineno == trace.lineno

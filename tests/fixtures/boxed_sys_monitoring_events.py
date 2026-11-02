def current():
    import sys
    from sys import monitoring
    from types import ModuleType, SimpleNamespace

    assert type(monitoring) is ModuleType
    assert monitoring is sys.__dict__["monitoring"]
    assert monitoring.__name__ == "sys.monitoring"
    assert monitoring.__doc__ is monitoring.__package__ is None
    assert monitoring.__loader__ is monitoring.__spec__ is None
    assert "sys.monitoring" not in sys.modules
    try:
        import sys.monitoring
    except ModuleNotFoundError:
        pass
    else:
        raise AssertionError("sys.monitoring unexpectedly importable")

    exports = "DISABLE MISSING events DEBUGGER_ID COVERAGE_ID PROFILER_ID OPTIMIZER_ID".split()
    assert [name for name in vars(monitoring) if name in exports] == exports

    events = monitoring.events
    assert type(events) is SimpleNamespace
    names = (
        "PY_START PY_RESUME PY_RETURN PY_YIELD CALL LINE INSTRUCTION JUMP "
        "BRANCH_LEFT BRANCH_RIGHT STOP_ITERATION RAISE EXCEPTION_HANDLED "
        "PY_UNWIND PY_THROW RERAISE C_RETURN C_RAISE BRANCH NO_EVENTS"
    ).split()
    assert list(events.__dict__) == names
    for i, name in enumerate(names[:-1]):
        assert getattr(events, name) == 1 << i
    assert events.NO_EVENTS == 0
    assert (monitoring.DEBUGGER_ID, monitoring.COVERAGE_ID,
            monitoring.PROFILER_ID, monitoring.OPTIMIZER_ID) == (0, 1, 2, 5)
    assert type(monitoring.DISABLE) is type(monitoring.MISSING) is object
    assert monitoring.DISABLE is not monitoring.MISSING

    original = events.LINE
    try:
        events.LINE = -1
        assert sys.monitoring.events.LINE == -1
    finally:
        events.LINE = original

    del sys.modules["sys"]
    try:
        fresh = __import__("sys")
        assert fresh is not sys
        assert not hasattr(fresh, "monitoring")
        assert sys.monitoring is monitoring and monitoring.events is events
    finally:
        sys.modules["sys"] = sys

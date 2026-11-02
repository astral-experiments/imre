def current():
    import time
    from time import get_clock_info
    from types import SimpleNamespace

    class Name(str):
        def __str__(self):
            raise AssertionError("get_clock_info called __str__")

    for name in ("time", "monotonic", "perf_counter", "process_time", "thread_time"):
        info = get_clock_info(Name(name))
        assert type(info) is SimpleNamespace
        assert list(vars(info)) == ["implementation", "monotonic", "adjustable", "resolution"]
        assert info.monotonic is (name != "time")
        assert info.adjustable is (name == "time")
        assert type(info.resolution) is float and 0 < info.resolution <= 1
        assert type(info.implementation) is str and info.implementation
        info.resolution = None
        assert time.get_clock_info(name).resolution is not None

    assert get_clock_info("perf_counter") == get_clock_info("monotonic")
    for name, error in ((b"time", TypeError), ("time_ns", ValueError),
                        ("time\0", ValueError), ("x\ud800", UnicodeEncodeError)):
        try:
            get_clock_info(name)
        except error:
            pass
        else:
            raise AssertionError("invalid clock accepted")

    prefix = "get_clock_info() argument 1 must be str, not "
    for kind, expected in (("x" * 60, TypeError), ("界" * 17, UnicodeDecodeError)):
        try:
            get_clock_info(type(kind, (), {})())
        except expected as exc:
            if expected is TypeError:
                assert str(exc) == prefix + kind[:50]
            else:
                assert exc.object == prefix.encode() + kind.encode()[:50]
                assert exc.reason == "unexpected end of data"
        else:
            raise AssertionError("invalid clock type accepted")

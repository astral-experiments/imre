import _warnings
import sys
import warnings


def current():
    assert warnings.warn is _warnings.warn
    assert warnings.warn_explicit is _warnings.warn_explicit
    original = sys.modules.pop("_warnings")
    try:
        fresh = __import__("_warnings")
        assert fresh is not original
        assert fresh.warn.__self__ is fresh
        assert original.warn.__self__ is original
        assert not hasattr(fresh.warn, "__code__")
        assert "warn_explicit" in dir(fresh)
        assert fresh.warn_explicit.__doc__ == "Issue a warning, or maybe ignore it or raise an exception."
        assert fresh._release_lock.__text_signature__ == "($module, /)"
        assert fresh.filters is original.filters
        assert fresh._onceregistry is original._onceregistry
        assert fresh._warnings_context is original._warnings_context
        fresh._acquire_lock()
        try:
            original._filters_mutated_lock_held()
        finally:
            original._release_lock()
        try:
            fresh._filters_mutated_lock_held()
        except RuntimeError as error:
            assert str(error) == "warnings lock is not held"
        else:
            raise AssertionError("filter version changed without the lock")
    finally:
        sys.modules["_warnings"] = original

    registry = {}
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("default")
        for lineno in (7, 7, 8):
            assert _warnings.warn_explicit("again", UserWarning, "input.py", lineno,
                                           registry=registry) is None
        line = sys._getframe().f_lineno + 1
        assert _warnings.warn("caller", UserWarning) is None
    assert [(w.filename, w.lineno) for w in caught] == [
        ("input.py", 7), ("input.py", 8), (__file__, line),
    ]
    assert all(w.category is UserWarning for w in caught)
    assert [str(w.message) for w in caught] == ["again", "again", "caller"]

    with warnings.catch_warnings():
        warnings.simplefilter("error")
        assert _warnings.warn_explicit("ignored", UserWarning, "input.py", 7, module=None) is None
        message = UserWarning("raised")
        try:
            _warnings.warn_explicit(message, UserWarning, "input.py", 7)
        except UserWarning as error:
            assert error is message
        else:
            raise AssertionError("error filter did not raise the supplied warning")

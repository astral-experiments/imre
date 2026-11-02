import gc
import importlib
import sys
import weakref


def current():
    # Indirect imports also initialize the native module state.
    name = "".join(("_loc", "ale"))
    first = importlib.import_module(name)
    setter = first.setlocale
    original_error = first.Error
    assert original_error.__doc__ is None
    assert "__weakref__" in vars(original_error)
    assert setter.__self__ is first
    assert (original_error.__module__, original_error.__name__) == ("locale", "Error")
    try:
        del sys.modules[name]
        second = importlib.import_module(name)
        assert second.Error is not original_error
        assert second.setlocale.__self__ is second
        new_setter = second.setlocale
        new_type = weakref.ref(second.Error)
        del second.Error, second.setlocale
        assert not hasattr(second, "setlocale")
        del sys.modules[name], second
        first.Error = RuntimeError
        gc.collect()
        assert new_type() is not None
        for f, expected in ((setter, original_error), (new_setter, new_type())):
            try:
                f(-500)
            except Exception as exc:
                assert type(exc) is expected
                assert exc.args == ("locale query failed",)
            else:
                raise AssertionError("invalid category accepted")
    finally:
        sys.modules[name] = first
        first.Error = original_error

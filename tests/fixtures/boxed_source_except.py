"""A runtime-loaded module can recover from an optional import."""

import gc
import sys


class Catch(tuple):
    def __iter__(self):
        raise AssertionError("except invoked custom iteration")


class ExitRecorder:
    def __init__(self, suppress):
        self.suppress = suppress

    def __enter__(self):
        return self

    def __exit__(self, cls, error, tb):
        self.error = error
        return self.suppress


def current():
    namespace = {"optional_errors": Catch((ImportError,))}
    exec("""
import sys
try:
    import _imre_missing_optional_plugin
except optional_errors:
    import os as backend
else:
    raise AssertionError("unexpected plugin")

def try_with(manager, error):
    try:
        with manager:
            raise error
    except LookupError:
        return sys.exception()
    else:
        return None

def suspended():
    try:
        raise KeyError("missing")
    except ValueError:
        raise AssertionError("wrong handler")
    except (LookupError, ImportError):
        error = sys.exception()
        yield error
        assert sys.exception() is error
        try:
            raise ValueError("nested")
        except ValueError:
            assert sys.exception().__context__ is error
        assert sys.exception() is error
    assert sys.exception() is None
    yield 42
""", namespace)

    assert namespace["backend"].__name__ == "os"
    failure = KeyError("original")
    for suppress in (False, True):
        manager = ExitRecorder(suppress)
        result = namespace["try_with"](manager, failure)
        assert manager.error is failure
        assert result is (None if suppress else failure)
    stream = namespace["suspended"]()
    error = next(stream)
    assert isinstance(error, KeyError) and error.args == ("missing",)
    assert sys.exception() is None
    gc.collect()
    assert next(stream) == 42
    assert next(stream, None) is None

    try:
        exec("try: raise ValueError()\nexcept (ValueError, 1): pass", {})
    except TypeError as error:
        assert isinstance(error.__context__, ValueError)
    else:
        raise AssertionError("invalid except target accepted")

    filename = "<handlers>"
    code = "try: raise ValueError()\nexcept KeyError: pass\nexcept: caught = True"
    exec(compile(code, filename, "exec"), namespace)
    assert namespace["caught"] is True
    try:
        compile("try: pass\nexcept: pass\nexcept ValueError: pass", filename, "exec")
    except SyntaxError as error:
        assert "default 'except:' must be last" in str(error)
    else:
        raise AssertionError("bare handler accepted before a typed handler")

    print("source exception handlers passed")


if __name__ == "__main__":
    current()

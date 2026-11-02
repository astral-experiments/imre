"""A runtime-loaded module can recover from an optional import."""

import abc
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


def bound_handlers():
    failure = KeyError("observed")
    namespace = {"sys": sys, "lookup": failure, "previous": 42}
    exec("""def run(failure, events, behavior):
    try:
        try:
            raise failure
        except KeyError as error:
            assert error is failure
            if behavior == "return":
                return error
            if behavior == "raise":
                raise
            error = 17
    finally:
        try:
            events.append(error)
        except UnboundLocalError:
            events.append("cleared")

def loop(failure, events):
    for action in [1, 2]:
        try:
            raise failure
        except KeyError as error:
            events.append(error)
            if action == 1:
                continue
            break
    try:
        events.append(error)
    except UnboundLocalError:
        events.append("cleared")

def suspended(failure, events):
    try:
        try:
            raise failure
        except KeyError as error:
            yield error
            assert sys.exception() is error
            raise
    finally:
        try:
            events.append(error)
        except UnboundLocalError:
            events.append("cleared")

try:
    raise lookup
except KeyError as previous:
    assert previous is lookup
""", namespace)
    assert "previous" not in namespace
    run = namespace["run"]
    for action in ["fallthrough", "return", "raise"]:
        events = []
        try:
            result = run(failure, events, action)
        except KeyError as error:
            assert action == "raise" and error is failure
        else:
            assert action != "raise"
            assert result is (failure if action == "return" else None)
        assert events == ["cleared"]
    events = []
    namespace["loop"](failure, events)
    assert events == [failure, failure, "cleared"]
    for action in ["resume", "close", "throw"]:
        events = []
        g = namespace["suspended"](failure, events)
        assert next(g) is failure
        if action == "close":
            g.close()
        else:
            replacement = ValueError("replacement")
            try:
                if action == "throw":
                    g.throw(replacement)
                else:
                    next(g)
            except (KeyError, ValueError) as error:
                assert error is (replacement if action == "throw" else failure)
            else:
                raise AssertionError("lost exception")
        assert events == ["cleared"]
        assert sys.exception() is None


def loop_else_finally():
    namespace = {}
    exec("""def select(callback, events):
    try:
        for value in (0, 1):
            try:
                callback(value)
                break
            except LookupError:
                events.append(value)
        else:
            raise IndexError("no selection")
    except IndexError:
        events.append("exhausted")
    else:
        events.append("selected")
    finally:
        events.append("release")
    yield events

def exit_from_else(events):
    try:
        events.append("body")
    except LookupError:
        events.append("handler")
    else:
        yield "ready"
        raise KeyError("else failure")
    finally:
        events.append("release")
""", namespace)

    def miss(value):
        raise KeyError(value)

    def second(value):
        if value == 0:
            miss(value)

    for select, expected in ((second, [0, "selected", "release"]),
                             (miss, [0, 1, "exhausted", "release"])):
        assert list(namespace["select"](select, [])) == [expected]
    events = []
    iterator = namespace["exit_from_else"](events)
    assert next(iterator) == "ready"
    try:
        next(iterator)
    except KeyError as exc:
        assert exc.args == ("else failure",)
    else:
        raise AssertionError("else error was swallowed")
    assert events == ["body", "release"]


def current():
    bound_handlers()
    loop_else_finally()
    namespace = {"abc": abc, "optional_errors": Catch((ImportError,))}
    exec("""
import sys
class Virtual(Exception, metaclass=abc.ABCMeta):
    pass
class Real(Exception):
    pass
Virtual.register(Real)
assert issubclass(Real, Virtual)
try:
    raise Real()
except Virtual:
    raise AssertionError("except matched a virtual subclass")
except Real:
    pass
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

    code = "try: raise ValueError()\nexcept KeyError: pass\nexcept: caught = True"
    exec(compile(code, "<handlers>", "exec"), namespace)
    assert namespace["caught"] is True
    try:
        compile("try: pass\nexcept: pass\nexcept LookupError: pass", "<invalid>", "exec")
    except SyntaxError as error:
        assert "default 'except:' must be last" in str(error)
    else:
        raise AssertionError("compiled a non-final bare except")

    print("source exception handlers passed")


if __name__ == "__main__":
    current()

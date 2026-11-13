import sys
from types import FrameType, TracebackType


def total(data):
    return data['values']


def dispatch():
    return total({})


def log_exception():
    kind, exc, tb = sys.exc_info()
    print(kind is KeyError, exc is sys.exception(), tb is exc.__traceback__)
    first = tb
    while tb is not None:
        print(type(tb) is TracebackType, isinstance(tb.tb_frame, FrameType), tb.tb_lineno)
        tb = tb.tb_next
    return first


def wsgi():
    try:
        dispatch()
    except KeyError as error:
        head = log_exception()
        try:
            raise ValueError('inner')
        except ValueError as inner:
            print(sys.exception() is inner, sys.exc_info()[2] is inner.__traceback__)
        print(sys.exception() is error, sys.exc_info()[2] is head)
        return error


class Manager:
    def __enter__(self):
        return self

    def __exit__(self, kind, error, tb):
        print(kind is KeyError, error is sys.exception(), tb is error.__traceback__)
        return True


print(sys.exc_info())
error = wsgi()
head = error.__traceback__
print(head is error.__traceback__, head.tb_frame is head.tb_frame, head.tb_next is head.tb_next)
print(sys.exc_info())
print(KeyError().__traceback__ is None)
with Manager():
    total({})
print(sys.exception() is None)


def reraises():
    try:
        total({})
    except KeyError as error:
        first = error.__traceback__
        try:
            raise
        except KeyError as bare:
            print(bare is error, bare.__traceback__ is first)
        try:
            raise error
        except KeyError as explicit:
            extended = explicit.__traceback__
            print(explicit is error, extended.tb_next is first, extended.tb_frame is first.tb_frame)


def reraised_from_callee():
    raise


def inherited():
    try:
        total({})
    except KeyError as error:
        first = error.__traceback__
        try:
            reraised_from_callee()
        except KeyError as propagated:
            extended = propagated.__traceback__
            print(propagated is error, extended.tb_next is first, extended.tb_frame is first.tb_frame)


reraises()
inherited()


def repeated_raise():
    error = ValueError()
    previous = None
    for _ in range(2):
        try:
            raise error
        except ValueError:
            head = error.__traceback__
            print(head.tb_next is previous, head.tb_lineno)
            previous = head


repeated_raise()

try:
    reraised_from_callee()
except RuntimeError as empty:
    tb = empty.__traceback__
    print(str(empty), tb.tb_next.tb_lineno, tb.tb_next.tb_next is None)


def keep_traceback():
    try:
        total({})
    except KeyError as error:
        caught = sys.exc_info()
        for i in range(5000):
            allocation = [i]
        print(caught[1] is sys.exception(), caught[2] is error.__traceback__)
        return caught[2]


saved = keep_traceback()
for i in range(5000):
    allocation = [i]
print(type(saved.tb_next) is TracebackType, type(saved.tb_next.tb_frame) is FrameType)


def current_error_type():
    assert sys.exception() is sys.exc_info()[1]
    return type(sys.exception())


try:
    raise KeyError("outer matcher")
except KeyError as outer:
    try:
        raise ValueError("inner matcher")
    except current_error_type():
        print("selected", str(sys.exception()))
    print(sys.exception() is outer)


def make(base, marker):
    class Reused(base):
        pass
    Reused.marker = marker
    return Reused


first = make(ValueError, 1)
second = make(KeyError, 2)
for error in (first("left"), second("right")):
    print(type(error) is first, type(error) is second,
          isinstance(error, (second, first)), isinstance(error, ValueError),
          isinstance(error, KeyError), isinstance(error, BaseException))
    try:
        raise error
    except (second, RuntimeError) as caught:
        print("second", caught is error, str(caught))
    except first as caught:
        print("first", caught is error, str(caught))

try:
    raise first
except second:
    print("wrong")
except first as caught:
    print("class", type(caught) is first)

held = make(Exception, 3)("earlier")
replacement = make(Exception, 4)
for n in range(12000):
    garbage = [n]
print("retained", type(held).marker, type(held) is replacement)

builtin_value_error = ValueError
class ValueError(Exception):
    pass
native = builtin_value_error("native")
print("builtin", type(native) is builtin_value_error, isinstance(native, ValueError))
try:
    raise native
except ValueError:
    print("wrong builtin")
except builtin_value_error:
    print("native handler")


class Unrelated:
    def __init__(self):
        raise AssertionError("invalid raise constructed an unrelated class")


for target, cause in ((Unrelated, None), (ValueError, Unrelated), (ValueError, print)):
    try:
        raise target from cause
    except TypeError as error:
        print(type(error).__name__, str(error))


args_error = RuntimeError("before")
original = args_error.args
print(original is args_error.args)
new_args = ([], 2)
args_error.args = new_args
print(args_error.args is new_args, str(args_error))
args_error.args = ["after"]
print(args_error.args, repr(args_error))
try:
    del args_error.args
except TypeError as error:
    print(str(error))

recursive = RuntimeError()
recursive.args = (recursive,)
for render in (str, repr):
    try:
        render(recursive)
    except RecursionError:
        print("recursive exception arguments")
recursive.args = ("recovered",)
print(str(recursive))

# Each list retains the next exception through its args tuple.
deep_argument_error = ValueError("leaf")
for _ in range(1500):
    outer_error = ValueError()
    outer_error.args = ([deep_argument_error],)
    deep_argument_error = outer_error
print(len(str(deep_argument_error)), len(repr(deep_argument_error)))


# ImportError retains its original message separately from its replaceable args.
for initial in ("before", ["marker"]):
    imported = ImportError(initial)
    replacement = ("after",)
    imported.args = replacement
    print(imported.msg is initial, imported.args is replacement, str(imported), repr(imported))

imported = ImportError.__new__(ImportError, "allocation")
print(imported.msg, imported.args)
module_name = ["module"]
ImportError.__init__(imported, "initial", name=module_name)
BaseException.__init__(imported, "replacement")
print(imported.msg, imported.args, str(imported), imported.name is module_name)
imported.__init__("bound")
print(imported.msg, imported.args, imported.name)

# A loader error keeps its original args even when msg is written first.
from importlib import import_module

try:
    import_module("missing_" + "boxed_args_package.child")
except ImportError as imported:
    imported.msg = "replacement"
    print(str(imported), imported.args)


# Clearing completed frames releases their locals while leaving the traceback chain intact.
import traceback
from _weakref import ref


class Local:
    pass


def fail_with_local(refs):
    local = Local()
    refs.append(ref(local))
    raise LookupError()


def check_clear():
    refs = []
    try:
        fail_with_local(refs)
    except LookupError as error:
        tb = error.__traceback__
        tail = tb.tb_next
        assert refs[0]() is not None
        try:
            tb.tb_frame.clear()
        except RuntimeError:
            pass
        else:
            raise AssertionError("cleared an executing frame")
        traceback.clear_frames(tb)
        assert refs[0]() is None
        assert tb.tb_next is tail
        assert error.with_traceback(None) is error and error.__traceback__ is None
        try:
            raise error.with_traceback(tail)
        except LookupError as raised:
            assert raised is error and raised.__traceback__.tb_next is tail


check_clear()


class FailingExit:
    def __enter__(self):
        return self

    def __exit__(self, *exc):
        print("exit caller", sys._getframe(1).f_lineno)
        raise ValueError()


def exit_locations(mode):
    try:
        with (
            FailingExit(),
            FailingExit(),
        ):
            if mode == "return":
                return
            if mode == "raise":
                raise KeyError()
    except ValueError as exc:
        while exc is not None:
            print(type(exc).__name__, exc.__traceback__.tb_lineno)
            exc = exc.__context__


for mode in ("normal", "return", "raise"):
    exit_locations(mode)


# Explicit causes are writable independently of raising, including cycles.
error = Exception("error")
error.__context__ = KeyError("context")
error.__cause__ = error
assert error.__cause__ is error and error.__suppress_context__
try:
    error.__cause__ = Exception
except TypeError as problem:
    assert str(problem) == "exception cause must be None or derive from BaseException"
else:
    raise AssertionError("accepted an exception class as the cause")
assert error.__cause__ is error
error.__cause__ = None
assert error.__cause__ is None and error.__suppress_context__
assert isinstance(error.__context__, KeyError)
try:
    del error.__cause__
except TypeError as problem:
    assert str(problem) == "__cause__ may not be deleted"
else:
    raise AssertionError("deleted the cause")


# Re-raising preserves a cause installed on the caught instance.
linked = KeyError("cause")
try:
    try:
        raise error
    except Exception as caught:
        caught.__cause__ = linked
        raise
except Exception as reraised:
    assert reraised is error and reraised.__cause__ is linked


class OwnCause(Exception):
    @property
    def __cause__(self):
        return self.link

    @__cause__.setter
    def __cause__(self, value):
        self.link = value


custom = OwnCause()
custom.__cause__ = 42
assert custom.__cause__ == 42 and not custom.__suppress_context__


class Payload:
    pass


def attach_payload(error):
    import weakref
    payload = Payload()
    error.__cause__ = Exception(payload)
    return weakref.ref(payload)


ref = attach_payload(error)
assert ref() is not None
error.__cause__ = None
assert ref() is None


from traceback import format_tb

last_frame, = format_tb(saved, limit=-1)
assert "in total" in last_frame
assert "return data['values']" in last_frame
assert format_tb(saved, limit=0) == []


def report_pull(items):
    try:
        for item in (
            items
        ):
            print("pulled", item)
    except Exception as error:
        print(type(error).__name__, error.__traceback__.tb_lineno)


report_pull(map(int, ("7", "bad")))

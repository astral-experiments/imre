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

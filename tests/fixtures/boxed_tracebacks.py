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

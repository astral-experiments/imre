import gc
from traceback import TracebackException
from types import FunctionType


def format_name_error(call):
    try:
        call()
    except NameError as error:
        return "".join(TracebackException.from_exception(error).format_exception_only())
    raise AssertionError("expected a NameError")


def misspelled_builtin():
    return prinr


assert format_name_error(misspelled_builtin) == (
    "NameError: name 'prinr' is not defined. Did you mean: 'print'?\n"
)


def suspended():
    yield shiping


captured = {"shipping": "ready"}
namespace = {"__builtins__": captured}
generator = FunctionType(suspended.__code__, namespace)()
namespace["__builtins__"] = {}
frame = generator.gi_frame
assert frame.f_builtins is captured
assert format_name_error(lambda: next(generator)) == (
    "NameError: name 'shiping' is not defined. Did you mean: 'shipping'?\n"
)

try:
    frame.f_builtins = {}
except AttributeError as error:
    assert str(error) == "attribute 'f_builtins' of 'frame' objects is not writable"
else:
    raise AssertionError("replaced the frame's builtins")

del captured
frame.clear()
gc.collect()
assert frame.f_builtins == {"shipping": "ready"}
frame.f_builtins["shipping"] = "sent"
assert frame.f_builtins["shipping"] == "sent"

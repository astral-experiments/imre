import io
import sys
from traceback import print_exception


class Writer:
    def __init__(self):
        self.chunks = []

    def write(self, text):
        self.chunks.append(text)


def missing(data):
    return data['value']


def call():
    return missing({})


def capture(exc, **options):
    stream = Writer()
    print_exception(exc, file=stream, **options)
    print(repr(stream.chunks))


try:
    call()
except KeyError as original:
    tb = original.__traceback__
    print(tb.tb_frame.f_code.co_name, tb.tb_frame.f_code.co_filename == __file__)
    print(tb.tb_next.tb_frame.f_code is call.__code__)
    print(tb.tb_next.tb_next.tb_frame.f_code is missing.__code__)
    capture(original, limit=-1)
    for cause in (original, None):
        try:
            raise RuntimeError('outer') from cause
        except RuntimeError as outer:
            print(outer.__context__ is original, outer.__cause__ is cause, outer.__suppress_context__)
            capture(outer, limit=0)
            capture(outer, limit=0, chain=False)

def nested_error():
    class Local(ValueError):
        __module__ = "builtins"
    return Local("inside function")


class Outer:
    class Local(ValueError):
        __module__ = "builtins"


for error in (nested_error(), Outer.Local("inside class")):
    print(type(error).__qualname__)
    capture(error, limit=0)


print_exception(None, file=Writer())
print(sys.exception() is None)


# A static write method can remove the last Python reference to the default file.
previous_stderr = sys.stderr
chunks = []


class ReplacingWriter:
    @staticmethod
    def write(text):
        chunks.append(text)
        sys.stderr = previous_stderr
        for item in range(12000):
            temporary = [item, item]


sys.stderr = ReplacingWriter()
try:
    print_exception(ValueError("retained writer"))
finally:
    sys.stderr = previous_stderr
assert chunks == ["ValueError: retained writer\n", ""]


class Pretty(Exception):
    def __str__(self):
        return "formatted text"

class Broken(Exception):
    def __str__(self):
        raise LookupError("while writing")

for err in (Pretty("raw payload"), Broken("raw payload")):
    try:
        print("str", str(err))
    except BaseException as failure:
        print("str raised", type(failure).__name__)
    stream = io.StringIO()
    print_exception(err, file=stream, limit=0)
    print("line", stream.getvalue().strip())


class Ordered(Exception):
    def __str__(self):
        calls.append(self.args[0])
        return self.args[0]


try:
    try:
        raise Ordered("inner")
    except Ordered as inner:
        raise Ordered("outer") from inner
except Ordered as outer:
    for chain in (True, False):
        calls = []
        output = io.StringIO()
        print_exception(outer, limit=0, file=output, chain=chain)
        print(calls, "Ordered: inner" in output.getvalue(), sys.exception() is outer)

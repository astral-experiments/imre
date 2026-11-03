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


class Renames(Exception):
    __module__ = "builtins"

    def __str__(self):
        Renames.__qualname__ = "AfterRender"
        return "renamed"


stream = io.StringIO()
print_exception(Renames(), limit=0, file=stream)
assert stream.getvalue() == "AfterRender: renamed\n"

class ModuleName(str):
    def __eq__(self, other):
        visits.append(other)
        return str(self) == other

    def __add__(self, other):
        visits.append(other)
        return str(self) + other


for module in ("package.errors", "__main__", None, ModuleName("package.errors")):
    Outer.Local.__module__ = module
    visits = []
    capture(Outer.Local("with module"), limit=0)
    print(visits)


from traceback import print_stack


def stack_messages(depth, limit):
    if depth:
        return stack_messages(depth - 1, limit)
    stream = io.StringIO()
    print_stack(limit=limit, file=stream)
    return stream.getvalue()


assert stack_messages(0, 1).endswith("    print_stack(limit=limit, file=stream)\n")
assert "[Previous line repeated 2 more times]" in stack_messages(5, 6)
assert sum(line.startswith('  File "') for line in stack_messages(5, -1).splitlines()) == 1


# A frame with no line number has a header, no source text and no repetition marker.
class NoLine:
    f_code = stack_messages.__code__
    f_lineno = None
    f_globals = {}

    def __init__(self, back):
        self.f_back = back


frame = None
for index in range(5):
    frame = NoLine(frame)
stream = io.StringIO()
print_stack(frame, file=stream)
assert stream.getvalue().count('line None, in stack_messages\n') == 5


# Capture the walk here even though another frame consumes it later.
from traceback import FrameSummary, StackSummary, walk_stack


def captured_walk(marker):
    return walk_stack(None)


summary = StackSummary.extract(captured_walk('live'), limit=1,
                               capture_locals=True, lookup_lines=False)
assert isinstance(summary, StackSummary) and len(summary) == 1
frame = summary[0]
assert isinstance(frame, FrameSummary) and frame.name == 'captured_walk'
assert frame.locals['marker'] == "'live'"
assert frame.line == 'return walk_stack(None)'
assert 'in captured_walk' in summary.format()[0]


def saved_closure(value):
    def inner():
        binding = value
        return sys._getframe()
    return inner()


saved = saved_closure(17)
summary = StackSummary.extract([(saved, saved.f_lineno)], capture_locals=True)
assert summary[0].locals == {'binding': '17', 'value': '17'}
saved.clear()
assert StackSummary.extract([(saved, 1)], capture_locals=True)[0].locals is None

from traceback import extract_tb, walk_tb


def divide_by_zero():
    return 1 / 0


def extract_failure():
    try:
        divide_by_zero()
    except ZeroDivisionError as exc:
        return exc.__traceback__


tb = extract_failure()
summary = extract_tb(tb)
assert isinstance(summary, StackSummary)
assert all(isinstance(frame, FrameSummary) for frame in summary)
assert [(frame.name, frame.line) for frame in summary] == [
    ('extract_failure', 'divide_by_zero()'), ('divide_by_zero', 'return 1 / 0')
]
assert extract_tb(tb, limit=1) == summary[:1]
assert extract_tb(tb, limit=-1) == summary[-1:]
assert extract_tb(tb, limit=0) == extract_tb(None) == []
assert summary[-1].lineno == tb.tb_next.tb_lineno

assert list(walk_tb(None)) == []
assert list(walk_tb(tb)) == [(tb.tb_frame, tb.tb_lineno),
                             (tb.tb_next.tb_frame, tb.tb_next.tb_lineno)]
view = tb.tb_next.tb_frame.f_locals
view["request_id"] = 17
captured = StackSummary.extract(walk_tb(tb), capture_locals=True)
view["request_id"] = 42
assert captured[-1].locals == {"request_id": "17"}

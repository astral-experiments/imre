"""Compare exception propagation and cleanup with CPython, including sanitizer runs."""

import argparse
import ast
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from textwrap import dedent


def upstream_support_payloads() -> str:
    """Run the original CPython exception classes, including their constructors and methods."""
    tests = Path(__file__).resolve().parent
    support = tests.parent / "benchmarks/cpython_stdlib/full/shared/upstream/Lib/test/support/__init__.py"
    source = support.read_text()
    names = ["Error", "TestFailed", "TestFailedWithDetails", "TestDidNotRun"]
    classes = [node for node in ast.parse(source).body if isinstance(node, ast.ClassDef) and node.name in names]
    assert [node.name for node in classes] == names
    fragments = [ast.get_source_segment(source, node) for node in classes]
    calls = (tests / "fixtures/exceptions/support_payload_calls.py").read_text()
    return "\n\n".join([*fragments, calls])


ACCEPTED = {
    "deep_context_default_repr": """
from contextvars import Context, ContextVar
n = 3000
rows: list[list[object]] = []
keys: list[object] = []
ctx = Context()
for _ in range(n):
    items: list[object] = []
    var = ContextVar("deep", default=items)
    rows.append(items)
    try:
        ctx[var]
    except KeyError as missing:
        keys.append(missing.args[0])
for i in range(n - 1):
    rows[i].append(keys[i + 1])

try:
    text = repr(keys[0])
except RecursionError:
    print("recursion limit")
else:
    print(len(text) > 40 * n)
for row in rows:
    row.clear()
""",
    "import_error_expanded_keywords": "print(ImportError(**{'name': 'pkg'}).name)",
    "private_forwarded_exception_fields": """
class Root(Exception):
    def __init__(self, value, *rest):
        self.__value = value
        super().__init__(value, *rest)

    def __str__(self):
        return self.__value

class _Leaf(Root):
    def __init__(self, first, second):
        super().__init__(first)
        self.__value = second

error = _Leaf("root", "leaf")
print(str(error))
print(repr(error._Root__value))
print(repr(error._Leaf__value))
""",
    "context_key_message": """
from contextvars import Context, ContextVar

default = [1]
key = ContextVar("mutable", default=default)
try:
    Context()[key]
except KeyError as error:
    print(str(error) == repr(key))
    default.append(2)
    print(str(error) == repr(key))
""",
    "upstream_support_payloads": upstream_support_payloads(),
    "boxed_payload": (Path(__file__).parent / "fixtures/exceptions/boxed_payload.py").read_text(),
    "inherited_payload": (Path(__file__).parent / "fixtures/exceptions/inherited_payload.py").read_text(),
    "warning_hierarchy": (Path(__file__).parent / "fixtures/exceptions/warning_hierarchy.py").read_text(),
    **{
        name: (Path(__file__).parent / f"fixtures/exceptions/{name}.py").read_text()
        for name in ("generic_args", "generic_args_processes", "generic_args_groups", "generic_args_records")
    },
    "explicit_base": (Path(__file__).parent / "fixtures/exceptions/explicit_base.py").read_text(),
    "import_error": (Path(__file__).parent / "fixtures/exceptions/import_error.py").read_text(),
    "private_forwarding_parameter_names": """
class Problem(Exception):
    def __init__(__self, __message, *args, __detail="detail"):
        __self.value = __message
        super().__init__(__message, __detail, *args)

class Child(Problem):
    def __init__(self, value, detail):
        super().__init__(value, _Problem__detail=detail)

try:
    print(repr(Problem(_Problem__message="yes", _Problem__detail="extra").args))
except TypeError as e:
    print(str(e))
try:
    print(repr(Problem(__message="wrong").args))
except TypeError as e:
    print(str(e))
try:
    Problem("positional", _Problem__self="duplicate")
except TypeError as e:
    print(str(e))
try:
    Problem()
except TypeError as e:
    print(str(e))

print(repr(Child("nested", "forwarded").args))
""",
    "ordinary_forwarding_fields": """
class Ordinary(Exception):
    def __init__(self, value, *rest):
        self.value = value
        self.name = value
        self.obj = value
        self.errno = value
        self.strerror = value
        self.filename = value
        self.filename2 = value
        self.characters_written = value
        super().__init__(value, *rest)

error = Ordinary("ordinary")
print(repr(error.value))
print(repr(error.name))
print(repr(error.obj))
print(repr(error.errno))
print(repr(error.strerror))
print(repr(error.filename))
print(repr(error.filename2))
print(repr(error.characters_written))
""",
    "builtin_exception_forwarding_formats": """
class Item(KeyError):
    def __init__(self, tag, *args):
        self.tag = tag
        super().__init__(*args)

class Root(ValueError):
    def __init__(self, value, *rest):
        self.tag = value
        super().__init__(value, *rest)

class Mixed(Root, KeyError):
    pass

class One(OSError):
    def __init__(self, value, *rest):
        self.tag = value
        super().__init__(value, *rest)

class OrdinaryOS(One, KeyError):
    pass

print(str(Item("marker")))
print(str(Item("marker", "missing")))
print(str(Item("marker", 17)))
print(str(Item("marker", "x", "y")))
print(repr(Item("marker", "missing").args))
print(str(Mixed("through the MRO")))
print(str(One("ordinary message")))
print(str(OrdinaryOS("OSError takes precedence")))
""",
    "classvar_exception_constructor": """
from typing import ClassVar

class ValidationProblem(ValueError):
    def __init__(self, issues: list[str]) -> None:
        super().__init__("failed")
        self.issues = issues
    def __repr__(self) -> str:
        return f"ValidationProblem(issues={self.issues!r})"

class Reader:
    error: ClassVar[type[Exception]] = ValueError
    def fail(self, issues: list[str]) -> None:
        raise self.error(issues)

class Child(Reader):
    error = ValidationProblem

def check(reader: Reader) -> None:
    values = ["a"]
    try:
        reader.fail(values)
    except ValueError as err:
        values.append("b")
        print(str(err))
        print(repr(err))
        print(isinstance(err, ValidationProblem))

class ExitPolicy:
    error: ClassVar[type[BaseException]] = SystemExit

check(Reader())
check(Child())
try:
    raise ExitPolicy.error(7)
except SystemExit as error:
    print(error.code == 7)

print(repr(Reader.error(1)))
print(str(Child.error(["x"])))
""",
    "exception_record_fields_and_mro": """
from dataclasses import dataclass
from collections.abc import Iterable

class InputError(Exception):
    pass

@dataclass(frozen=True)
class BadRow(Exception):
    elements: Iterable[str]
    issues: Iterable[str]
    def __post_init__(self) -> None:
        super().__init__(", ".join(self.issues))

class InvalidInput(ValueError, InputError):
    def __init__(self, *, filename: str | None, issues: list[str]) -> None:
        super().__init__(repr(issues))
        self.filename = filename
        self.issues = issues
    def __str__(self) -> str:
        return f"{self.filename!r}: {self.issues!r}"
    def __repr__(self) -> str:
        return f"InvalidInput(issues={self.issues!r})"

class Reader:
    error = InvalidInput
    def fail(self, items: list[str]) -> None:
        raise self.error(filename=None, issues=items)

def exercise() -> None:
    issues = ["first"]
    issue = BadRow(issues=issues, elements=["field"])
    issues.append("later")
    try:
        raise issue
    except BadRow as caught:
        print(caught is issue)
        print(repr(caught.issues))
        print(repr(caught.args))
        print(str(caught))
        print(repr(caught))
    try:
        Reader().fail(issues)
    except InputError as err:
        print(isinstance(err, ValueError))
        print(isinstance(err, BadRow))
        print(str(err))
        print(repr(err))
    try:
        raise Reader.error(filename="source", issues=issues)
    except ValueError as err:
        print(isinstance(err, InputError))
        print(str(err))
exercise()
""",
    "exception_field_conversions": """
from dataclasses import dataclass

@dataclass
class InvalidRow(Exception):
    issues: list[str]
    def __post_init__(self) -> None:
        super().__init__(", ".join(self.issues))

class Report(ValueError):
    def __init__(self, text: str) -> None:
        self.text = text
        super().__init__("base message")
    def __str__(self) -> str:
        return f"reported {self.text}"
    def __repr__(self) -> str:
        return f"Report(detail={self.text!r})"

def show(err: Exception) -> None:
    try:
        raise err
    except Exception as caught:
        print(f"detail: {caught!r} | {caught!s} | {caught}")
        print(repr(caught))

show(ValueError("can't open"))
show(KeyError("field"))
show(ValueError(["count", "hash"]))
show(InvalidRow(["bad hash", "size"]))
show(Report("item"))
""",
    "exception_generated_arguments": r"""
def show_bytes(data: bytes) -> None:
    try:
        print(data.decode("utf-8"))
    except Exception as err:
        print(f"{err!r}")
        print(f"{err!s}")

def show_path() -> None:
    try:
        with open("missing-assignable-file", "rb") as stream:
            print(stream.read())
    except OSError as err:
        print(f"{err!r}")
        print(str(err))

show_bytes(b"prefix\xffabc")
show_bytes(b"\xe3\x80")
show_bytes(b"x\xc0\xa0")
show_bytes(b"x\xe2\x82x")
show_path()
""",
    "exception_display_raises": """
class RenderError(ValueError):
    def __init__(self, key: str) -> None:
        super().__init__(key)
        self.key = key
    def __str__(self) -> str:
        raise TypeError(self.key)
    def __repr__(self) -> str:
        raise ValueError("repr: " + self.key)
def run() -> None:
    try:
        raise RenderError("render failed")
    except Exception as error:
        try:
            print(str(error))
        except TypeError as rendering:
            print(str(rendering))
        try:
            print(f"{error!r}")
        except ValueError as rendering:
            print(str(rendering))
run()
""",
    "checked_unbound_local": (Path(__file__).parent / "fixtures/exceptions/unbound_local.py").read_text(),
    "nominal_exception_hierarchy": """
class ParseError(ValueError):
    "A failure while reading input."

class MissingField(ParseError):
    pass

class InvalidField(ParseError):
    pass

try:
    try:
        raise InvalidField("unknown key")
    except ParseError as original:
        raise MissingField("missing name") from original
except InvalidField:
    raise AssertionError("a sibling matched the handler")
except ParseError as error:
    print(str(error))
    print(isinstance(error, MissingField))
    print(isinstance(error, InvalidField))
    print(isinstance(error, ValueError))
    print(error.__suppress_context__)
    cause = error.__cause__
    assert cause is not None
    print(str(cause))
    print(isinstance(cause, InvalidField))
    print(isinstance(cause, MissingField))
""",
    "explicit_causes": """
def show(error: Exception) -> None:
    print(str(error))
    print(error.__suppress_context__)
    cause = error.__cause__
    if cause is None:
        print("no cause")
    else:
        print(str(cause))
        print(cause.__suppress_context__)

def optional_cause(cause: Exception | None) -> None:
    raise RuntimeError("optional") from cause

try:
    try:
        raise KeyError("missing")
    except KeyError as original:
        raise ValueError("invalid input") from original
except ValueError as wrapped:
    show(wrapped)

try:
    raise ValueError from KeyError
except ValueError as wrapped:
    show(wrapped)

try:
    raise ValueError("suppressed") from None
except ValueError as wrapped:
    show(wrapped)

show(ValueError("plain"))

for index in range(2):
    cause: Exception | None = None
    if index:
        cause = ValueError("present")
    try:
        optional_cause(cause)
    except RuntimeError as wrapped:
        show(wrapped)
""",
    "cause_evaluation": """
def message() -> str:
    print("exception expression")
    return "outer"

def cause(fail: bool) -> Exception:
    print("cause expression")
    if fail:
        raise IndexError("cause failed")
    return ValueError("inner")

for fail in [False, True]:
    try:
        raise RuntimeError(message()) from cause(fail)
    except Exception as problem:
        print(str(problem))
        print(problem.__suppress_context__)
        original = problem.__cause__
        if original is not None:
            print(str(original))
""",
    "long_cause_cleanup": """
def make_chain() -> None:
    tail: Exception | None = None
    for index in range(20000):
        try:
            raise ValueError("chain") from tail
        except ValueError as current:
            tail = current
    count = 0
    while tail is not None:
        count += 1
        tail = tail.__cause__
    print(count)

def drop_chain() -> None:
    tail: Exception | None = None
    for index in range(20000):
        try:
            raise ValueError("chain") from tail
        except ValueError as current:
            tail = current

make_chain()
drop_chain()
print("released")
""",
    "handlers_and_messages": """
def fail(value: int) -> int:
    if value == 0:
        raise ValueError("invalid request")
    if value == 1:
        raise IndexError("missing row")
    return 9

for value in range(3):
    try:
        print(fail(value))
    except (ValueError, TypeError) as problem:
        print(problem)
    except LookupError as problem:
        print(str(problem))
    else:
        print("success")
    finally:
        print("done")

print(str(Exception()))
print(str(KeyError()))
print(str(KeyError("")))
print(str(KeyError("quote's")))
print(str(KeyError("single'" + '"' + "double")))
print(str(KeyError("line\\nzero\\0é\\u200b\\ue000")))
try:
    raise ArithmeticError
except BaseException:
    print("base")
try:
    raise RuntimeError
except:
    print("bare")
""",
    "context_default_decimal_limit": """
from contextvars import ContextVar

for number in [10**4299, 10**4300]:
    key = ContextVar("digits", default=[number])
    try:
        repr(key)
    except ValueError:
        print("limited")
    else:
        print("unlimited")
    token = key.set([0])
    key.reset(token)
    try:
        key.reset(token)
    except ValueError:
        print("limited token")
    except RuntimeError:
        print("reused token")
""",
    "runtime_failures": """
for turn in range(3):
    try:
        print(int("bad"))
    except ValueError:
        print("conversion")
    try:
        print(12 // turn)
    except ArithmeticError:
        print("division")
    try:
        print(12 % turn)
    except ZeroDivisionError:
        print("modulo")
    values: list[int] = []
    words: list[str] = []
    try:
        print(values.pop())
    except IndexError:
        print("int pop")
    try:
        print(words.pop())
    except LookupError:
        print("str pop")
    try:
        values[0] = 10
    except IndexError:
        print("int store")
    try:
        words[0] = "word"
    except IndexError:
        print("str store")
    try:
        print(values[10000000000000000000000000000000000])
    except IndexError:
        print("huge index")
    try:
        print(words[-1])
    except IndexError:
        print("negative index")
    try:
        for value in range(0, 3, 0):
            print(value)
    except ValueError:
        print("range")
    try:
        assert False
    except AssertionError as problem:
        print(str(problem))
    try:
        print(input())
    except EOFError:
        print("eof")
""",
    "context_recursive_default": """
from contextvars import Context, ContextVar

data: list[object] = []
key = ContextVar("guard", default=data)
ctx = Context()
try:
    ctx[key]
except KeyError as error:
    data.append(error.args[0])
    print("default=[<ContextVar name=" in repr(error.args[0]))
    data.clear()
    print("default=[]" in repr(error.args[0]))
""",
    "dictionary_errors": """
values: dict[str, int] = {"present": 10}
numbers: dict[int, str] = {1: "one"}
try:
    print(values["missing"])
except KeyError as problem:
    print(str(problem))
try:
    del numbers[123456789012345678901234567890]
except LookupError as problem:
    print(problem)
try:
    values["missing"] += 1
except KeyError:
    print("still absent")
print(len(values))
for turn in range(4):
    try:
        for name in values:
            values["added"] = turn
            del values["present"]
            values["another"] = 8
    except RuntimeError:
        print("changed")
    values = {"present": 10}
    print(len(values))
""",
    "active_exception": """
def again() -> None:
    raise

def after_handler_return() -> int:
    try:
        raise ValueError("handled")
    except ValueError:
        return 1
    finally:
        raise

try:
    again()
except RuntimeError as problem:
    print(problem)
try:
    raise ValueError("outer")
except ValueError as outer:
    try:
        raise TypeError("inner")
    except TypeError:
        try:
            again()
        except TypeError as inner:
            print(inner)
    try:
        again()
    except ValueError as restored:
        print(restored)
try:
    again()
except RuntimeError:
    print("inactive")
try:
    try:
        raise IndexError("finally active")
    finally:
        again()
except IndexError as problem:
    print(problem)
try:
    after_handler_return()
except RuntimeError as problem:
    print(problem)
""",
    "handler_and_else_errors": """
try:
    try:
        raise ValueError("first")
    except ValueError:
        raise IndexError("second")
    except IndexError:
        print("wrong sibling")
    finally:
        print("handler cleanup")
except LookupError as problem:
    print(problem)

try:
    try:
        print("body")
    except ValueError:
        print("wrong else handler")
    else:
        raise ValueError("from else")
    finally:
        print("else cleanup")
except ValueError as problem:
    print(problem)
""",
    "finally_return": """
def preserved() -> list[int]:
    values = [1]
    try:
        return values
    finally:
        values.append(2)
        values = [99]

def overridden() -> list[int]:
    try:
        return [123456789012345678901234567890]
    finally:
        return [20]

def suppressed() -> str:
    try:
        raise ValueError("discarded")
    finally:
        return "recovered"

def replaced() -> list[int]:
    try:
        return [123456789012345678901234567890]
    finally:
        raise ValueError("replacement")

for item in preserved():
    print(item)
print(overridden()[0])
print(suppressed())
try:
    replaced()
except ValueError as problem:
    print(problem)
""",
    "finally_loops": """
def loops() -> list[int]:
    events: list[int] = []
    for outer in range(3):
        try:
            for inner in range(2):
                return [999999999999999999999999999]
        finally:
            events.append(outer)
            if outer == 0:
                continue
            break
    return events

for value in loops():
    print(value)
for number in range(4):
    try:
        if number == 1:
            continue
        if number == 3:
            break
        print(number)
    finally:
        print("cleanup")
try:
    for number in range(2):
        try:
            raise ValueError("suppressed by break")
        finally:
            break
except ValueError:
    print("wrong exception")
print("after break")
""",
    "partial_construction": """
def fail_int() -> int:
    raise ValueError("stop")

def fail_str() -> str:
    raise ValueError("stop")

def consume(values: list[int], number: int) -> None:
    print("wrong call")

def pair(first: list[int], second: int) -> tuple[list[int], int]:
    return (first, second)

class Partial:
    values: list[int]
    number: int
    def __init__(self) -> None:
        self.values = [999999999999999999999999999999]
        self.number = fail_int()

for iteration in range(100):
    try:
        consume([99999999999999999999999999999], fail_int())
    except ValueError:
        pass
    try:
        captured = pair([99999999999999999999999999999], fail_int())
        print(captured[1])
    except ValueError:
        pass
    try:
        instance = Partial()
    except ValueError:
        pass
    try:
        values = [99999999999999999999999999999, fail_int()]
    except ValueError:
        pass
    try:
        words = {"first": "owned" + " string", "second": fail_str()}
    except ValueError:
        pass
    try:
        names = {"owned" + " string", fail_str()}
    except ValueError:
        pass
    try:
        print(("owned" + " string") + fail_str())
    except ValueError:
        pass
    try:
        print(999999999999999999999999999999 < fail_int() < 1)
    except ValueError:
        pass
print("recovered 100 times")
""",
    "atomic_updates_and_assertion": """
def failure() -> int:
    raise ValueError("failure")

def message(log: list[int]) -> str:
    log.append(1)
    return "failed condition"

left = 1000000000000000000000000000000
right = 2
try:
    left, right = (3, failure())
except ValueError:
    print(left)
    print(right)
values = [1000000000000000000000000000000]
try:
    values[0] += failure()
except ValueError:
    print(values[0])
log: list[int] = []
assert True, message(log)
print(len(log))
try:
    assert False, message(log)
except AssertionError as problem:
    print(problem)
print(len(log))
""",
    "exception_storage": """
def saved(value: Exception) -> Exception:
    return value

class Box:
    error: Exception
    def __init__(self, error: Exception) -> None:
        self.error = error

values: list[Exception] = [ValueError("stored")]
errors: dict[str, Exception] = {"error": values[0]}
box = Box(saved(errors["error"]))
optional: Exception | None = box.error
assert optional is not None
try:
    raise optional
except ValueError as caught:
    print(caught)
    retained: Exception = caught
try:
    raise retained
except Exception as repeated:
    print(repeated)
""",
    "exception_range_facts": """
def compute(value: int) -> None:
    index = value
    divisor = value
    try:
        assert index >= 0
        assert index < 2
        divisor = 1
        index = 10000000000000000000000000000000
        raise ValueError("failed after assignment")
    except Exception:
        try:
            print([10, 20][index])
        except IndexError:
            print("bounds")
        try:
            print(10 // divisor)
        except ZeroDivisionError:
            print("zero")
    finally:
        print(index)

compute(0)
compute(-1)
""",
    "infallible_field_boundaries": """
class Record:
    number: int
    optional: int | None
    flag: bool
    nothing: None
    def __init__(self, number: int, optional: int | None) -> None:
        self.number = number
        self.optional = optional
        self.flag = False
        self.nothing = None

def identity(record: Record) -> Record:
    return record

def read(record: Record) -> int:
    return identity(record).number

def replace(record: Record, number: int) -> int:
    record.number = number
    return read(record)

def fail_int() -> int:
    raise ValueError("integer child")

def fail_record() -> Record:
    raise ValueError("receiver child")

def receiver_failure() -> int:
    return identity(fail_record()).number

def argument_failure(record: Record) -> int:
    return replace(record, fail_int())

class Partial:
    values: list[int]
    number: int
    def __init__(self) -> None:
        self.values = [999999999999999999999999999999]
        self.number = fail_int()

def initializer_failure() -> Partial:
    return Partial()

def constructor_argument_failure() -> Record:
    return Record(fail_int(), None)

def field_assignment_failure(record: Record) -> None:
    record.number = fail_int()

def default_failure() -> int:
    values = {"present": 1}
    return values.get("present", fail_int())

def finally_failure(record: Record) -> int:
    try:
        return read(record)
    finally:
        fail_int()

def else_failure() -> int:
    try:
        value = 1
    except ValueError:
        return 0
    else:
        return value + fail_int()

record = Record(1000000000000000000000000000000, None)
print(read(record))
print(record.optional is None)
print(record.flag)
print(record.nothing)
print(replace(record, 7))
record.optional = 1000000000000000000000000000001
saved = record.optional
if saved is not None:
    print(saved)
for turn in range(3):
    try:
        print(receiver_failure())
    except ValueError as error:
        print(error)
    try:
        print(argument_failure(record))
    except ValueError:
        print(read(record))
    try:
        initializer_failure()
    except ValueError:
        print("partial released")
    try:
        constructor_argument_failure()
    except ValueError:
        print("constructor skipped")
    try:
        field_assignment_failure(record)
    except ValueError:
        print(read(record))
    try:
        print(default_failure())
    except ValueError:
        print("eager default")
    try:
        print(finally_failure(record))
    except ValueError:
        print("finally replaced return")
    try:
        print(else_failure())
    except ValueError:
        print("else propagated")
""",
    "integer_list_error_results": """
def read(values: list[int], index: int) -> int:
    return values[index]

def write(values: list[int], index: int, value: int) -> None:
    values[index] = value

def consume(first: list[int], second: int) -> None:
    print("wrong call")

values = [0, 1000000000000000000000000000000]
print(read(values, 0))
print(values[0] == values[0])
write(values, -1, 1000000000000000000000000000001)
values[0] = values[1]
values[1] = 0
values[0] = values[0]
print(read(values, 0))
print(values.pop())
print(values.pop())
for turn in range(3):
    try:
        print(values.pop())
    except IndexError:
        print("empty pop")
    values.append(1000000000000000000000000000002 + turn)
    try:
        print(values[0] < values[9])
    except IndexError:
        print("borrow failure")
    try:
        consume([1000000000000000000000000000003], read(values, -2))
    except IndexError:
        print("argument cleanup")
    try:
        print(read(values, 1000000000000000000000000000000))
    except IndexError:
        print("huge index")
    try:
        values[0] = values[9]
    except IndexError:
        print("copy source")
    try:
        values[9] = values[0]
    except IndexError:
        print("copy target")
    try:
        values[9] = 1000000000000000000000000000004
    except IndexError:
        print("decoded store")
    try:
        write(values, -2, 1000000000000000000000000000005)
    except IndexError:
        print("signed store")
    saved = values[0]
    values[0] = 0
    print(saved)
    print(values.pop())

flags: list[bool] = [False]
nones: list[None] = [None]
optionals: list[int | None] = [None]
mapping: dict[str, None] = {"present": None}
print(flags[0])
print(nones[0])
print(optionals[0] is None)
print(optionals.pop() is None)
print(mapping["present"])
try:
    print(flags[1])
except IndexError:
    print("generic false remains checked")
try:
    print(mapping["missing"])
except KeyError:
    print("generic none remains checked")
""",
}

REJECTED = {
    "exception_descriptor_annotated_field": """
class Problem(StopIteration):
    value: str
    def __init__(self, before: str, after: str):
        self.value = before
        super().__init__(after)
Problem("before", "after")
""",
    **{
        f"exception_descriptor_inherited_field_{base}": f"""
class Ordinary(Exception):
    def __init__(self, before, after, *rest):
        self.{field} = before
        super().__init__(after, *rest)
class Mixed(Ordinary, {base}):
    pass
Mixed({arguments})
"""
        for base, field, arguments in [
            ("StopIteration", "value", '"before", "after"'),
            ("ImportError", "msg", '"before", "after"'),
            ("UnicodeEncodeError", "reason", '"before", "ascii", "x", 0, 1, "after"'),
        ]
    },
    **{
        f"forwarding_exception_{base}_{name}": f"""
class Parent({base}):
    pass
class Problem(Parent):
    def __init__(self, before, after, *rest):
        self.{name} = before
        super().__init__(after, *rest)
Problem("before", "after")
"""
        for base, fields in {
            "StopIteration": ("value",),
            "NameError": ("name",),
            "UnboundLocalError": ("name",),
            "AttributeError": ("name", "obj"),
            "OSError": ("errno", "strerror", "filename", "filename2", "characters_written"),
            "FileNotFoundError": ("filename",),
        }.items()
        for name in fields
    },

    "unsupported_forwarding_OSError": """
class Problem(OSError):
    def __init__(self, leading, *rest):
        self.tag = leading
        super().__init__(leading, *rest)
e = Problem(2, "missing", "path")
print(repr(e.args))
print(str(e))
""",
    "unsupported_forwarding_UnicodeEncodeError": """
class Problem(UnicodeEncodeError):
    def __init__(self, leading, *rest):
        self.tag = leading
        super().__init__(leading, *rest)
e = Problem("ascii", "ø", 0, 1, "invalid")
print(repr(e.args))
print(str(e))
""",
    **{
        f"forwarding_exception_descriptor_{name}": f"""
class Problem(Exception):
    def __init__(self, value, *rest):
        super().__init__(value, *rest)
        self.{name} = value
Problem("ab")
"""
        for name in (
            "args", "__cause__", "__context__", "__traceback__",
            "__suppress_context__", "__dict__", "__class__", "__weakref__",
        )
    },
    "separate_payload_layouts": """
class Left(ValueError):
    def __init__(self, x: int) -> None:
        self.x = x
        super().__init__(x)
class Right(ValueError):
    def __init__(self, y: str) -> None:
        self.y = y
        super().__init__(y)
class Both(Left, Right):
    pass
""",
    "exception_binder_adjacent_error": """
class Basic(Exception):
    def __init__(self, item, *tail):
        self.item = item
        super().__init__(item, *tail)
def annotated(value: str) -> str:
    return value
Basic("original", item=annotated(37))
""",
    "exception_recursive_dynamic_list": """
class E(Exception):
    def __init__(self, value, *args):
        self.value = value
        super().__init__(value, *args)


def exercise() -> None:
    values: list[object] = []
    error = E(values)
    values.append(error.value)
    print(repr(error.value))


exercise()
""",
    "exception_forwarding_object": """
class Box:
    def __init__(self, item: int) -> None:
        self.item = item
class Report(Exception):
    def __init__(self, msg, *args):
        self.msg = msg
        super().__init__(msg, *args)
print(Report(Box(1)))
""",
    "exception_mutable_default": """
class Shared(Exception):
    def __init__(self, item=[], *tail):
        self.item = item
        super().__init__(item, *tail)
print(Shared())
""",
    "exception_shadowed_super": """
class Report(Exception):
    def __init__(self, super, *tail):
        self.value = super
        super().__init__(super, *tail)
print(Report("argument"))
""",
    "exception_intercepted_field": """
class Report(Exception):
    def __setattr__(self, key: str, value: str) -> None:
        raise TypeError(key)
    def __init__(self, message, *tail):
        self.message = message
        super().__init__(message, *tail)
print(Report("message"))
""",
    "explicit_base_missing_inherited_field": """
        class Parent(Exception):
            def __init__(self, text: str):
                super().__init__(text)
                self.text = text
        class Child(Parent):
            def __init__(self, text: str):
                BaseException.__init__(self, text)
        print(Child("error"))
    """,
    "explicit_base_shadowed": """
        class Problem(Exception):
            def __init__(self, Exception: str):
                Exception.__init__(self, "wrong receiver")
        print(Problem("shadow"))
    """,
    "import_error_unsupported_argument": "ImportError([1, 2])",
    "import_error_unnarrowed_field": "def f(e: Exception) -> None:\n    print(e.name)\n",
    "handler_uninitialized": """
try:
    value = int(input())
except ValueError:
    print(value)
""",
    "handler_target_deleted": """
try:
    raise ValueError("error")
except ValueError as problem:
    print(problem)
print(problem)
""",
    "handler_target_previous_deleted": """
problem: Exception = ValueError("old")
try:
    raise ValueError("new")
except ValueError as problem:
    pass
print(problem)
""",
    "handler_target_deleted_in_loop": """
problem: Exception = ValueError("old")
for turn in range(2):
    try:
        raise ValueError("new")
    except ValueError as problem:
        pass
print(problem)
""",
    "handler_target_deleted_on_backedge": """
problem: Exception = ValueError("old")
for turn in range(2):
    print(problem)
    try:
        raise ValueError("new")
    except ValueError as problem:
        pass
""",
    "nested_handler_target_deleted": """
problem: Exception = ValueError("old")
try:
    try:
        raise ValueError("inner")
    except ValueError as problem:
        pass
    raise IndexError()
except IndexError:
    print(problem)
""",
    "finally_uninitialized": """
try:
    value = int(input())
finally:
    print(value)
""",
    "handler_optional_fact": """
value: int | None = 1
try:
    value = None
    raise ValueError()
except ValueError:
    number: int = value
""",
    "finally_optional_fact": """
value: int | None = 1
try:
    value = None
finally:
    number: int = value
""",
    "multiple_messages": 'raise ValueError("a", "b")',
    "subclass_annotation": 'value: ValueError = ValueError("error")',
    "existing_exception_cause": 'error = ValueError("error")\nraise error from None',
    "nonexception_cause": 'raise ValueError("error") from "wrong"',
    "writable_exception_cause": 'error = ValueError("error")\nerror.__cause__ = None',
    "writable_exception_suppression": 'error = ValueError("error")\nerror.__suppress_context__ = True',
    "exception_group": "try:\n    raise ValueError()\nexcept* ValueError:\n    pass\n",
    "dynamic_handler": "kind = ValueError\ntry:\n    pass\nexcept kind:\n    pass\n",
    "implicit_exception_constructor_args": """
from dataclasses import dataclass
@dataclass
class Payload(Exception):
    field: str
raise Payload("x")
""",
    "inconsistent_exception_mro": """
class First(Exception):
    pass
class Second(First):
    pass
class Reversed(First, Second):
    pass
raise Reversed()
""",
    "exception_subclass_method": """
class WithMethod(Exception):
    def describe(self) -> str:
        return "extra behavior"
""",
}

ERRORS = {
    "unhandled_skip_class": (
        'import unittest\nraise unittest.SkipTest("skip")\n',
        "unittest.case.SkipTest",
    ),
    "unhandled_json_class": (
        'import json\njson.loads("{")\n',
        "json.decoder.JSONDecodeError",
    ),
    "handler_optional_arithmetic": (
        """\
value: int | None = 1
try:
    value = None
    raise ValueError()
except ValueError:
    print(value + 1)
""",
        "TypeError",
    ),
    "finally_optional_arithmetic": (
        """\
value: int | None = 1
try:
    value = None
finally:
    print(value + 1)
""",
        "TypeError",
    ),
    "unhandled_cleanup": (
        'try:\n    values = [999999999999999999999999999999]\n    raise ValueError("terminal")\nfinally:\n    print("cleaned")\n',
        "ValueError",
    ),
    "unhandled_finally_replacement": (
        'try:\n    raise ValueError("first")\nfinally:\n    raise IndexError("replacement")\n',
        "IndexError",
    ),
    "unhandled_reraise": (
        'try:\n    raise ValueError("again")\nexcept ValueError:\n    raise\n',
        "ValueError",
    ),
    "unhandled_mixed_chain": (
        """
try:
    raise ValueError("first")
except ValueError:
    try:
        raise RuntimeError("middle")
    except RuntimeError as middle:
        raise KeyError("last") from middle
""",
        "KeyError",
    ),
    "unhandled_reversed_context": (
        """
first = ValueError("first")
last = RuntimeError("last")
try:
    raise first
except ValueError:
    try:
        raise last
    except RuntimeError:
        raise first
""",
        "ValueError",
    ),
    "unhandled_explicit_chain": (
        """
try:
    try:
        raise KeyError("inner")
    except KeyError as inner:
        raise ValueError("middle") from inner
except ValueError as middle:
    raise RuntimeError("outer") from middle
""",
        "RuntimeError",
    ),
    "unhandled_suppressed_context": (
        """
try:
    raise KeyError("hidden predecessor")
except KeyError:
    raise ValueError("visible") from None
""",
        "ValueError",
    ),
}

CAUSE_SEPARATOR = "The above exception was the direct cause of the following exception:"
CONTEXT_SEPARATOR = "During handling of the above exception, another exception occurred:"
CHAIN_REPORTS = {
    "unhandled_mixed_chain": [
        "ValueError: first",
        CONTEXT_SEPARATOR,
        "RuntimeError: middle",
        CAUSE_SEPARATOR,
        "KeyError: 'last'",
    ],
    "unhandled_reversed_context": [
        "RuntimeError: last",
        CONTEXT_SEPARATOR,
        "ValueError: first",
    ],
    "unhandled_finally_replacement": [
        "ValueError: first",
        CONTEXT_SEPARATOR,
        "IndexError: replacement",
    ],
    "unhandled_explicit_chain": [
        "KeyError: 'inner'",
        CAUSE_SEPARATOR,
        "ValueError: middle",
        CAUSE_SEPARATOR,
        "RuntimeError: outer",
    ],
    "unhandled_skip_class": ["unittest.case.SkipTest: skip"],
    "unhandled_suppressed_context": ["ValueError: visible"],
}


def exception_report(stderr: str) -> list[str]:
    """Keep exception messages and chaining separators without Python stack frames."""
    return [
        line
        for line in stderr.splitlines()
        if line
        and not line[0].isspace()
        and line != "Traceback (most recent call last):"
    ]


def run(command: list[str], *, timeout: int = 60) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        input="",
        text=True,
        capture_output=True,
        check=False,
        timeout=timeout,
        env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"},
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    compiler = str(args.compiler.resolve())
    flags = ["--runtime", str(args.runtime.resolve()), "--clang", args.clang]
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags += ["--sanitize"]
    passed = 0
    with tempfile.TemporaryDirectory(prefix="imre-exceptions-") as directory:
        temporary = Path(directory)
        for name, source in REJECTED.items():
            path = temporary / f"{name}.py"
            path.write_text(dedent(source), encoding="utf-8")
            result = run([compiler, "check", str(path)])
            assert result.returncode > 0 and result.stderr, (name, result)
            if name == "exception_subclass_method":
                assert "exception payload needs an explicit initializer" in result.stderr, result
            if name == "separate_payload_layouts":
                assert "multiple independent exception payload layouts" in result.stderr, result
            if name == "exception_binder_adjacent_error":
                assert "str" in result.stderr and "int" in result.stderr, result
            if name in {"exception_forwarding_object", "exception_recursive_dynamic_list"}:
                assert "forwarding exception fields require" in result.stderr, result
            passed += 1
        cases = [(name, source, None) for name, source in ACCEPTED.items()]
        cases += [
            (name, source, category) for name, (source, category) in ERRORS.items()
        ]
        for name, source, category in cases:
            path = temporary / f"{name}.py"
            path.write_text(dedent(source), encoding="utf-8")
            reference = run(
                [args.python, "-I", "-W", "ignore::SyntaxWarning", str(path)]
            )
            if category:
                assert reference.returncode > 0 and category in reference.stderr, (
                    name,
                    reference,
                )
                if name in CHAIN_REPORTS:
                    assert (
                        exception_report(reference.stderr) == CHAIN_REPORTS[name]
                    ), (
                        name,
                        reference.stderr,
                    )
            else:
                assert reference.returncode == 0 and not reference.stderr, (
                    name,
                    reference,
                )
            for level in args.opt_levels:
                binary = temporary / f"{name}-O{level}"
                built = run(
                    [
                        compiler,
                        "build",
                        str(path),
                        "-o",
                        str(binary),
                        "--opt-level",
                        level,
                        *flags,
                    ],
                    timeout=120 if args.lto_linker and not args.sanitize else 60,
                )
                assert built.returncode == 0, (name, level, built.stderr)
                actual = run([str(binary)])
                binary.unlink()
                assert actual.stdout == reference.stdout, (
                    name,
                    level,
                    actual,
                    reference,
                )
                if category:
                    if name in CHAIN_REPORTS:
                        assert actual.returncode == 1 and exception_report(
                            actual.stderr
                        ) == exception_report(reference.stderr), (name, level, actual)
                    else:
                        # These cases check the terminal exception and its message.
                        assert (
                            actual.returncode == 1
                            and exception_report(actual.stderr)[-1].startswith(category + ":")
                            and exception_report(actual.stderr)[-1:]
                            == exception_report(reference.stderr)[-1:]
                        ), (name, level, actual.stderr, reference.stderr)
                else:
                    assert actual.returncode == 0 and not actual.stderr, (
                        name,
                        level,
                        actual,
                    )
                assert "Sanitizer" not in actual.stderr, (name, level, actual.stderr)
                passed += 1
    print(
        f"Passed {passed} exception checks ({len(REJECTED)} rejected programs; optimization levels {args.opt_levels}; ASan={args.sanitize})"
    )


if __name__ == "__main__":
    main()

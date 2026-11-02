"""Executable source code shares code identity and its caller's live namespaces."""

class Builtins:
    def __getitem__(self, name):
        if name == "answer":
            return 42
        if name == "broken":
            raise ValueError("builtin lookup")
        raise KeyError(name)


def ordinary():
    pass


def install_global():
    global exec_probe
    exec_probe = any(item for item in [False, True])


def remove_global():
    global exec_probe
    del exec_probe


class GlobalDict(dict):
    def __setitem__(self, key, value):
        raise AssertionError("global assignment called __setitem__")

    def __delitem__(self, key):
        raise AssertionError("global deletion called __delitem__")


class ComparisonResult:
    def __bool__(self):
        raise AssertionError("final comparison coerced")


comparison_result = ComparisonResult()


class Comparable:
    def __eq__(self, other):
        return comparison_result


def decode_allocation():
    # Match the boxed-call allocator fixture's 113-word request.
    padding = 113 * 8 - len(b"#\nVALUE = 7\n")
    source = bytearray(b"#" + b"x" * padding + b"\nVALUE = 7\n")
    marker = {}
    marker[0] = 1
    try:
        compile(source, "settings.py", "exec")
    except MemoryError as error:
        print("caught", error.args)
    else:
        print("compiled")
    config = {}
    exec(compile(source, "settings.py", "exec"), config)
    print(config["VALUE"])


def syntax_members():
    try:
        compile(b"# coding: unknown-encoding\n", "dir/one.py", "exec")
    except SyntaxError as error:
        saved = error.args
        fields = ("msg", "filename", "lineno", "offset", "text", "end_lineno", "end_offset")
        values = ("revised", "other/two.py", 8, 3, "line", 9, 5)
        for field, value in zip(fields, values):
            setattr(error, field, value)
        print(str(error), error.args is saved, tuple(getattr(error, field) for field in fields))
        for field in fields:
            delattr(error, field)
        print(str(error), error.args is saved, all(getattr(error, field) is None for field in fields))

    class Message:
        def __str__(self):
            error.filename = "new.py"
            error.lineno = 11
            return "changed while rendering"

    error = SyntaxError(Message(), ("old.py", 7, 0, ""))
    print(str(error), str(error))


def source_classes():
    events = []
    def mark(label):
        events.append(("evaluate", label))
        def apply(cls):
            events.append(("apply", label, cls.__name__))
            return cls
        return apply
    namespace = {"__name__": "disk_cases", "mark": mark, "events": events, "value": "global"}
    code = compile("""class Base:
    def label(self):
        return 'base'

@mark('outer')
@mark('inner')
class Case(Base):
    'Source case.'
    events.append(('body', value))
    value = 'class'
    def label(self):
        self.__seen = value
        return super().label(), self.__seen
    def increment(self):
        self.previous += 1
    def nested(self):
        class Child:
            self.applied = True
        return Child
    @classmethod
    def describe(cls):
        return cls.__qualname__
""", "disk_cases.py", "exec")
    exec(code, namespace)
    case = namespace["Case"]
    assert events == [("evaluate", "outer"), ("evaluate", "inner"), ("body", "global"),
                      ("apply", "inner", "Case"), ("apply", "outer", "Case")]
    assert case.__doc__ == "Source case."
    assert (case.__module__, case.__qualname__) == ("disk_cases", "Case")
    assert case.label.__module__ == "disk_cases" and case.label.__qualname__ == "Case.label"
    assert case.__static_attributes__ == ("__seen", "applied")
    instance = case()
    nested = instance.nested()
    assert instance.applied and nested.__qualname__ == "Case.nested.<locals>.Child"
    assert nested.__static_attributes__ == ()
    instance.previous = 4
    instance.increment()
    assert instance.previous == 5
    assert case().label() == ("base", "global")
    assert case.describe() == "Case"
    assert case.__firstlineno__ == 5
    try:
        deep_code = compile("class Deep:\n    total = 1" + "+1" * 8192, "deep_class.py", "exec")
    except RecursionError:
        pass
    else:
        deep = {}
        exec(deep_code, deep)
        assert deep["Deep"].total == 8193
    old = object()
    namespace["Broken"] = old
    try:
        exec(compile("class Broken:\n    events.append('started')\n    raise ValueError('class body')\n",
                     "failure.py", "exec"), namespace)
    except ValueError as error:
        assert error.args == ("class body",)
    else:
        raise AssertionError("class body exception was lost")
    assert namespace["Broken"] is old and events[-1] == "started"

    def reject(cls):
        events.append(cls.__name__)
        raise LookupError("class decorator")
    namespace["reject"] = reject
    try:
        exec(compile("@reject\nclass Broken:\n    events.append('decorated')\n", "failure.py", "exec"), namespace)
    except LookupError as error:
        assert error.args == ("class decorator",)
    else:
        raise AssertionError("class decorator exception was lost")
    assert namespace["Broken"] is old and events[-2:] == ["decorated", "Broken"]


def source_test_cases():
    import types
    import unittest

    module = types.ModuleType("disk_cases")
    visited = []
    module.visited = visited
    exec(compile("""import unittest

class Cases(unittest.TestCase):
    def setUp(self):
        visited.append('setup')
    def tearDown(self):
        visited.append('teardown')
    def test_pass(self):
        visited.append('pass')
        self.assertEqual(2 + 3, 5)
    def test_fail(self):
        visited.append('fail')
        self.assertEqual(4, 5)
    @unittest.skip('source method')
    def test_skip(self):
        raise AssertionError('skipped method executed')
""", "disk_cases.py", "exec"), module.__dict__)
    suite = unittest.TestLoader().loadTestsFromModule(module)
    assert suite.countTestCases() == 3
    result = unittest.TestResult()
    suite.run(result)
    assert result.testsRun == 3 and not result.wasSuccessful()
    assert len(result.failures) == 1 and result.failures[0][0].id() == "disk_cases.Cases.test_fail"
    assert len(result.skipped) == 1 and result.skipped[0][0].id() == "disk_cases.Cases.test_skip"
    assert result.skipped[0][1] == "source method" and not result.errors
    assert visited == ["setup", "fail", "teardown", "setup", "pass", "teardown"]


def current():
    source_classes()
    source_test_cases()
    syntax_members()
    for data in (b"\xef\xbb\xbfVALUE = 61", bytearray(b"#!python\r# coding: latin-1\rVALUE = '\xe9'")):
        config = {}
        exec(compile(data, "settings.py", "exec"), config)
        print(config["VALUE"])
    for data in (b"# coding: missing-codec", b"\xef\xbb\xbf# coding: latin-1", b"VALUE = '\xff'", b"x=1\0", b"\xef\xbb\xbf\xef\xbb\xbfX=1"):
        try:
            compile(data, "settings.py", "exec")
        except SyntaxError as error:
            print(str(error), error.args, error.msg, error.filename, error.lineno, error.offset, error.text, error.end_lineno, error.end_offset)
    namespace = {"__name__": "template", "events": [], "external": 41, "default": 100}
    namespace["left"] = Comparable()
    namespace["right"] = object()
    exec("comparison = left == right", namespace)
    print(namespace["comparison"] is comparison_result)
    local = {"default": 1}
    text = (
        "def root(value=default):\n"
        "    events.append('start')\n"
        "    yield external + value\n"
        "    events.append('end')\n"
        "    yield external\n"
        "alias = root\n"
    )
    module = compile(text, "<template>", "exec")
    again = compile(text, "<template>", "exec")
    exec(module, namespace, local)
    root = local["root"]
    print(type(module) is type(root.__code__), module is not again, root.__globals__ is namespace)
    print(module.co_name, root.__code__.co_filename, root.__code__.co_firstlineno, root.__module__)
    print(local["alias"] is root, "root" in namespace, "external" in local, "__builtins__" in namespace)
    builtins = namespace["__builtins__"]
    print(builtins["dict"] is dict, "enum.Enum" in builtins, "enum.StrEnum" in builtins)
    result = root()
    print(result.gi_code is root.__code__, iter(result) is result, namespace["events"])
    print(next(result), namespace["events"])
    namespace["external"] = 99
    print(next(result), next(result, "done"), namespace["events"])
    exec("def sequence():\n    yield 11\n    yield 12\n    return 42", namespace)
    sequence = namespace["sequence"]()
    for first in sequence:
        print(first)
        break
    print(next(sequence))
    try:
        next(sequence)
    except StopIteration as error:
        print(error.args, next(sequence, "exhausted"))
    print(list(namespace["sequence"]()))
    expression = (item for item in namespace["sequence"]())
    print(type(expression) is type(sequence), type(expression.gi_code) is type(module))
    print(next(expression), list(expression), next(expression, "exhausted"))
    supplied = GlobalDict()
    separate = {"exec_probe": "local"}
    exec(install_global.__code__, supplied, separate)
    print(supplied["exec_probe"], separate["exec_probe"], supplied["__builtins__"]["any"] is any)
    exec(remove_global.__code__, supplied, separate)
    print("exec_probe" in supplied, separate["exec_probe"])
    globals_ = {"__name__": "source_globals", "value": 3}
    locals_ = {"value": 99}
    exec("""observed = value
def update():
    global value
    value += 1
class Reader:
    value = 7
    def read(self):
        global value
        return value
    def no_class_cell(self):
        global __class__
        return super()
class Clear:
    global value, callback, Exported
    before = value
    del value
    def callback():
        return 4
    class Exported:
        pass
""", globals_, locals_)
    assert locals_["observed"] == locals_["Clear"].before == 3
    assert "value" not in globals_ and locals_["value"] == 99
    assert globals_["callback"].__qualname__ == "callback" and globals_["callback"]() == 4
    assert globals_["Exported"].__qualname__ == "Exported"
    globals_["value"] = 11
    locals_["update"]()
    reader = locals_["Reader"]()
    assert reader.value == 7 and reader.read() == 12
    try:
        reader.no_class_cell()
    except RuntimeError as error:
        assert str(error) == "super(): __class__ cell not found"
    else:
        raise AssertionError("a global __class__ does not create a closure cell")
    other = {}
    exec(module, {"default": 2}, other)
    print(other["root"] is not root, other["root"].__code__ is root.__code__)
    g = {"__builtins__": {"len": lambda obj: 29}}
    exec("def measure():\n    return len('x')\n", g)
    measure = g["measure"]
    g["__builtins__"] = {}
    print(measure(), measure.__globals__ is g)
    try:
        exec("def missing():\n    return len('x')\n", g)
        g["missing"]()
    except NameError as error:
        print(type(error).__name__, str(error))
        trace = error.__traceback__
        while trace.tb_next is not None:
            trace = trace.tb_next
        print(trace.tb_frame.f_code is g["missing"].__code__, trace.tb_lineno, trace.tb_frame.f_code.co_filename)
    try:
        deep = compile("result = " + "1+" * 32768 + "1", "<deep-compile>", "exec")
    except RecursionError:
        pass
    else:
        assert deep.co_filename == "<deep-compile>"
        del deep
    print("deep compile returned")
    try:
        deep = compile("result = 1" + "+1" * 16384, "<deep-eval>", "exec")
    except RecursionError:
        pass
    else:
        deep_values = {}
        exec(deep, deep_values)
        assert deep_values["result"] == 16385
    module = compile("from_source = 73\n", "<rebound>", "exec")
    measure.__code__ = module
    print(measure(), measure.__globals__["from_source"], measure.__code__ is module)
    custom = {"__builtins__": Builtins()}
    for name in ("answer", "broken", "missing"):
        exec("def get(): return " + name, custom)
        try:
            print(custom["get"]())
        except (NameError, ValueError) as error:
            print(type(error).__name__, str(error))
    def importer(name, globals_, locals_, fromlist, level):
        print(name, globals_ is imported, locals_ is separate, locals_ is None, fromlist, level)
        return __import__(name, globals_, locals_, fromlist, level)
    imported = {"__builtins__": {"__import__": importer}}
    separate = {}
    exec("def modules():\n    import sys, collections.abc as abc\n    yield sys.__name__, abc.__name__", imported)
    assert list(imported["modules"]()) == [("sys", "collections.abc")]
    exec("from sys import argv", imported, separate)
    exec("def read():\n    from sys import argv\n    return argv", imported)
    print(imported["read"]() is separate["argv"])
    ordinary.__code__ = imported["read"].__code__
    print(ordinary() is separate["argv"])
    try:
        exec("from sys import argv", {"__builtins__": Builtins()})
    except ImportError as error:
        print(type(error).__name__, str(error))
    for operation in (lambda: exec("x = 1", 7), lambda: exec(module, {}, 7),
                      lambda: compile("", "<template>", "invalid")):
        try:
            operation()
        except (ValueError, TypeError) as error:
            print(type(error).__name__, str(error))


if __name__ == "__main__":
    current()

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
        self.__seen: UnusedAnnotation = value
        seen = {}
        seen[0]: UnusedAnnotation = self.__seen
        return super().label(), seen[0]
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


def source_annotations():
    from types import FunctionType

    ns = {"__name__": "annotations", "format": bytes, "Alias": bytes}
    exec("""def factory(outer, Alias, Global):
    class C:
        global Global
        def uses_global(self, x: Global): ...
        def method(self, first: Late, /, normal: outer, *args: format,
                   named: Alias, **kw: int) -> None: ...
        Alias = str
        def sibling(self, arg: Alias): ...
    return C
""", ns)
    cls = ns["factory"](bool, int, int)
    cls.Global = float
    ns["Global"] = complex
    assert cls.uses_global.__annotations__ == {"x": complex}
    assert cls.uses_global.__annotate__.__code__.co_freevars == ("__classdict__",)
    provider = cls.method.__annotate__
    assert provider.__code__.co_freevars == ("__classdict__", "outer")
    assert provider.__closure__[0] is cls.sibling.__annotate__.__closure__[0]
    assert provider.__globals__ is ns
    try:
        cls.method.__annotations__
    except NameError as exc:
        assert exc.name == "Late"
    else:
        raise AssertionError("unresolved annotation was evaluated")
    for unsupported in (3, 4):
        try:
            provider(unsupported)
        except NotImplementedError as exc:
            assert exc.args == ()
        else:
            raise AssertionError("raw provider accepted a public replay format")
    # Replay with replacement globals, as annotationlib does for missing names.
    clone = FunctionType(provider.__code__, {"Late": complex, "format": list},
                         closure=provider.__closure__)
    cls.Alias = float
    replayed = clone(2)
    assert list(replayed) == ["normal", "first", "args", "named", "kw", "return"]
    assert replayed == {"normal": bool, "first": complex, "args": list, "named": float,
                        "kw": int, "return": None}
    del cls.Alias
    ns["Late"] = tuple
    values = cls.method.__annotations__
    assert values["first"] is tuple and values["args"] is bytes
    assert values["named"] is bytes
    assert cls.method.__annotations__ is values
    exec("from __future__ import annotations\nclass F:\n    def f(self, n: Missing, obj: 'A') -> tuple[int]: ...", ns)
    future = ns["F"].f
    assert future.__annotate__.__closure__ is None
    assert future.__annotations__ == {"n": "Missing", "obj": "'A'", "return": "tuple[int]"}
    fallback = {"__annotations__": {}}
    exec("""from __future__ import annotations
def fields():
    __annotations__ = {}
    class Fields:
        first: int
        del __annotations__
        second: Missing
    return Fields, __annotations__
""", fallback)
    fields, outer = fallback["fields"]()
    assert not outer and fallback["__annotations__"] == {"second": "Missing"}
    assert fields.__annotate__ is None
    exec("class Declared:\n    existed = '__annotations__' in locals()\n    field: int\nclass Empty(Declared): pass", ns)
    assert not ns["Declared"].existed
    assert ns["Empty"].__annotate__ is None and ns["Empty"].__annotations__ == {}
    for expr in ("(tmp := int)", "(yield int)", "(await T)"):
        for source in ("def f(arg: " + expr + "): pass",
                       "def f(obj): obj.field: " + expr + " = 1",
                       "def f(obj): obj[0]: " + expr + " = 1"):
            try:
                compile(source, "<annotations>", "exec")
            except SyntaxError:
                pass
            else:
                raise AssertionError("invalid annotation expression compiled")


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


def source_name_deletion():
    import gc
    import weakref

    namespace = {"weakref": weakref, "gc": gc, "name": "global"}
    exec(
        "class Box:\n"
        "    name = 'local'\n"
        "    del name\n"
        "    remaining = name\n"
        "def check():\n"
        "    value = Box()\n"
        "    ref = weakref.ref(value)\n"
        "    del value\n"
        "    gc.collect()\n"
        "    return ref()\n",
        namespace,
    )
    assert namespace["Box"].remaining == "global"
    assert not hasattr(namespace["Box"], "name")
    assert namespace["check"]() is None
    try:
        exec("def f():\n    del name\nf()", namespace)
    except UnboundLocalError as error:
        assert str(error) == "cannot access local variable 'name' where it is not associated with a value"
    else:
        raise AssertionError("deleting an unbound local must raise")
    exec("del name", namespace)
    assert "name" not in namespace


def relative_import_hooks():
    import sys
    import types

    exports = types.ModuleType("base_events")
    exports.flag = object()
    exports._hidden = True
    calls = []
    missing = ImportError("relative hook failed")

    def importer(name, globals_, locals_, fromlist, level):
        assert globals_ is namespace and locals_ is local
        calls.append((name, fromlist, level))
        if name == "base_events":
            return exports
        if name == "missing":
            raise missing
        return sys

    namespace = {"__builtins__": {"__import__": importer}}
    local = {}
    exec("from sys import argv as absolute\nfrom . import argv as sibling\nfrom ..helper import argv as parent",
         namespace, local)
    assert local["absolute"] is local["sibling"] is local["parent"] is sys.argv
    try:
        exec("from .missing import attr", namespace, local)
    except ImportError as error:
        assert error is missing
    else:
        raise AssertionError("relative import hook exception was lost")
    exec("from .base_events import *", namespace, local)
    assert local["flag"] is exports.flag and "_hidden" not in local
    assert calls == [("sys", ("argv",), 0), ("", ("argv",), 1),
                     ("helper", ("argv",), 2), ("missing", ("attr",), 1),
                     ("base_events", ("*",), 1)]
    # The locals dictionary receives the original key even if the exported
    # attribute removes the last reference to it during lookup.
    import gc
    import _weakref

    class Key(str):
        pass

    class Exports:
        @property
        def export(self):
            self.__all__.clear()
            gc.collect()
            return 42

    exports = Exports()
    exports.__all__ = [Key("export")]
    original = _weakref.ref(exports.__all__[0])
    exec("from .base_events import *", namespace, local)
    assert local["export"] == 42
    assert any(key is original() for key in local)
    for scope in ("class Case:", "def case():"):
        try:
            compile(scope + "\n    from .base_events import *", "case.py", "exec")
        except SyntaxError as error:
            assert "import * only allowed at module level" in str(error)
        else:
            raise AssertionError("wildcard import accepted in a nested scope")


def source_match():
    class Labels:
        first = "hit"

    calls = []
    def observe(value):
        calls.append(value)
        return value

    scope = {"Labels": Labels, "observe": observe}
    exec("""
def choose(value, guard):
    match observe(value):
        case Labels.first if observe(guard):
            return "first"
        case str():
            return "text"
        case ValueError():
            return "exception"
        case _:
            return "other"
""", scope)
    choose = scope["choose"]
    assert choose("hit", True) == "first"
    assert choose("hit", False) == "text"
    assert choose(5, False) == "other"
    assert calls == ["hit", True, "hit", False, 5]
    assert choose(ValueError("matched"), False) == "exception"


def source_genexpr():
    namespace = {}
    exec("""
def make(events):
    def source():
        events.append("outer")
        return [(1, 2), (3, 4)]
    def inner(value):
        events.append(value)
        return (value, value + 1)
    tag = "early"
    value = "outer local"
    result = ((tag, item) for pair in source() for value in pair if value % 2 for item in inner(value))
    tag = "late"
    return result, value
""", namespace)
    events = []
    generator, original = namespace["make"](events)
    assert events == ["outer"] and original == "outer local"
    assert next(generator) == ("late", 1)
    assert events == ["outer", 1]
    assert list(generator) == [("late", 2), ("late", 3), ("late", 4)]
    assert events == ["outer", 1, 3]


def named_bindings():
    namespace = {}
    exec("""def recipe(values):
    source = iter(values)
    while batch := next(source, None):
        yield batch
    yield batch

def outer(value):
    state = []
    def update():
        nonlocal cell
        state.append((cell, (cell := value), cell))
    def read():
        return cell
    (cell := [])
    update()
    return state, read

def shadow():
    if False:
        (shadowed := 1)
    return shadowed

def use_global(value):
    global global_target
    return (global_target := value)

def annotated(arg: (lambda: (shadowed, (shadowed := 4)))):
    pass
""", namespace)
    marker = []
    assert list(namespace["recipe"]([1, 2, marker])) == [1, 2, marker]
    stored, read = namespace["outer"](marker)
    assert stored[0][0] is not marker
    assert stored[0][1] is stored[0][2] is read() is marker
    namespace["shadowed"] = 9
    for read_local in (namespace["shadow"], namespace["annotated"].__annotations__["arg"]):
        try:
            read_local()
        except UnboundLocalError:
            pass
        else:
            raise AssertionError("walrus target was not a local")
    assert namespace["use_global"](marker) is namespace["global_target"] is marker
    exec("x = lambda a=(from_default := 3): (inside := a)", namespace)
    assert namespace["x"]() == namespace["from_default"] == 3
    assert "inside" not in namespace
    for text in (
        "def f():\n    (x := 1)\n    global x",
        "def f():\n    global x\n    if True: (x := 1)\n    global x",
        "(i for i in (items := [1]))",
        "(__debug__ := True)",
    ):
        try:
            compile(text, "walrus.py", "exec")
        except SyntaxError:
            pass
        else:
            raise AssertionError(text)


def method_argument_lifetime():
    import gc

    events = []

    class Target:
        def __call__(self, value):
            events.append(value)

        def __del__(self):
            events.append("released")

    class Holder:
        pass

    holder = Holder()

    def argument():
        del holder.method
        gc.collect()
        events.append("argument")
        return "called"

    for source in ("holder.method(argument())", "holder.method(value=argument())"):
        holder.method = Target()
        exec(source, {"holder": holder, "argument": argument})
        assert events == ["argument", "called", "released"], events
        events.clear()


def source_eval():
    make = tuple
    assert eval("make([2, 1])") == (2, 1)
    assert eval("(make := 3)") == 3 and make is tuple
    supplied = {"a": 9}
    code = compile("a * 2", "<expr>", "eval")
    assert eval(code, supplied) == 18
    assert eval(code, globals=supplied, locals={"a": 1}) == 2
    assert eval("len([])", {"__builtins__": {"len": lambda value: 17}}) == 17
    try:
        eval("a = 3")
    except SyntaxError:
        pass
    else:
        raise AssertionError("eval accepted a statement")


def starred_calls():
    events = []

    def mark(value):
        events.append(value)
        return value

    def spread():
        mark("iter")
        yield 2

    def collect(*values, label):
        return values, label

    scope = {"collect": collect, "mark": mark, "spread": spread}
    exec("answer = collect(*spread(), label=mark(3))", scope)
    assert scope["answer"] == ((2,), 3) and events == [3, "iter"]
    events.clear()
    exec("answer = collect(mark(1), *spread(), *(), label=mark(3))", scope)
    assert scope["answer"] == ((1, 2), 3) and events == [1, "iter", 3]

    for source in ("collect(**{}, label=1, label=2)", "collect(label=1, **{}, label=2)"):
        events.clear()
        try:
            exec("mark(0)\n" + source, scope)
        except SyntaxError as error:
            assert error.msg == "keyword argument repeated: label" and not events
            assert error.filename == "<string>" and (error.lineno, error.end_lineno) == (2, 2)
            assert (error.offset, error.end_offset) == (24, 31)
        else:
            raise AssertionError("duplicate explicit keyword compiled")

    try:
        compile("x=1\rcollect(a=1, a=2)(b=1, b=2)", "<caller>", "exec")
    except SyntaxError as error:
        assert error.msg == "keyword argument repeated: b"
        assert (error.lineno, error.offset, error.end_lineno, error.end_offset) == (2, 24, 2, 27)
    else:
        raise AssertionError("invalid outer call accepted")

    class Options:
        def keys(self):
            return ["label"]

        def __getitem__(self, key):
            return mark(4)

    scope["options"] = Options()
    events.clear()
    exec("answer = collect(*spread(), **options)", scope)
    assert scope["answer"] == ((2,), 4) and events == [4, "iter"]

    events.clear()
    try:
        exec("collect(*spread(), **options, label=mark(5), other=mark(6), **mark(7))", scope)
    except TypeError as error:
        assert "got multiple values for keyword argument 'label'" in str(error)
        assert events == [4, 5, 6]
    else:
        raise AssertionError("duplicate keyword was accepted")

    events.clear()
    try:
        exec("collect(*spread(), **{1: mark(5)})", scope)
    except TypeError as error:
        assert error.args == ("keywords must be strings",) and events == [5, "iter"]
    else:
        raise AssertionError("non-string keyword was accepted")

    class NoIter:
        __iter__ = None

    class BrokenIter:
        def __iter__(self):
            raise TypeError("from iterator")

    scope["no_iter"] = NoIter()
    scope["broken_iter"] = BrokenIter()
    for source, message in (
        ("collect(*42)", f"{collect.__module__}.{collect.__qualname__}() argument after * must be an iterable, not int"),
        ("collect(1, *42)", "Value after * must be an iterable, not int"),
        ("collect(*no_iter)", "'NoIter' object is not iterable"),
        ("collect(1, *broken_iter)", "from iterator"),
    ):
        try:
            exec(source, scope)
        except TypeError as error:
            assert str(error) == message
        else:
            raise AssertionError("invalid positional unpack was accepted")

    class BadHint:
        def __iter__(self):
            return iter(())

        def __length_hint__(self):
            raise LookupError("bad hint")

    scope["bad_hint"] = BadHint()
    exec("answer = collect(*bad_hint, label=0)", scope)
    assert scope["answer"] == ((), 0)
    try:
        exec("collect(1, *bad_hint, label=0)", scope)
    except LookupError as error:
        assert error.args == ("bad hint",)
    else:
        raise AssertionError("starred length hint failure was lost")

    def broken():
        yield 0
        raise ValueError("broken iterator")

    scope["broken"] = broken
    try:
        exec("collect(*broken(), label=0)", scope)
    except ValueError as error:
        assert error.args == ("broken iterator",)
    else:
        raise AssertionError("starred iterator failure was lost")


def current():
    starred_calls()
    source_eval()
    method_argument_lifetime()
    named_bindings()
    source_annotations()
    source_genexpr()
    source_match()
    check_scope_directives()
    relative_import_hooks()
    source_name_deletion()
    source_classes()
    source_test_cases()
    source_nested_functions()
    check_source_lambdas()
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
    exec("assert ... is expected", {"expected": Ellipsis, "Ellipsis": None})
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


def source_nested_functions():
    namespace = {"__name__": "closures", "left": "global"}
    exec(compile("""def outer(first):
    left = first
    def mid():
        def inner(arg=left):
            return left, arg
        return inner
    def sibling():
        return left
    inner = mid()
    left = first + 1
    return inner, sibling

def missing():
    def read():
        return left
    return read
    left = 1
""", "closures.py", "exec"), namespace)
    inner, sibling = namespace["outer"](7)
    assert inner() == (8, 7)
    assert sibling() == 8
    assert inner.__closure__[0] is sibling.__closure__[0]
    assert inner.__code__.co_freevars == ("left",)
    assert inner.__qualname__ == "outer.<locals>.mid.<locals>.inner"
    second = namespace["outer"](11)[0]
    assert second.__code__ is inner.__code__
    assert second.__closure__[0] is not inner.__closure__[0]
    assert second() == (12, 11)
    try:
        namespace["missing"]()()
    except NameError as error:
        assert "free variable 'left'" in str(error)
    else:
        raise AssertionError("unbound closure read a global")



def check_source_lambdas():
    namespace = {}
    source = """events = []
def make(x):
    def default():
        events.append(x)
        return x
    cb = lambda arg=default(), /, *, suffix=2: (x, arg, suffix)
    x += 1
    return cb

maker = lambda x: lambda y: x + y
def shadow(x):
    return lambda x=x: x
gen = lambda: (yield "ready")

class Example:
    def callback(self):
        return lambda: __class__

def from_class(x):
    class Inner:
        x = 99
        callback = lambda self: x
    return Inner().callback
"""
    exec(compile(source, "callbacks.py", "exec"), namespace)
    make = namespace["make"]
    first = make(4)
    second = make(7)
    assert namespace["events"] == [4, 7]
    assert first() == (5, 4, 2)
    assert first(9, suffix=1) == (5, 9, 1)
    assert second() == (8, 7, 2)
    assert first.__code__ is second.__code__
    assert first.__closure__[0] is not second.__closure__[0]
    assert first.__code__.co_freevars == ("x",)
    assert first.__name__ == "<lambda>"
    assert first.__qualname__ == "make.<locals>.<lambda>"
    assert first.__code__.co_firstlineno == 6
    try:
        first(arg=1)
    except TypeError:
        pass
    else:
        raise AssertionError("accepted positional-only argument")
    assert namespace["shadow"](9).__closure__ is None
    assert namespace["shadow"](9)() == 9
    assert namespace["maker"](10)(3) == 13
    instance = namespace["Example"]()
    assert instance.callback()() is type(instance)
    callback = namespace["from_class"](3)
    assert callback() == 3
    assert callback.__func__.__code__.co_freevars == ("x",)
    stream = namespace["gen"]()
    assert next(stream) == "ready"
    try:
        stream.send(11)
    except StopIteration as error:
        assert error.value == 11
    else:
        raise AssertionError("generator did not return")


def check_scope_directives():
    namespace = {"x": 90}
    source = """def enclosing(x):
    def update(value):
        if False:
            nonlocal x
        x = value
    def remove():
        nonlocal x
        del x
    class Owner:
        x = 99
        def read(self):
            nonlocal x
            return x
    def write_global(value):
        if False:
            global x
        x = value
    def generate():
        nonlocal x
        yield x
        x += 1
        yield x
    return update, remove, Owner().read, write_global, generate

def skip_class(x):
    class C:
        global x
        def read(self):
            nonlocal x
            return x
    return C().read()
"""
    exec(compile(source, "directives.py", "exec"), namespace)
    update, remove, read, write_global, generate = namespace['enclosing'](2)
    assert read() == 2
    update(7)
    assert read() == 7
    assert namespace['x'] == 90
    assert update.__code__.co_freevars == ("x",)
    assert update.__closure__[0] is remove.__closure__[0]
    write_global(13)
    assert namespace['x'] == 13
    assert read() == 7
    assert namespace['skip_class'](2) == 2
    it = generate()
    assert next(it) == 7
    assert next(it) == 8
    assert read() == 8
    remove()
    try:
        read()
    except NameError:
        pass
    else:
        raise AssertionError("nonlocal delete left a binding")
    update(5)
    assert read() == 5

    local = {}
    exec("if False:\n    global x\nx = 11\nlocal_only = x", namespace, local)
    assert namespace['x'] == 11 and local == {"local_only": 11}
    separate = {}
    exec("y = 7\ndef writes():\n    global y", namespace, separate)
    assert namespace['y'] == 7 and 'y' not in separate
    compile("def f():\n    (lambda: x)\n    global x", "valid.py", "exec")
    compile("def f():\n    [x for i in []]\n    global x", "valid.py", "exec")
    exec("""class C:
    def read(self):
        return __class__
    def make_writer(self):
        def write(value):
            nonlocal __class__
            __class__ = value
        return write
    def clear(self):
        nonlocal __class__
        del __class__
""", namespace)
    namespace['__class__'] = False
    obj = namespace['C']()
    assert obj.read() is type(obj)
    write = obj.make_writer()
    write(int)
    assert obj.read() is int
    obj.clear()
    try:
        obj.read()
    except NameError:
        pass
    else:
        raise AssertionError("class cell was not cleared")
    write(str)
    assert obj.read() is str
    for bad in (
        "nonlocal x",
        "def f():\n    nonlocal x",
        "def f(x):\n    global x",
        "def f(x):\n    nonlocal x",
        "def f():\n    x = 5\n    global x",
        "def f():\n    print(x)\n    global x",
        "def f():\n    global x\n    nonlocal x",
        "def f():\n    global x\n    x: int",
        "def f():\n    [i for i in x]\n    global x",
        "def f(x):\n    def mid():\n        global x\n        def child():\n            nonlocal x",
    ):
        try:
            compile(bad, "invalid.py", "exec")
        except SyntaxError:
            pass
        else:
            raise AssertionError(bad)



if __name__ == "__main__":
    current()

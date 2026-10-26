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


def current():
    namespace = {"__name__": "template", "events": [], "external": 41, "default": 100}
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

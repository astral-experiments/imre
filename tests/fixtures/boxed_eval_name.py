import builtins
import sys

sys.path.append("unused")
eval_function = builtins.__dict__["eval"]


def evaluate(source, globals, locals=None):
    return eval_function(source, globals, locals)


namespace = {"Element": str, "Global": bytes}
print("precedence", evaluate("Element", namespace, {"Element": int}) is int)
print("globals", evaluate("Element", namespace, {}) is str)
custom = {"__builtins__": {"Token": bytes}}
print("builtins", evaluate("Token", custom, {}) is bytes)
injected = {}
print("injected", evaluate("int", injected) is int, "__builtins__" in injected)


class Lookup:
    def __getitem__(self, key):
        if key == "Element":
            return float
        raise KeyError(key)


print("mapping", evaluate("Element", namespace, Lookup()) is float)
print("fallback", evaluate("Global", namespace, Lookup()) is bytes)


class FailingLookup:
    def __getitem__(self, key):
        raise ValueError(key)


try:
    evaluate("Element", namespace, FailingLookup())
except ValueError as error:
    print("hook-error", str(error))
try:
    evaluate("(\n Missing\n)", {})
except NameError as error:
    tb = error.__traceback__
    while tb.tb_next is not None:
        tb = tb.tb_next
    print("missing", error.name, tb.tb_frame.f_code.co_filename, tb.tb_lineno)
print("unicode", evaluate("Ｋey", {"Key": 23}))
print("bytes", evaluate(b"# coding: latin-1\ncaf\xe9", {"caf\u00e9": 31}))
try:
    evaluate("1+", {})
except SyntaxError as error:
    print("syntax", error.__class__.__name__)

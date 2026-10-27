"""Transport native literal values to the original CPython TestCase assertions."""
import ast
import json
import sys


def encode(value):
    if isinstance(value, complex):
        return ["complex", [value.real, value.imag]]
    if isinstance(value, bytes):
        return ["bytes", list(value)]
    if isinstance(value, dict):
        return ["dict", [[encode(k), encode(v)] for k, v in value.items()]]
    if isinstance(value, (list, tuple, set)):
        return [type(value).__name__, [encode(v) for v in value]]
    if value is Ellipsis:
        return ["ellipsis", None]
    return ["scalar", value]


def run(node_or_string):
    try:
        value = ast.literal_eval(node_or_string)
    except Exception as error:
        result = ["error", type(error).__name__, str(error)]
    else:
        result = ["value", encode(value)]
    print(json.dumps(result))


run(sys.argv[1])

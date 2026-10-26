"""Write an executable probe with Werkzeug's unchanged access-handler selection."""
import argparse
import ast
import hashlib
from pathlib import Path
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("werkzeug_wheel", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    with zipfile.ZipFile(args.werkzeug_wheel) as wheel:
        source = wheel.read("werkzeug/_internal.py")
    if hashlib.sha256(source).hexdigest() != "411ca77d410e9e01009a6e2ace196e7db864d4afa7f74d50d4a4162b39ec68d5":
        raise ValueError("expected the locked Werkzeug 3.1.8 _internal.py")
    method = next(
        node for node in ast.parse(source).body
        if isinstance(node, ast.FunctionDef) and node.name == "_has_level_handler"
    )
    body = b"".join(source.splitlines(keepends=True)[method.lineno - 1:method.end_lineno])
    args.output.write_bytes(b"from __future__ import annotations\n" + body + FIXTURE)


FIXTURE = b"""
class Logger:
    def __init__(self, handlers, parent=None, propagate=True):
        self.handlers = handlers
        self.parent = parent
        self.propagate = propagate

    def getEffectiveLevel(self):
        return 20


class Handler:
    def __init__(self, level):
        self.level = level


class Unreached:
    @property
    def level(self):
        raise AssertionError("handler scan continued after a match")


parent = Logger([Handler(20), Unreached()])
assert _has_level_handler(Logger([], parent))
assert not _has_level_handler(Logger([], parent, False))
assert _has_level_handler(Logger([Handler(30), Handler(10), Unreached()]))
assert not _has_level_handler(Logger([Handler(30)]))
print("four handler scans matched")
"""


if __name__ == "__main__":
    main()

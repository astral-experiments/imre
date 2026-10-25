"""Compile CPython's two cache-name validation branches, using its public APIs as the oracle."""
import ast
import hashlib
from importlib import _bootstrap_external, util
from pathlib import Path
from textwrap import dedent, indent


def cache_guards():
    text = Path(_bootstrap_external.__file__).read_text(encoding="utf8")
    assert hashlib.sha256(text.encode()).hexdigest() == "b9af1fb7b625030cf5ee06b39aeb7838c2a30c97ddee09410286b7afbd8b7098"
    functions = {f.name: f for f in ast.parse(text).body if isinstance(f, ast.FunctionDef)}
    lines = text.splitlines(keepends=True)

    def original_guard(function):
        branch, = (node for node in ast.walk(functions[function])
                   if isinstance(node, ast.If) and isinstance(node.test, ast.UnaryOp)
                   and isinstance(node.test.operand, ast.Call)
                   and isinstance(node.test.operand.func, ast.Attribute)
                   and node.test.operand.func.attr == "isalnum")
        return indent(dedent("".join(lines[branch.lineno - 1:branch.end_lineno])), "    ")

    # The two locals and forward empty-string check come from surrounding path logic.
    # Only the predicate and raise statement below execute as original native source.
    source = ("def forward(optimization: str) -> None:\n"
              "    if optimization == '':\n        return\n"
              + original_guard("cache_from_source")
              + "def reverse(opt_level: str) -> None:\n"
                "    optimization = 'opt-' + opt_level\n"
              + original_guard("source_from_cache"))
    tokens = ["", "0", "abc123", "é", "é१", "½", "Ⅻ", "①", "𐐀", "🄇", "A🄇9",
              "_", "x_y", "x ", "!", "🐍", "\u0301", "a\u0301", "\x00", "\n", "\u0378"]
    expected = []
    for name in ("forward", "reverse"):
        source += f"for token in {tokens!r}:\n    try:\n        {name}(token)\n        print('ok')\n"
        source += "    except ValueError as error:\n        print('ValueError: ' + str(error))\n"
        for token in tokens:
            try:
                if name == "forward":
                    util.cache_from_source("pkg/mod.py", optimization=token)
                else:
                    util.source_from_cache(f"pkg/__pycache__/mod.tag.opt-{token}.pyc")
            except ValueError as error:
                expected.append(f"ValueError: {error}")
            else:
                expected.append("ok")
    return source, expected

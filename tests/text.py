"""Compare native text parsing, fixed regexes and bounded URL decoding with CPython."""

import argparse
import ast
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from comprehensions import require_result, run

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "text"
STDLIB = FIXTURES.parents[2] / "benchmarks" / "cpython_stdlib"
REJECTED = {
    "formatter_unknown_export": "from _string import no_such_export",
    "formatter_unknown_neighbor": "import _string, missing_formatter_neighbor",
    "formatter_nested_argument": 'from _string import formatter_parser\ndef number(item: int) -> int:\n    return item\nformatter_parser(number("bad"))',
    "isalnum_union_descriptor_receiver": "def test(value: str | int) -> bool:\n    return str.isalnum(value)",
    "isalnum_nested_descriptor_argument": "def required(value: int) -> int:\n    return value\nstr.isalnum(required())",
    "ord_bytearray": 'ord(bytearray(b"x"))',
    "ord_memoryview": 'ord(memoryview(b"x"))',
    "ord_nested_argument": 'def required(x: str) -> str:\n    return x\nord(c=required())',
    "splitlines_nested_argument": 'def number(value: int) -> int:\n    return value\n"x".splitlines(number("x"))',
    "isspace_nested_argument": 'def number(value: int) -> int:\n    return value\n"x".isspace(number("x"))',
    "splitlines_unproven_unpack": 'def lines(options: list[int]) -> list[str]:\n    return "x".splitlines(*options)',
    "expandtabs_deprecated_index": "class Width:\n    def __index__(self) -> bool:\n        return True\n'x'.expandtabs(Width())",
    "expandtabs_dynamic_args": "def run(args: list[int]) -> str:\n    return 'x'.expandtabs(*args)",
    "split_bytes": '"abc".split(b"b")',
    "split_wrong_limit": '"abc".split("b", True)',
    "split_duplicate": '"abc".split("b", sep="c")',
    "split_unknown_keyword": '"abc".split(limit=1)',
    "split_extra": '"abc".split("b", 1, 2)',
    "strip_keyword": '"abc".strip(chars="c")',
    "strip_wrong_type": '"abc".lstrip(1)',
    "startswith_mixed_tuple": '"abc".startswith(("x", 1))',
    "startswith_bounds": '"abc".startswith("a", 0)',
    "endswith_keyword": '"abc".endswith(suffix="c")',
    "find_keyword": '"abc".find("a", start=0)',
    "find_end": '"abc".find("a", 0, 1)',
    "find_wrong_start": '"abc".find("a", True)',
    "partition_keyword": '"abc".partition(sep="b")',
    "partition_missing": '"abc".partition()',
    "translate_sequence": 'def tr(table: list[str]) -> str:\n    return "a".translate(table)',
    "translate_counter_missing": 'from collections import Counter\ntable = Counter({97: 98})\n"a".translate(table)',
    "translate_nested_keyword": 'def number(value: int, /) -> int:\n    return value\n"a".translate(table=number(value=1))',
    "unquote_bytes": 'from urllib.parse import unquote\nunquote(b"abc")',
    "unquote_codec": 'from urllib.parse import unquote\nunquote("abc", encoding="ascii")',
    "unquote_errors": 'from urllib.parse import unquote\nunquote("abc", errors="strict")',
    "unsupported_url_api": 'from urllib.parse import urljoin\nurljoin("http://host", "child")',
    "escaped_unquote": 'from urllib.parse import unquote\ncallback = unquote',
    "unused_invalid_method": 'def unused(value: str) -> str:\n    return value.strip(chars="x")',
    "regex_dynamic": 'import re\ndef bad(value: str) -> None:\n    re.compile(value)',
    "regex_bytes": 'import re\nre.compile(b"x")',
    "regex_flags": 'import re\nre.compile("x", re.IGNORECASE)',
    "regex_compiled_flags": 'import re\np = re.compile("x")\nre.search(p, "x", re.UNICODE)',
    "regex_search_bytes": 'import re\nre.search("x", b"x")',
    "regex_dynamic_group": 'import re\ndef bad(index: int) -> None:\n    if m := re.search("(x)", "x"):\n        m.group(index)',
    "regex_unchecked_match": 'import re\nre.search("x", "y").start()',
    "regex_match_bounds": 'import re\nre.compile("x").search("text", 2)',
    "regex_backreference": 'import re\nre.compile(r"(x)\\1")',
    "regex_lookbehind": 'import re\nre.compile(r"(?<=x)y")',
    "regex_interior_end_anchor": 'import re\nre.compile("x$y")',
    "regex_optional_group_as_str": 'import re\ndef bad() -> str:\n    if m := re.search("(?P<x>x)?z", "z"):\n        a, = m.groups()\n        return a\n    return ""',
    "regex_aliased_dict_as_str": 'import re\ndef bad() -> str:\n    match = re.search("(?P<x>x)", "x")\n    if match is None:\n        return ""\n    groups = match.groupdict()\n    alias = groups\n    alias["x"] = None\n    return groups["x"]',
    "regex_nested_repeat": 'import re\nre.compile("(x?)*")',
    "sub_nullable": 'import re\nre.sub("x*", "!", "xxx")',
    "sub_invalid_group": 'import re\nre.sub("(x)", r"\\2", "none")',
    "sub_unclosed_group": 'import re\nre.sub("(x)", r"\\g<1", "none")',
    "sub_dynamic_template": 'import re\ndef bad(s: str) -> str:\n    return re.sub("x", s, "x")',
    "regex_invalid_unused": 'import re\ndef unused() -> None:\n    re.compile("[")',
}


def cpython_sub_cases(directory):
    """Keep the three upstream bodies byte-identical, including their expected values."""
    sys.path.insert(0, str(STDLIB))
    from upstream import verify_upstream

    verify_upstream()
    data = (STDLIB / "upstream/Lib/test/test_re.py").read_bytes()
    cls = next(n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == "ReTests")
    adapter = b"""import re

class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertEqual(self, actual: str, expected: str) -> None:
        assert actual == expected
        self.checks += 1

"""
    for name, count in (("test_bug_449964", 1), ("test_bug_449000", 4), ("test_bug_114660", 1)):
        method = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == name)
        assert not method.decorator_list
        body = b"".join(data.splitlines(keepends=True)[method.lineno - 1:method.end_lineno])
        entry = f"\ncase = Case()\ncase.{name}()\nassert case.checks == {count}\n"
        source = directory / (name + ".py")
        source.write_bytes(adapter + body + entry.encode())
        yield source


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
    if sys.flags.optimize:
        parser.error("validation requires assertions")
    compiler = str(args.compiler.resolve())
    flags = ["--quiet", "--runtime", str(args.runtime.resolve()), "--clang", args.clang]
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags += ["--sanitize"]
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-text-") as directory:
        temporary = Path(directory)
        for source in [*sorted(FIXTURES.glob("*.py")), *cpython_sub_cases(temporary)]:
            expected = run([args.python, "-I", "-B", str(source)], temporary)
            require_result(expected, None)
            for level in args.opt_levels:
                binary = temporary / f"{source.stem}-O{level}"
                built = run([
                    compiler, "build", str(source), "--module-root", str(source.parent),
                    "-o", str(binary), "--opt-level", level, *flags,
                ], temporary)
                require_result(built, None)
                actual = run([str(binary)], temporary)
                require_result(actual, None)
                assert actual.stdout == expected.stdout, (source.name, level, actual.stdout, expected.stdout)
                checks += 1
        for name, source in REJECTED.items():
            path = temporary / f"{name}.py"
            path.write_text(source + "\n", encoding="utf-8")
            result = run([compiler, "check", str(path)], temporary)
            assert result.returncode > 0 and result.stderr and "panicked" not in result.stderr, (name, result)
            checks += 1
    print(f"Passed {checks} text checks ({len(REJECTED)} rejections; optimization levels {args.opt_levels}; ASan={args.sanitize})")


if __name__ == "__main__":
    main()

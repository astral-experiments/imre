"""Compare native text parsing, fixed regexes and bounded URL decoding with CPython."""

import argparse
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from comprehensions import require_result, run

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "text"
REJECTED = {
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
    "unquote_bytes": 'from urllib.parse import unquote\nunquote(b"abc")',
    "unquote_codec": 'from urllib.parse import unquote\nunquote("abc", encoding="ascii")',
    "unquote_errors": 'from urllib.parse import unquote\nunquote("abc", errors="strict")',
    "unsupported_url_api": 'from urllib.parse import urlsplit\nurlsplit("a:b")',
    "escaped_unquote": 'from urllib.parse import unquote\ncallback = unquote',
    "unused_invalid_method": 'def unused(value: str) -> str:\n    return value.strip(chars="x")',
    "regex_dynamic": 'import re\ndef bad(value: str) -> None:\n    re.compile(value)',
    "regex_bytes": 'import re\nre.compile(b"x")',
    "regex_flags": 'import re\nre.compile("x", re.IGNORECASE)',
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
    "regex_invalid_unused": 'import re\ndef unused() -> None:\n    re.compile("[")',
}


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
    flags = ["--runtime", str(args.runtime.resolve()), "--clang", args.clang]
    if args.target:
        flags += ["--target", args.target]
    if args.lto_linker:
        flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.sanitize:
        flags += ["--sanitize"]
    checks = 0
    with tempfile.TemporaryDirectory(prefix="relar-text-") as directory:
        temporary = Path(directory)
        for source in sorted(FIXTURES.glob("*.py")):
            expected = run([args.python, "-I", "-B", str(source)], temporary)
            require_result(expected, None)
            for level in args.opt_levels:
                binary = temporary / f"{source.stem}-O{level}"
                built = run([
                    compiler, "build", str(source), "--module-root", str(FIXTURES),
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

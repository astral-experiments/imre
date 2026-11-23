"""Compare Imre's decisions on suite programs with and without an unused import.

Use Python 3.14 to parse the current suites. --tests can point to the tests
directory of another trusted checkout to keep the inputs fixed across revisions.
The default run also enforces each suite's declared outcome. When comparing
against another revision, --parity-only checks just the effect of each import.
"""

import argparse
import ast
import runpy
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

SUITES = (
    "assignments",
    "callables",
    "calls",
    "classes",
    "comprehensions",
    "containers",
    "dataclass_records",
    "expressions",
    "floats",
    "inference",
    "iterables",
    "iterator_pipelines",
    "matching",
    "module_values",
    "primitives",
    "truthiness",
)
PEER = "_imre_probe_peer"
UNUSED = "_imre_probe_import"
IMPORTS = (
    "import unittest",
    f"import {PEER}",
    "from unittest import TestCase",
    f"from {PEER} import value",
)


def programs(tests):
    for suite in SUITES:
        data = runpy.run_path(str(tests / f"{suite}.py"))
        for kind in ("CHECKED", "REJECTED", "IMPORTED_REJECTIONS"):
            for name, files in data.get(kind, {}).items():
                if isinstance(files, str):
                    files = {"main.py": files}
                if isinstance(files, dict) and "main.py" in files:
                    yield f"{suite}.{name}", files, kind == "CHECKED"


def insertion_line(source):
    module = ast.parse(source)
    body = iter(module.body)
    at = 0
    if ast.get_docstring(module, clean=False) is not None:
        at = next(body).end_lineno
    for node in body:
        if not isinstance(node, ast.ImportFrom) or node.module != "__future__":
            break
        at = node.end_lineno
    return at


def compare(compiler, case, parity_only):
    name, files, should_compile = case
    source = files["main.py"]
    try:
        at = insertion_line(source)
    except SyntaxError:
        return None
    assert all(UNUSED not in text and PEER not in text for text in files.values())
    lines = source.splitlines(keepends=True)
    if at and not lines[at - 1].endswith("\n"):
        lines[at - 1] += "\n"
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for key, text in files.items():
            path = root / key
            path.parent.mkdir(exist_ok=True, parents=True)
            path.write_text(text)

        def check():
            p = subprocess.run(
                [compiler, "check", "main.py", "--module-root", tmp],
                cwd=tmp,
                check=False,
                text=True,
                capture_output=True,
                timeout=120,
            )
            if p.returncode not in (0, 1) or "panicked" in p.stderr:
                raise RuntimeError(f"{name} crashed ({p.returncode})\n{p.stderr}")
            return p

        baseline = check()
        changes = []
        if not parity_only and (baseline.returncode == 0) != should_compile:
            expected = "accept" if should_compile else "reject"
            print(
                f"{name} should {expected} without extra imports\n{baseline.stderr.strip()}",
                flush=True,
            )
            changes.append("baseline")
        for statement in IMPORTS:
            if PEER in statement:
                peer = "print(bytearray(b'initialized'))\n"
                if statement.startswith("from"):
                    peer = "value = bytearray(b'initialized')\nprint(value)\n"
                (root / f"{PEER}.py").write_text(peer)
            modified = lines[:at] + [f"{statement} as {UNUSED}\n"] + lines[at:]
            (root / "main.py").write_text("".join(modified))
            variant = check()
            if baseline.returncode != variant.returncode:
                failure = baseline if baseline.returncode else variant
                print(
                    f"{name} changes with {statement}\n{failure.stderr.strip()}",
                    flush=True,
                )
                changes.append(statement)
        return changes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--tests", type=Path, default=Path(__file__).parent)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--parity-only", action="store_true")
    args = parser.parse_args()
    compiler = args.compiler.resolve(strict=True)
    cases = list(programs(args.tests.resolve(strict=True)))
    with ThreadPoolExecutor(args.jobs) as pool:
        results = list(
            pool.map(lambda case: compare(compiler, case, args.parity_only), cases)
        )
    compared = [r for r in results if r is not None]
    print(
        f"Compared {len(compared)} programs. Skipped {len(results) - len(compared)} unparsable programs."
    )
    if not args.parity_only:
        print(
            f"{sum('baseline' in r for r in compared)} unexpected baseline decisions."
        )
    for statement in IMPORTS:
        print(
            f"{sum(statement in r for r in compared)} decisions changed with {statement}."
        )
    return bool(any(compared) or not compared)


if __name__ == "__main__":
    raise SystemExit(main())

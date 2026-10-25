"""Try every original JSON unittest method with its referenced globals and native fixture bindings."""

import argparse
import ast
from collections import Counter, defaultdict
import doctest
import hashlib
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference import BENCH, COMMIT, routine, verify
sys.path.insert(0, str(BENCH))
from suite_inventory import cases, load_suite
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures


def bindings(node):
    if isinstance(node, (ast.Import, ast.ImportFrom)):
        return [a.asname or a.name.partition(".")[0] for a in node.names]
    if isinstance(node, (ast.FunctionDef, ast.ClassDef)):
        return [node.name]
    if isinstance(node, ast.Assign):
        return [t.id for t in node.targets if isinstance(t, ast.Name)]
    return []


def prepare(path, owner, method):
    """Copy referenced module declarations and class helpers from the same pinned file."""
    lines = path.read_bytes().splitlines(keepends=True)
    module = ast.parse(b"".join(lines))
    cls, = [n for n in module.body if isinstance(n, ast.ClassDef) and n.name == owner]
    target, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == method]
    top = {name: n for n in module.body for name in bindings(n) if n is not cls}
    members = {name: n for n in cls.body for name in bindings(n) if n is not target}
    done, helpers, globals_, attrs = set(), set(), set(), set()
    pending = [target]
    while pending:
        part = pending.pop()
        if part in done:
            continue
        done.add(part)
        for n in ast.walk(part):
            if isinstance(n, ast.Name) and isinstance(n.ctx, ast.Load) and n.id in top:
                dep = top[n.id]
                globals_.add(dep)
                pending.append(dep)
            if (isinstance(n, ast.Attribute) and isinstance(n.value, ast.Name)
                    and n.value.id == "self"):
                attrs.add(n.attr)
                if n.attr in members:
                    dep = members[n.attr]
                    helpers.add(dep)
                    pending.append(dep)

    def original(n):
        start = min([n.lineno, *(d.lineno for d in getattr(n, "decorator_list", ()))])
        return b"".join(lines[start - 1:n.end_lineno]).decode()

    prelude = "import json\nimport unittest\n"
    prelude += "".join(original(n) + "\n" for n in module.body
                       if n in globals_ and not (isinstance(n, ast.Import) and
                           all(a.name in {"unittest", "json"} and not a.asname for a in n.names)))
    prelude += "\nclass NativeCase(unittest.TestCase):\n"
    # The C and Python reference fixtures bind the same public APIs to different
    # reference modules. Both native fixtures bind them to the sole Imre module.
    for field in ("json", "loads", "dumps", "JSONDecodeError"):
        if field in attrs:
            binding = "json" if field == "json" else f"staticmethod(json.{field})"
            prelude += f"    {field} = {binding}\n"
    prelude += "".join(original(n) + "\n" for n in cls.body if n in helpers)
    return prelude


def diagnostic(exc, work):
    lines = [s for s in str(exc).splitlines() if s.strip() and not s.strip().startswith("Compiling ")]
    return (lines or [str(exc)])[0].replace(str(work), "<case>")


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def run(tools, directory):
    suite = load_suite("test.test_json")
    ids = defaultdict(list)
    for case in cases(suite):
        if not isinstance(case, doctest.DocTestCase):
            ids[routine(case)].append(case.id())
    results, executions = {}, {}
    for source, names in ids.items():
        mod, owner, method = source.rsplit(".", 2)
        path = BENCH / "upstream/Lib" / mod.replace(".", "/")
        path = path / "__init__.py" if path.is_dir() else path.with_suffix(".py")
        row = results[source] = {"fixture_ids": names}
        if source.endswith("TestCEncodeBasestringAscii.test_overflow"):
            row.update(outcome="reference-skip", reason="pinned upstream bigaddrspacetest memory gate")
            continue
        fixture = prepare(path, owner, method)
        row["fixture_sha256"] = hashlib.sha256(fixture.encode()).hexdigest()
        work = directory / source
        work.mkdir()
        try:
            callback = native_method(tools, work, path, owner, names[0], fixture)
        except (NativeCompileError, ValueError) as exc:
            row.update(outcome="blocked", reason=diagnostic(exc, work))
            continue
        if mod.endswith(".test_tool"):
            # A native unittest body alone cannot establish which executable its child ran.
            row.update(outcome="child-unverified", reason="native module-launch provenance requires the CLI gate")
            continue
        row["outcome"] = "compiled"
        executions.update(dict.fromkeys(names, callback))
    complete, errors, skips = run_in_fixtures(suite, executions)
    for row in results.values():
        if row["outcome"] == "compiled":
            row["executions"] = {name: complete.get(name, []) for name in row["fixture_ids"]}
            row_errors = [(name, message.splitlines()[-1]) for name, message in errors
                          if name in row["fixture_ids"]]
            row_skips = [(name, reason) for name, reason in skips if name in row["fixture_ids"]]
            if row_errors or row_skips:
                row.update(outcome="failed", errors=row_errors, skips=row_skips)
            elif all(len(complete.get(name, [])) >= row["fixture_ids"].count(name)
                     for name in row["fixture_ids"]):
                row["outcome"] = "body-completed"
            else:
                raise ValueError(f"incomplete native fixture: {row}")
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    verify()
    suite = list(cases(load_suite("test.test_json")))
    out = {"cpython_commit": COMMIT, "denominator": len(suite),
           "unittest_occurrences": sum(not isinstance(c, doctest.DocTestCase) for c in suite),
           "doctest_occurrences": sum(isinstance(c, doctest.DocTestCase) for c in suite),
           "sha256": {name: digest(path)
                      for name, path in (("imre", args.imre), ("runtime", args.runtime))},
           "levels": {}}
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as work:
            rows = run(Toolchain(args.imre, args.runtime, args.clang, level), Path(work))
        out["levels"][str(level)] = rows
        print(f"-O{level}: {len(rows)} definitions, {dict(Counter(r['outcome'] for r in rows.values()))}", flush=True)
    args.result.write_text(json.dumps(out, indent=2) + "\n")


if __name__ == "__main__":
    main()

"""Try complete CPython shutil methods under their original host fixtures."""
import argparse
import ast
from contextlib import ExitStack, chdir
from functools import cache
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import types

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path[:0] = [str(HERE), str(STDLIB)]
import copyfile
import move_ops
import native_copy
import which_cases
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures, testcase_fixture

SOURCE = STDLIB / "upstream/Lib/test/test_shutil.py"
LINES = SOURCE.read_bytes().splitlines(keepends=True)
MODULE = ast.parse(b"".join(LINES))
CLASSES = {n.name: {m.name: m for m in n.body if isinstance(m, ast.FunctionDef)}
           for n in MODULE.body if isinstance(n, ast.ClassDef)}
FUNCTIONS = {n.name: n for n in MODULE.body if isinstance(n, ast.FunctionDef)}
ASSIGNS = {t.id: n for n in MODULE.body if isinstance(n, ast.Assign)
           for t in n.targets if isinstance(t, ast.Name)}
SCALARS = (type(None), str, bytes, bool, int, float)


def source(node, decorated=True):
    start = node.lineno
    if decorated and getattr(node, "decorator_list", ()):
        start = node.decorator_list[0].lineno
    return b"".join(LINES[start - 1:node.end_lineno])


def owner_of(case, method):
    return next((c.__name__ for c in type(case).__mro__
                 if method in CLASSES.get(c.__name__, {})), None)


@cache
def imports():
    result = {}
    for node in MODULE.body:
        for stmt in node.body if isinstance(node, ast.Try) else [node]:
            if isinstance(stmt, ast.ImportFrom):
                for name in stmt.names:
                    binding = name.asname or name.name
                    result[binding] = f"from {stmt.module} import {name.name}" + (
                        f" as {binding}" if name.asname else "")
    return result


def fixture(case, suite, node):
    # Known complete routines use their already verified native fixture bindings.
    if isinstance(case, suite.TestWhich) and node.name in {"test_basic", "test_absolute_cmd", "test_nonexistent_file"}:
        return which_cases.which_fixture(case)
    if isinstance(case, suite.TestCopy) and node.name == copyfile.NATIVE_CASE:
        return copyfile.copy_fixture(case)
    if isinstance(case, suite.TestCopy) and node.name == native_copy.NATIVE_RETURN_VALUE:
        return native_copy.return_value_fixture(case)
    if isinstance(case, suite.TestCopy):
        if node.name in {"test_copy_dir", "test_copy2_dir"}:
            return native_copy.native_path_fixture(case, 2) + source(CLASSES["TestCopy"]["_test_copy_dir"]).decode()
        if node.name in {"test_copyfile_same_file", "test_copyfile_copy_dir"}:
            return native_copy.native_path_fixture(case, 1 if node.name == "test_copyfile_same_file" else 2)
    if isinstance(case, suite.TestCopyTree) and node.name in {"test_copytree_return_value", "test_copytree_subdirectory"}:
        return native_copy.native_path_fixture(case, 1)
    if isinstance(case, suite.TestMove) and node.name in move_ops.NATIVE_METHODS:
        return move_ops.native_fixture(case, SOURCE, move_ops.NATIVE_METHODS[node.name])[0]
    helpers, globals_, values = {}, {}, {}
    needs_mock = False
    seen = set()

    def visit(tree):
        nonlocal needs_mock
        if id(tree) in seen:
            return
        seen.add(id(tree))
        for child in ast.walk(tree):
            if isinstance(child, ast.Attribute) and isinstance(child.value, ast.Name):
                if child.value.id == "unittest" and child.attr == "mock":
                    needs_mock = True
                if child.value.id == "self":
                    owner = owner_of(case, child.attr)
                    if owner and child.attr != node.name:
                        helper = CLASSES[owner][child.attr]
                        visit(helper)
                        helpers[child.attr] = source(helper).decode()
                    elif child.attr in case.__dict__:
                        value = case.__dict__[child.attr]
                        if type(value) in SCALARS:
                            values[child.attr] = value
            if not isinstance(child, ast.Name) or not isinstance(child.ctx, ast.Load):
                continue
            key = child.id
            if key in globals_ or key not in vars(suite):
                continue
            obj = getattr(suite, key)
            if key in FUNCTIONS:
                visit(FUNCTIONS[key])
                globals_[key] = source(FUNCTIONS[key]).decode()
            elif type(obj) in SCALARS:
                globals_[key] = f"{key} = {obj!r}"
            elif key in imports():
                globals_[key] = imports()[key]
            elif isinstance(obj, types.ModuleType):
                globals_[key] = "import " + obj.__name__ + (f" as {key}" if key != obj.__name__ else "")
            elif key in ASSIGNS:
                visit(ASSIGNS[key])
                globals_[key] = source(ASSIGNS[key]).decode()
            else:
                globals_[key] = f"from test.test_shutil import {key}"

    visit(node)
    additions = ["import unittest"]
    if needs_mock:
        additions.append("import unittest.mock")
    if values:
        # These are scalar fields of the real host instance after setUp.
        attrs = [f"    {key}: {type(value).__name__ if value is not None else 'None'}" for key, value in values.items()]
        attrs += ["    def __init__(self) -> None:", "        super().__init__()"]
        attrs += [f"        self.{key} = {value!r}" for key, value in values.items()]
    else:
        attrs = []
    declarations = []
    for stmt in globals_.values():
        if stmt.startswith(("from ", "import ")):
            additions.append(stmt)
        else:
            declarations.append(stmt)
    cls = testcase_fixture().split("\n\n", 1)[1]
    return "\n".join([*dict.fromkeys(additions), *declarations, cls, *attrs, *helpers.values(), ""])


def attempt(tools, case, suite, work, support):
    method = case._testMethodName
    owner = owner_of(case, method)
    node = CLASSES[owner][method]
    data = source(node)
    proof = {"owner": owner, "source": f"Lib/test/test_shutil.py:{node.lineno}",
             "method_sha256": hashlib.sha256(source(node, False)).hexdigest(),
             "decorated_sha256": hashlib.sha256(data).hexdigest(),
             "decorators": [ast.unparse(d) for d in node.decorator_list]}
    header = fixture(case, suite, node)
    with tempfile.TemporaryDirectory(dir=work) as tmp:
        folder = Path(tmp)
        # Imre correctly rejects sources reached via a symlink outside its
        # module root. Hardlink the verified fixture files into this entry root.
        for path in support.rglob("*"):
            dest = folder / "test" / path.relative_to(support)
            if path.is_dir():
                dest.mkdir(parents=True, exist_ok=True)
            else:
                dest.parent.mkdir(parents=True, exist_ok=True)
                preserve(path, dest)
        try:
            if not node.decorator_list:
                execute = native_method(tools, folder, SOURCE, owner, case.id(), header)
            else:
                # The shared harness rejects decorated methods. Compile the
                # entire decorated definition, including every original wrapper.
                marker = f"completed {case.id()}"
                entry = header.encode() + b"\n" + data + f"\ncase = NativeCase()\ncase.{method}()\nprint({marker!r})\n".encode()
                binary = tools.build(entry, folder)
                def execute(*_):
                    child = subprocess.run([binary], capture_output=True, text=True, timeout=60)
                    if (child.returncode, child.stderr, child.stdout) != (0, "", marker + "\n"):
                        raise AssertionError(f"native exit {child.returncode}: {child.stdout!r} {child.stderr!r}")
        except NativeCompileError as exc:
            diagnostic = str(exc).strip().splitlines()[-1]
            return dict(proof, status="compile-blocked",
                        reason=diagnostic.replace(str(folder) + "/", "").replace(str(folder), "<method>"))
        try:
            execute(case, None)
        except (AssertionError, RuntimeError, subprocess.TimeoutExpired) as exc:
            return dict(proof, status="failed", reason=str(exc).replace(str(folder) + "/", ""))
    return dict(proof, status="body-completed", fixture="original CPython module, class and instance fixtures; native child")


def preserve(src, dst):
    try:
        os.link(src, dst)
    except OSError:
        shutil.copyfile(src, dst)
        shutil.copymode(src, dst)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def compact(report):
    levels = report["optimization_levels"]
    if set(levels) != {0, 3} or levels[0].keys() != levels[3].keys():
        raise ValueError("incomplete optimization pair")
    result = {k: v for k, v in report.items() if k != "optimization_levels"}
    result.update(schema="imre.shutil-method-attempts.v1", denominator=len(levels[0]),
                  fixture_execution="original CPython module/class/instance hooks for every attempted method",
                  cases={})
    for name, first in levels[0].items():
        second = levels[3][name]
        result["cases"][name] = ([dict(first, opt_levels=[0, 3])] if first == second
                                 else [dict(first, opt_levels=[0]), dict(second, opt_levels=[3])])
    return result


def proof_for(report, name, level):
    record, = [r for r in report["cases"][name] if level in r["opt_levels"]]
    if record["status"] in {"reference-skip", "fixture-error", "fixture-skip"} and "owner" not in record:
        return record
    owner, method = record["owner"], name.rsplit(".", 1)[-1]
    original = CLASSES[owner][method]
    if (record["method_sha256"] != hashlib.sha256(source(original, False)).hexdigest() or
            record["decorated_sha256"] != hashlib.sha256(source(original)).hexdigest()):
        raise ValueError(f"{name}: complete-method proof does not match unchanged CPython source")
    return record


def main(stack):
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + key, type=lambda p: Path(p).resolve(), required=True)
    parser.add_argument("--only", action="append")
    parser.add_argument("--reference", type=Path, default=HERE / "reference-linux.json",
                        help="pinned suite_inventory output for the platform running the fixtures")
    parser.add_argument("--fixture-root", type=Path, default=HERE.parent / "shared/upstream")
    parser.add_argument("--fixture-manifest", type=Path, default=HERE.parent / "shared/UPSTREAM.json")
    parser.add_argument("--compiler-tree", required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("requires test-enabled CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    oracle = json.loads(args.reference.read_text())
    reference = oracle["reference"]
    if (oracle["module"] != manifest["module"] or oracle["cpython_commit"] != manifest["commit"]
            or oracle["platform"] != sys.platform or oracle["denominator"] != len(reference)):
        raise ValueError("reference does not describe this platform's pinned shutil suite")
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    shared = json.loads(args.fixture_manifest.read_text())
    if shared["commit"] != manifest["commit"]:
        raise ValueError("shared fixtures have the wrong CPython revision")
    fixtures = {path: blob for path, blob in shared["files"].items() if path.startswith("Lib/test/")}
    verify_files(args.fixture_root, fixtures)
    staging = Path(stack.enter_context(tempfile.TemporaryDirectory()))
    for path in fixtures:
        dest = staging / path
        dest.parent.mkdir(parents=True, exist_ok=True)
        preserve(args.fixture_root / path, dest)
    support = staging / "Lib/test"
    if not (support / "test_shutil.py").exists():
        preserve(SOURCE, support / "test_shutil.py")
    # Pin the same compiled pair for both levels, even if another local build
    # replaces a target while the sweep is running. Hardlinks cost no extra copy.
    imre, runtime = staging / "imre", staging / "libimre_runtime.a"
    preserve(args.imre, imre)
    preserve(args.runtime, runtime)
    import test
    test.__path__.insert(0, str(SOURCE.parent))
    report = {"cpython_commit": manifest["commit"], "compiler_sha256": digest(imre),
              "compiler_tree": args.compiler_tree, "runtime_sha256": digest(runtime),
              "fixture_manifest_sha256": digest(args.fixture_manifest), "reference_sha256": digest(args.reference),
              "optimization_levels": {}}
    for level in (0, 3):
        tools = Toolchain(imre, runtime, args.clang, level)
        rows = {}
        with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
            work = Path(tmp)
            from test import test_shutil as suite
            assert Path(suite.__file__).resolve() == SOURCE
            loaded = load_suite(manifest["module"])
            ids = [case.id() for case in cases(loaded)]
            assert len(ids) == len(set(ids)) and set(ids) == set(reference)
            if set(args.only or ()) - set(ids):
                raise ValueError("unknown occurrence")
            def run(case, original):
                row = attempt(tools, case, suite, work, support)
                rows[case.id()] = row
                return row
            selected = {}
            for name in ids:
                outcome = reference[name]
                if isinstance(outcome, dict) and "skipped" in outcome:
                    rows[name] = {"status": "reference-skip", "reason": outcome["skipped"]}
                elif args.only and name not in args.only:
                    rows[name] = {"status": "not-selected"}
                else:
                    selected[name] = run
            _, errors, skips = run_in_fixtures(loaded, selected)
            for name, detail in errors:
                rows[name] = {**rows.get(name, {}), "status": "fixture-error", "reason": detail.strip().splitlines()[-1]}
            for name, reason in skips:
                rows[name] = {"status": "fixture-skip", "reason": reason}
            assert set(rows) == set(ids)
        report["optimization_levels"][level] = dict(sorted(rows.items()))
    args.result.write_text(json.dumps(compact(report), indent=2) + "\n")


if __name__ == "__main__":
    with ExitStack() as stack:
        main(stack)

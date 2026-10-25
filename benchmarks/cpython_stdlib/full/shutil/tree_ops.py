"""Run the pinned tree-copy methods against native directory, file and link operations."""

import argparse
import ast
import builtins
from contextlib import chdir
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path[:0] = [str(HERE), str(STDLIB)]
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_evidence import roles
from suite_native import Toolchain, run_in_fixtures
from tree_error_controls import run as error_controls
from tree_callback_controls import run as callback_controls
from tree_filter_controls import run as filter_controls
import tree_host_callbacks

BLOCKERS = {
    "test_copytree_winerror": "the test replaces os.chmod with a PermissionError-raising mock",
}

COPY_CALLBACKS = {"test_copytree_custom_copy_function": 1, "test_copytree_special_func": 2}

SOURCE = '''import shutil
import sys

try:
    if sys.argv[1] == "rmtree":
        shutil.rmtree(sys.argv[2], ignore_errors=sys.argv[3] == "1")
        print("removed")
    else:
        print(shutil.copytree(sys.argv[2], sys.argv[3], symlinks=sys.argv[4] == "1",
                              ignore_dangling_symlinks=sys.argv[5] == "1", dirs_exist_ok=sys.argv[6] == "1"))
except shutil.Error as exc:
    print("shutil.Error:" + repr(exc.args[0]))
'''
ERRORS = ("FileNotFoundError", "FileExistsError", "PermissionError", "NotADirectoryError", "OSError")
for cls in ERRORS:
    SOURCE += (f'except {cls} as exc:\n    print("{cls}:" + repr(str(exc)))\n'
               '    print(repr(exc.errno))\n    print(repr(exc.filename))\n')


def executors(module, tools, work, *, with_roles=False):
    error_controls(tools, work / "errors")
    callback_controls(tools, work / "callbacks")
    filter_controls(tools, work / "filters")
    exe = tools.build(SOURCE.encode(), work)
    hosts = work / "host-copy"
    hosts.mkdir()
    bridge = tools.build(tree_host_callbacks.SOURCE.encode(), hosts)
    executions = {}
    def adapter(case, original):
        name = case._testMethodName
        if name in BLOCKERS:
            return {"status": "blocked", "reason": BLOCKERS[name]}
        counts = {"copytree": 0, "rmtree": 0}
        events = []
        source_types = []
        def run(op, *args):
            counts[op] += 1
            result = subprocess.run([exe, op, *args], check=True, capture_output=True,
                                    text=True, timeout=60)
            assert not result.stderr, result.stderr
            output = result.stdout.splitlines()
            if output[0].startswith("shutil.Error:"):
                payload = ast.literal_eval(output[0].partition(":")[2])
                assert isinstance(payload, list) and all(isinstance(e, tuple) and len(e) == 3 for e in payload)
                raise module.Error(payload)
            status, sep, value = output[0].partition(":")
            if sep and status in ERRORS:
                error = getattr(builtins, status)(ast.literal_eval(value))
                error.errno, error.filename = map(ast.literal_eval, output[1:])
                raise error
            assert len(output) == 1, output
            return output[0]
        def copytree(src, dst, symlinks=False, ignore=None, copy_function=None,
                     ignore_dangling_symlinks=False, dirs_exist_ok=False):
            if ignore is not None:
                assert copy_function is None
                counts["copytree"] += 1
                if isinstance(ignore, tree_host_callbacks.Patterns):
                    counts["ignore_patterns"] = counts.get("ignore_patterns", 0) + 1
                    return tree_host_callbacks.patterns(bridge, src, dst, ignore.patterns)
                source_types.append(type(src).__name__)
                return tree_host_callbacks.call(bridge, src, dst, ignore, events, ignore=True)
            if copy_function is not None:
                counts["copytree"] += 1
                return tree_host_callbacks.call(bridge, src, dst, copy_function, events)
            return run("copytree", src, dst, str(int(symlinks)), str(int(ignore_dangling_symlinks)),
                       str(int(dirs_exist_ok)))
        def rmtree(path, ignore_errors=False):
            assert run("rmtree", path, str(int(ignore_errors))) == "removed"
        proxy = SimpleNamespace(copytree=copytree, rmtree=rmtree, Error=module.Error,
                                ignore_patterns=tree_host_callbacks.Patterns)
        with patch.object(module, "shutil", proxy):
            original()
        assert counts["copytree"] > 0
        if name in COPY_CALLBACKS:
            assert len(events) == COPY_CALLBACKS[name] and all(types == ["str", "str"] for types in events)
        elif name == "test_copytree_arg_types_of_ignore":
            assert events == [["str", "list[str]"]] * 9 and source_types == ["str", "FakePath", "DirEntry"]
        elif name == "test_copytree_with_exclude":
            assert counts["ignore_patterns"] == 2 and events and all(e == ["str", "list[str]"] for e in events)
        detail = {"callback_argument_types": events} if events else {}
        if source_types:
            detail["host_source_types"] = source_types
        if with_roles:
            return {"roles": roles(body="cpython", assertions="cpython", operation="mixed" if events else "imre", fixture="cpython"),
                    "evidence": dict(operation="shutil.copytree", native_calls=sum(counts.values()),
                                     opt_level=tools.level, calls_by_operation=counts, **detail,
                                     fixture="original CPython TestCopyTree setup, assertions and cleanup; native traversal, copies, links and error aggregation; original host callback when present")}
        return {"status": "mixed" if events else "host_driven_native", "operations": counts, **detail}
    suite = load_suite("test.test_shutil")
    for case in cases(suite):
        if isinstance(case, module.TestCopyTree) and (not with_roles or case._testMethodName not in BLOCKERS):
            executions[case.id()] = adapter
    return executions


def execute_level(module, tools, work):
    executions = executors(module, tools, work)
    suite = load_suite("test.test_shutil")
    evidence, failures, skips = run_in_fixtures(suite, executions)
    rows = {name: value[0] for name, value in evidence.items() if value}
    rows.update({name: {"status": "failed", "reason": reason} for name, reason in failures})
    rows.update({name: {"status": "reference_skip", "reason": reason} for name, reason in skips})
    assert rows.keys() == executions.keys()
    return rows


def main(routine=execute_level):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--"+name, required=True, type=lambda x: Path(x).resolve())
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--manifest", type=Path, default=HERE / "UPSTREAM.json")
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        parser.error("use CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads(args.manifest.read_text())
    root = STDLIB / "upstream"
    verify_files(root, manifest["files"])
    verify_interpreter_tests(manifest)
    import test
    test.__path__.insert(0, str(root / "Lib/test"))
    levels = {}
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
            from test import test_shutil as module
            assert Path(module.__file__).resolve() == root / "Lib/test/test_shutil.py"
            levels[level] = routine(module, Toolchain(args.imre, args.runtime, args.clang, level), Path(tmp))
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"], "optimization_levels": levels}, indent=2) + "\n")
    return int(any(row["status"] == "failed" for rows in levels.values() for row in rows.values()))


if __name__ == "__main__":
    sys.exit(main())

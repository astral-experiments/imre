"""Try every unchanged method under original CPython fixtures; audit operation provenance separately."""

import argparse
import ast
from functools import cache
import json
import os
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_bodies import HERE, METHODS, PREAMBLE
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import (
    Toolchain,
    method_bytes,
    native_method,
    run_in_fixtures,
    testcase_fixture,
)

SOURCE = HERE / "upstream/Lib/test/test_subprocess.py"


@cache
def index():
    module = ast.parse(SOURCE.read_bytes())
    classes = {
        c.name: {f.name: f for f in c.body if isinstance(f, ast.FunctionDef)}
        for c in module.body
        if isinstance(c, ast.ClassDef)
    }
    imports = {}
    for node in module.body:
        for stmt in [node] if not isinstance(node, ast.Try) else node.body:
            if isinstance(stmt, (ast.Import, ast.ImportFrom)):
                for alias in stmt.names:
                    imports[alias.asname or alias.name] = ast.unparse(stmt)
    return classes, imports


def fixture(case, directory):
    from test import support, test_subprocess as suite

    classes, originals = index()
    method = case._testMethodName
    owner = next(
        c.__name__ for c in type(case).__mro__ if method in classes.get(c.__name__, {})
    )
    needed = set()
    names = set()
    helpers = []
    support_names = set()

    def visit(function):
        if function.name in needed:
            return
        needed.add(function.name)
        for node in ast.walk(function):
            if isinstance(node, ast.Name):
                names.add(node.id)
            if (
                isinstance(node, ast.Attribute)
                and isinstance(node.value, ast.Name)
                and node.value.id == "support"
            ):
                support_names.add(node.attr)
            if (
                isinstance(node, ast.Attribute)
                and isinstance(node.value, ast.Name)
                and node.value.id == "self"
            ):
                for c in type(case).__mro__:
                    match = classes.get(c.__name__, {}).get(node.attr)
                    if match is not None and match.name not in needed:
                        visit(match)
                        helpers.append(
                            method_bytes(SOURCE, c.__name__, match.name).decode()
                        )
                        break

    visit(classes[owner][method])
    globals_ = {
        k: getattr(suite, k)
        for k in names.intersection(
            {"ZERO_RETURN_CMD", "NONEXISTING_CMD", "SETBINARY", "mswindows"}
        )
    }
    extra = []
    if support_names and all(
        type(getattr(support, n, None)) in (str, int, bool, float)
        for n in support_names
    ):
        (directory / "support_fixture.py").write_text(
            "".join(f"{n} = {getattr(support, n)!r}\n" for n in sorted(support_names))
        )
        names.discard("support")
        extra.append("import support_fixture as support")
    if globals_:
        (directory / "command_fixture.py").write_text(
            "".join(f"{k} = {v!r}\n" for k, v in sorted(globals_.items()))
        )
        extra.append("from command_fixture import " + ", ".join(sorted(globals_)))
    if "NONEXISTING_ERRORS" in names:
        extra.append("from command_fixture import NONEXISTING_ERRORS")
        with (directory / "command_fixture.py").open("a") as out:
            classes = tuple(t.__name__ for t in suite.NONEXISTING_ERRORS)
            out.write("NONEXISTING_ERRORS = (" + ", ".join(classes) + ",)\n")
    imports = sorted({originals[n] for n in names if n in originals})
    scalar = METHODS.get(owner, {}).get(method)
    fixture = PREAMBLE.replace("TYPE", scalar) if scalar else testcase_fixture()
    return owner, "\n".join(imports + extra) + "\n" + fixture + "\n" + "\n".join(
        helpers
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for n in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + n, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument(
        "--trace-nopoll",
        action="store_true",
        help="install the original selector in native NoPoll parents and trace their system calls",
    )
    parser.add_argument(
        "--only",
        action="append",
        default=[],
        metavar="TEST_ID",
        help="select individual full IDs for diagnostics",
    )
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use the pinned CPython 3.14.5 interpreter")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    from test import support, test_subprocess as upstream

    support.use_resources = ["cpu"]
    refs = json.loads((HERE / "reference-linux.json").read_text())["reference"]
    toolchain = Toolchain(
        args.imre.resolve(),
        args.runtime.resolve(),
        args.clang.resolve(),
        args.opt_level,
    )
    rows = {}

    def attempt(case, original):
        name = case.id()
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            tc = toolchain
            trace = None
            if args.trace_nopoll and type(case).__name__ == "ProcessTestCaseNoPoll":
                from selector_fixture import ParentTrace, selected_entry, verify_binding

                verify_binding(upstream.subprocess, True)
                trace = ParentTrace(directory)

                class SelectedToolchain:
                    level = toolchain.level

                    def build(self, source, build_dir):
                        return trace.wrap(
                            toolchain.build(selected_entry(source), build_dir)
                        )

                tc = SelectedToolchain()
            try:
                owner, preamble = fixture(case, directory)
                execute = native_method(tc, directory, SOURCE, owner, name, preamble)
            except (ValueError, RuntimeError) as exc:
                last = str(exc).strip().splitlines()[-1]
                rows[name] = {
                    "status": "compile-blocked",
                    "reason": last.replace(str(directory) + "/", "").replace(
                        str(directory), "<case>"
                    ),
                }
            else:
                try:
                    result = execute(case, original)
                except (AssertionError, RuntimeError) as exc:
                    rows[name] = {"status": "failed", "reason": str(exc)}
                else:
                    # Test status alone cannot prove the operation ran at the correct site.
                    rows[name] = {
                        "status": "body-completed-needs-operation-review",
                        **result,
                    }
                    if trace is not None:
                        parents = trace.observations()
                        if any(
                            {"poll", "ppoll"} & p["native_parent_syscalls"].keys()
                            for p in parents
                        ):
                            rows[name] = {
                                "status": "failed",
                                "reason": "selected native parent used poll",
                            }
                        rows[name]["selector_parents"] = parents
        if len(rows) % 25 == 0:
            print(len(rows), "/", len(refs), flush=True)
        return rows[name]

    suite = load_suite(manifest["module"])
    ids = [c.id() for c in cases(suite)]
    if len(ids) != len(set(ids)) or set(ids) != set(refs):
        raise ValueError("incomplete denominator")
    if set(args.only) - set(ids):
        raise ValueError("unknown test IDs")
    for n in ids:
        r = refs[n]
        if args.only and n not in args.only:
            rows[n] = {"status": "not-selected"}
        elif r != "pass" and "resource 'cpu'" not in r["skipped"]:
            rows[n] = {"status": "reference-skip", "reason": r["skipped"]}
    selected = {n: attempt for n in ids if n not in rows}
    with patch.dict(os.environ, IMRE_PYTHON=sys.executable):
        _, errors, skips = run_in_fixtures(suite, selected)
    for n, reason in errors:
        rows[n] = {"status": "failed", "reason": reason.strip().splitlines()[-1]}
    for n, reason in skips:
        rows[n] = {"status": "fixture-blocked", "reason": reason}
    if set(rows) != set(ids):
        raise ValueError("missing executions")
    args.result.write_text(
        json.dumps(
            {
                "opt_level": args.opt_level,
                "denominator": len(ids),
                "rows": dict(sorted(rows.items())),
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()

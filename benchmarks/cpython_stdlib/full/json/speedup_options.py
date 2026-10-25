"""Trace the public options exercised by three original CTest methods in test_speedups."""

import argparse
import ast
import hashlib
import json
import subprocess
import sys
import tempfile
from collections import Counter
from operator import methodcaller
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from reference import BENCH, COMMIT, blob, verify
from suite_inventory import cases, load_suite
from suite_native import (
    NativeCompileError,
    Toolchain,
    method_bytes,
    native_method,
    run_in_fixtures,
)
from sweep import diagnostic, digest, prepare

MODULE = "test.test_json.test_speedups"
SOURCE = BENCH / "upstream/Lib/test/test_json/test_speedups.py"
METHODS = [
    ("TestDecode", "test_bad_bool_args"),
    ("TestEncode", "test_bad_bool_args"),
    ("TestEncode", "test_unsortable_keys"),
]
IDS = [f"{MODULE}.{owner}.{method}" for owner, method in METHODS]


def observe(factory, options, operation):
    phase = "constructor"
    try:
        instance = factory(**options)
        phase = "operation"
        return {"result": operation(instance)}
    except (ZeroDivisionError, TypeError) as exc:
        return {"phase": phase, "error": type(exc).__name__, "message": str(exc)}


def stock_options(module, bad_bool):
    return {
        "scanner_module": module.scanner.make_scanner.__module__,
        "decoder_strict": {
            text: observe(
                module.JSONDecoder, {"strict": bad_bool()}, methodcaller("decode", text)
            )
            for text in ('""', "{}", "1", '"x\x01y"')
        },
        "encoder_truth": {
            key: observe(
                module.JSONEncoder, {key: bad_bool()}, lambda e: e.encode({"a": 1})
            )
            for key in (
                "skipkeys",
                "ensure_ascii",
                "check_circular",
                "allow_nan",
                "sort_keys",
            )
        },
        "unsortable_keys": observe(
            module.JSONEncoder,
            {"sort_keys": True},
            lambda e: e.encode({"a": 1, 1: "a"}),
        ),
    }


def probe(tools, work, source):
    reference = subprocess.run(
        [sys.executable, "-I", "-B", "-c", source], capture_output=True, check=True
    )
    assert not reference.stderr
    try:
        binary = tools.build(source, work)
    except NativeCompileError as exc:
        return {
            "stock_stdout": reference.stdout.decode(),
            "outcome": "blocked",
            "diagnostic": diagnostic(exc, work),
        }
    actual = subprocess.run([binary], capture_output=True, timeout=60, check=False)
    matches = (actual.returncode, actual.stdout, actual.stderr) == (
        0,
        reference.stdout,
        b"",
    )
    return {
        "stock_stdout": reference.stdout.decode(),
        "outcome": "matched" if matches else "failed",
        "exit": actual.returncode,
        "stdout": actual.stdout.decode(),
        "stderr": actual.stderr.decode(),
    }


def operation_sources():
    raw = SOURCE.read_bytes()
    (helper,) = [
        n
        for n in ast.parse(raw).body
        if isinstance(n, ast.ClassDef) and n.name == "BadBool"
    ]
    bad = (
        b"".join(raw.splitlines(keepends=True)[helper.lineno - 1 : helper.end_lineno])
        + b"\n"
    )
    imported = b"import json\n"

    def caught(prefix, statements, error="ZeroDivisionError"):
        body = "".join(f"    {line}\n" for line in statements)
        return (
            prefix
            + f"try:\n{body}except {error} as exc:\n    print(str(exc))\n".encode()
        )

    return {
        "default_loads": imported + b'print(json.dumps(json.loads("{}")))\n',
        "original_bool": caught(bad, ["print(bool(BadBool()))"]),
        "default_decoder": imported
        + b'print(json.dumps(json.JSONDecoder().decode("{}")))\n',
        "strict_false": imported
        + b"print(json.dumps(json.JSONDecoder(strict=False).decode('\"a\\x01b\"')))\n",
        "decoder_bool": caught(
            imported + bad,
            ["json.JSONDecoder(strict=BadBool())", 'print("constructed")'],
        ),
        "encoder_bool": caught(
            imported + bad,
            [
                "enc = json.JSONEncoder(allow_nan=BadBool())",
                'print("constructed")',
                'print(enc.encode({"a": 1}))',
            ],
        ),
        "mixed_keys": caught(
            imported,
            ['print(json.JSONEncoder(sort_keys=True).encode({"a": 1, 1: "a"}))'],
            "TypeError",
        ),
    }


def original_methods(tools, root):
    suite = load_suite("test.test_json")
    selected = [case.id() for case in cases(suite) if case.id() in IDS]
    assert Counter(selected) == Counter(IDS)
    rows, executions = {}, {}
    for (owner, method), test_id in zip(METHODS, IDS, strict=True):
        work = root / test_id
        work.mkdir()
        fixture = prepare(SOURCE, owner, method)
        row = rows[test_id] = {
            "body_sha256": hashlib.sha256(
                method_bytes(SOURCE, owner, method)
            ).hexdigest(),
            "fixture_sha256": hashlib.sha256(fixture.encode()).hexdigest(),
        }
        try:
            executions[test_id] = native_method(
                tools,
                work,
                SOURCE,
                owner,
                test_id,
                fixture,
                operation=("imre", "json decoder or encoder"),
            )
        except NativeCompileError as exc:
            row.update(outcome="blocked", diagnostic=diagnostic(exc, work))
    completed, failures, skips = run_in_fixtures(suite, executions)
    for test_id in executions:
        reasons = [reason for name, reason in [*failures, *skips] if name == test_id]
        if reasons:
            rows[test_id].update(outcome="failed", diagnostics=reasons)
        else:
            assert len(completed.get(test_id, [])) == 1, test_id
            rows[test_id].update(
                outcome="completed-under-host-fixture", observations=completed[test_id]
            )
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--imre-revision", required=True)
    args = parser.parse_args()
    sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    from test.test_json import cjson, pyjson, test_speedups

    assert (
        Path(test_speedups.__file__).resolve()
        == args.cpython_source.resolve() / "Lib/test/test_json/test_speedups.py"
    )
    verify()
    inventory = list(cases(load_suite("test.test_json")))
    stock_ids = []
    for case in inventory:
        if case.id() in IDS:
            result = case.run()
            assert (
                result.wasSuccessful() and result.testsRun == 1 and not result.skipped
            ), (case.id(), result)
            stock_ids.append(case.id())
    assert Counter(stock_ids) == Counter(IDS)
    report = {
        "cpython_commit": COMMIT,
        "source_blob": blob(SOURCE),
        "suite_occurrences": len(inventory),
        "distinct_ids": len({case.id() for case in inventory}),
        "stock_ids": sorted(stock_ids),
        "stock_variants": {
            name: stock_options(module, test_speedups.BadBool)
            for name, module in [("accelerated", cjson), ("pure", pyjson)]
        },
        "imre_revision": args.imre_revision,
        "imre_sha256": digest(args.imre),
        "runtime_sha256": digest(args.runtime),
        "clang_sha256": digest(args.clang),
        "levels": {},
    }
    for level in (0, 3):
        tools = Toolchain(
            args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), level
        )
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            controls = {}
            for name, source in operation_sources().items():
                work = root / name
                work.mkdir()
                controls[name] = probe(tools, work, source)
                controls[name]["source_sha256"] = hashlib.sha256(source).hexdigest()
            report["levels"][str(level)] = {
                "original_methods": original_methods(tools, root),
                "controls": controls,
            }
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return any(
        row["outcome"] == "failed"
        for level in report["levels"].values()
        for section in level.values()
        for row in section.values()
    )


if __name__ == "__main__":
    if sys.flags.optimize:
        raise SystemExit("validation requires Python assertions")
    sys.exit(main())

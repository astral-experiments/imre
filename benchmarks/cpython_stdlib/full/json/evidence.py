"""Produce v1 evidence for every occurrence of the pinned test_json package."""

import argparse
import ast
from collections import Counter, defaultdict
import doctest
from functools import cache
import hashlib
import json
from pathlib import Path
import re
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bindings import Boundary, PRIVATE, Probe
from bodies import compiled
from deep_decode import DRIVER as DEEP_DRIVER, METHOD as DEEP_METHOD, fixture as deep_fixture
from key_identity import METHOD as KEY_METHOD, OPERATIONS as KEY_OPERATIONS, SOURCE as KEY_SOURCE
from key_identity import native_identity_control, native_key_fixture, owner_fixture
from formatting import METHODS as FORMATTING, Native as NativeFormatter, formatting_method
from native import counts
from reference import BENCH, COMMIT, HERE, routine, verify
sys.path.insert(0, str(BENCH))
from native_json_values import Native
from suite_evidence import ROLES, build_report, occurrence_keys, roles, write_reports
from suite_inventory import cases, load_suite
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures

UNOBSERVED = dict.fromkeys(ROLES, "unobserved")


@cache
def definition(source):
    module, owner, method = source.rsplit(".", 2)
    path = "Lib/" + module.replace(".", "/")
    path += "/__init__.py" if module == "test.test_json" else ".py"
    data = (BENCH / "upstream" / path).read_bytes()
    cls, = [n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == owner]
    fn, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == method]
    first = min([fn.lineno, *(d.lineno for d in fn.decorator_list)])
    method_bytes = b"".join(data.splitlines(keepends=True)[first - 1:fn.end_lineno])
    return f"{path}:{first}", hashlib.sha256(method_bytes).hexdigest()


def fixture_variant(case):
    variant = f"{type(case).__module__}.{type(case).__qualname__}"
    return f"{variant}[{case.id()}]" if isinstance(case, doctest.DocTestCase) else variant


@cache
def intact_startup():
    checks = json.loads((HERE / "doctest-startup-linux.json").read_bytes())["checks"]
    summaries = {}
    for reachable in (False, True):
        rows = [row for row in checks if row["reachable_only"] == reachable]
        reasons = {": ".join(row["diagnostic"].split(": Imre: ")[-2:]) for row in rows}
        if len(reasons) != 1 or not all(row["exit"] for row in rows):
            return "see the four entry checks in doctest-startup-linux.json"
        summaries[reachable] = reasons.pop()
    if summaries[False] == summaries[True]:
        return summaries[False]
    return f"ordinary: {summaries[False]}; --reachable-only: {summaries[True]}"


@cache
def direct_doctest(source, level):
    """Report the observed direct suite separately from the original package fixture."""
    report = json.loads((HERE / "doctest-direct-linux.json").read_bytes())
    row, = (row for row in report["levels"][str(level)].values() if source in row["ids"])
    if row["outcome"] == "blocked":
        return row["diagnostic"].splitlines()[-1]
    if row["outcome"] == "pass":
        return "direct native suite passed outside the original package fixture"
    raise ValueError(f"unexpected direct doctest result for {source}: {row}")


def missing(source, reference, variant, level):
    """Record individually verified exceptions; ordinary API failures stay in scope."""
    if isinstance(reference, dict):
        if not source.endswith(".TestCEncodeBasestringAscii.test_overflow") or set(reference) != {"skipped"}:
            raise ValueError(f"unexpected reference exception: {source}: {reference}")
        reason = reference["skipped"]
        return {"outcome": "not-run", "roles": UNOBSERVED, "reason": reason}, {
            "kind": "upstream-skip", "reason": reason, "fixture_variant": variant,
            "platform": sys.platform, "configuration": "64-bit Linux; -u cpu; no -M allocation limit",
            "condition": "test.support.bigaddrspacetest: max_memuse < MAX_Py_ssize_t",
            "source": definition(source)[0],
        }
    if reason := PRIVATE.get(".".join(source.split(".")[-2:])):
        line, digest = definition(source)
        return {"outcome": "excluded", "roles": UNOBSERVED, "reason": reason}, {
            "kind": "reviewed-exclusion", "reason": reason, "fixture_variant": variant,
            "platform": sys.platform, "configuration": f"one native Imre json implementation at -O{level}",
            "condition": reason, "source": line,
            "review": f"Inspected the complete upstream definition ({digest}): {reason}.",
            "revisit": f"Run {source} if Imre exposes the corresponding CPython backend identity or private factory.",
        }
    if source.startswith("json"):
        why = f"{direct_doctest(source, level)}; intact package: {intact_startup()} (DOCTEST.md)"
    elif ".test_tool." in source:
        why = "original subprocess invokes CPython -m json or json.tool; a native module entry point is required"
    else:
        return None, None
    return {"outcome": "not-run", "roles": UNOBSERVED, "reason": why}, None


def host_operation(program, directory, level, expected):
    def execute(case, original):
        source = routine(case)
        native = Native(program, directory, case.JSONDecodeError)
        binding = Probe(native)
        case.json, case.loads, case.dumps = binding, binding.loads, binding.dumps
        outcome, reason = "pass", None
        try:
            original()
        except Boundary as exc:
            outcome, reason = "blocked", str(exc)
        except Exception as exc:
            outcome = "failure" if isinstance(exc, AssertionError) else "error"
            reason = f"{type(exc).__name__}: {exc}"
        if outcome == "pass" and source in expected and len(native.calls) != expected[source]:
            raise ValueError(f"{source}: expected {expected[source]} calls, observed {len(native.calls)}")
        operations = Counter(call["method"] for call in native.calls)
        proof = {
            "operation": ", ".join(f"json.{op}" for op in sorted(operations)) or "no completed native JSON call",
            "fixture": "original CPython module, class and instance fixture with native JSON bindings",
            "method_sha256": definition(source)[1], "source": source, "opt_level": level,
            "native_calls": dict(sorted(operations.items())),
        }
        obs = {"outcome": outcome, "roles": roles(
            body="cpython", assertions="cpython" if outcome in ("pass", "failure") else "unknown",
            operation="imre" if operations else ("none" if outcome == "pass" else "unobserved"),
            fixture="cpython"), "evidence": proof}
        if reason:
            obs["reason"] = re.sub(r" at 0x[0-9a-fA-F]+", "", reason)
        return obs
    return execute


def run(tools, directory, reference):
    from test import support
    support.use_resources = ["cpu"]
    suite = load_suite(reference["module"])
    instances = list(cases(suite))
    discovered = [(case.id(), fixture_variant(case)) for case in instances]
    keys = list(occurrence_keys(name for name, _ in discovered))
    ids = defaultdict(list)
    for case in instances:
        ids[routine(case)].append(case.id())
    observations, exceptions, executions = {}, {}, {}
    for source, names, assertions, callback in compiled(tools, directory, ids, provenance=True):
        def native_body(case, original, callback=callback, source=source, assertions=assertions):
            obs = callback(case, original)["observation"]
            return {"outcome": "pass", "roles": obs["roles"],
                    "evidence": {**obs["evidence"], "source": source, "source_assertions": assertions}}
        executions.update(dict.fromkeys(names, native_body))
    driver = tools.build((BENCH / "json_values_driver.py").read_bytes(), directory)
    host = host_operation(driver, directory, tools.level, counts())
    deep_dir = directory / "iterative-decoder"
    deep_dir.mkdir()
    deep = deep_fixture(tools.build(DEEP_DRIVER, deep_dir), deep_dir)

    def native_depth(case, original):
        calls = deep(case, original)
        return {"outcome": "pass", "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"source": DEEP_METHOD, "method_sha256": definition(DEEP_METHOD)[1], "opt_level": tools.level,
                             "fixture": "original CPython lifecycle, recursion decorators, method and assertions",
                             "operation": "Imre json.loads; native RecursionError on each original input",
                             "native_calls": {"loads": len(calls)}, "native_exceptions": calls}}

    executions.update(dict.fromkeys(ids[DEEP_METHOD], native_depth))
    key_dir = directory / "decoder-key-owners"
    key_dir.mkdir()
    keys_method = owner_fixture(tools, key_dir)

    def native_keys(case, original):
        result = keys_method(case, original)
        obs = {"outcome": "pass" if result["outcome"] == "pass" else "failure",
               "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
               "evidence": {"source": KEY_METHOD, "method_sha256": definition(KEY_METHOD)[1], "opt_level": tools.level,
                            "fixture": "original CPython lifecycle, method, sort and identity assertions",
                            "operation": ", ".join(result["native_operations"]),
                            "native_key_owners": result["native_key_owners"]}}
        if result["reason"]:
            obs["reason"] = result["reason"]
        return obs

    executions.update(dict.fromkeys(ids[KEY_METHOD], native_keys))
    # Only choose the body observation if its actual native assertions also
    # reject different key owners and a nonempty memo. The focused report
    # separately retains the general TestCase and repr transport attempts.
    control = native_identity_control(tools, key_dir / "assertion-control")
    if control["outcome"] == "pass":
        body_dir = key_dir / "original-body"
        body_dir.mkdir()
        helper = method_bytes(KEY_SOURCE, "TestDecode", "check_keys_reuse")
        body = native_method(tools, body_dir, KEY_SOURCE, "TestDecode", ids[KEY_METHOD][0],
                             native_key_fixture(), operation=("imre", KEY_OPERATIONS))

        def native_key_body(case, original):
            try:
                result = body(case, original)["observation"]
            except AssertionError as exc:
                return {"outcome": "failure", "roles": roles(body="imre", assertions="imre", operation="imre", fixture="mixed"),
                        "reason": str(exc), "evidence": {"source": KEY_METHOD, "opt_level": tools.level,
                            "fixture": "CPython module/class/instance hooks; native instance in child", "operation": KEY_OPERATIONS}}
            return {"outcome": "pass", "roles": result["roles"], "evidence": {**result["evidence"], "source": KEY_METHOD,
                    "helper_sha256": hashlib.sha256(helper).hexdigest(), "negative_control": control["operation"]}}

        executions.update(dict.fromkeys(ids[KEY_METHOD], native_key_body))

    formatter = NativeFormatter(tools, directory)
    for short in FORMATTING:
        source = f"test.test_json.{short}"
        callback = formatting_method(formatter, short)

        def native_format(case, original, source=source, callback=callback):
            result = callback(case, original)
            return {"outcome": "pass", "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                    "evidence": {"source": source, "method_sha256": definition(source)[1], "opt_level": tools.level,
                                 "fixture": "original CPython module, class, instance, method and assertions",
                                 "operation": "Imre standard JSON formatting operations", "native_calls": result["native_calls"]}}

        for name in ids[source]:
            if name in executions:
                raise ValueError(f"duplicate observation for {name}")
            executions[name] = native_format
    for key, case, (_, variant) in zip(keys, instances, discovered, strict=True):
        if key[0] in executions:
            continue
        result = reference["reference"][key[0]]
        if isinstance(result, list):
            result = result[key[1] - 1]
        obs, exception = missing(routine(case), result, variant, tools.level)
        if obs is None:
            executions[key] = host
        else:
            observations[key] = obs
        if exception is not None:
            exceptions[*key, tools.level] = exception
    # Ordinary source methods can hit the bridge boundary mid-body. Record
    # this as a blocker and retain any successful native calls before it.
    completed, failures, skips = run_in_fixtures(suite, executions)
    if failures or skips:
        raise RuntimeError(f"unexpected fixture lifecycle result: {failures=} {skips=}")
    for name, results in completed.items():
        for occurrence, obs in enumerate(results, 1):
            key = name, occurrence
            if key in observations:
                raise ValueError(f"duplicate observation for {key}")
            observations[key] = obs
    if observations.keys() != set(keys):
        raise ValueError("missing JSON observations")
    return build_report(reference, tools.level, discovered, observations, exceptions=exceptions)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    verify()
    ref = json.loads((HERE / "reference-linux.json").read_text())
    if ref["cpython_commit"] != COMMIT or ref["platform"] != sys.platform:
        raise ValueError("run the pinned reference on the matching platform")
    reports = []
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as directory:
            report = run(Toolchain(args.imre, args.runtime, args.clang, level), Path(directory), ref)
        print(f"-O{level}: {report['denominator']} occurrences, {report['counts']}", flush=True)
        reports.append(report)
    write_reports(args.result, reports, ref)


if __name__ == "__main__":
    main()

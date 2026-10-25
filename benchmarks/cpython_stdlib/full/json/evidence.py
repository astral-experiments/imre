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
from native import counts
from reference import BENCH, COMMIT, HERE, routine, verify
sys.path.insert(0, str(BENCH))
from native_json_values import Native
from suite_evidence import ROLES, build_report, occurrence_keys, roles, write_reports
from suite_inventory import cases, load_suite
from suite_native import Toolchain, run_in_fixtures

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
        why = "DocTestSuite imports CPython json; native example execution requires the shared doctest fixture (DOCTEST.md)"
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

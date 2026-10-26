"""Measure HTTPStatus and server methods and retain all original HTTP suite occurrences."""
import ast
from dataclasses import replace
from hashlib import file_digest, sha256
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify import HERE, manifests, options, toolchains
from hosted import hosted
import server_cases
from suite_evidence import ROLES, build_report, occurrence_keys, reference_cases, roles, write_reports
from suite_inventory import cases, load_suite, verify_interpreter_tests
from suite_native import NativeCompileError, NativeDefaultMethod, method_bytes

sys.path.insert(0, str(HERE.parent))
from stage_tests import stage

STATUS = {
    "test_httpstatus_range": "HTTPStatus integer range comparisons",
    "test_httpstatus_category": "HTTPStatus range comparisons and five category properties",
    "test_dir_with_added_behavior_on_status": "dir on an HTTPStatus member",
    "test_simple_httpstatus": "enum._test_simple_enum applied to HTTPStatus",
}
HOSTED = {"test_httpstatus_range", "test_httpstatus_category"}


def sha(path):
    with path.open("rb") as stream:
        return file_digest(stream, "sha256").hexdigest()


def startup(tool, module, lib):
    entry = (f"import {module} as target\nimport unittest\n"
             "unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromModule(target))\n")
    try:
        replace(tool, module_root=lib).build(entry.encode(), lib)
    except NativeCompileError as exc:
        reason = str(exc).replace(str(lib), "Lib").strip()
        if "Imre:" not in reason and "error[" not in reason:
            raise
        return reason
    raise AssertionError(f"{module} built successfully; run its original suite and classify the results")


def skip_exception(case, ref, path, platform, variant):
    resource, = [name for name in ("network", "walltime", "cpu") if f"'{name}'" in ref["skipped"]]
    condition = "support.requires('network')" if resource == "network" else f"support.requires_resource('{resource}')"
    # Locate the resource check in this method, including its decorators.
    tree = ast.parse((HERE / "upstream" / path).read_bytes())
    cls, = [n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == type(case).__name__]
    method, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == case._testMethodName]
    line, = [n.lineno for n in ast.walk(method) if isinstance(n, ast.Call) and ast.unparse(n) == condition]
    return {"kind": "upstream-skip", "condition": condition, "reason": ref["skipped"],
            "source": f"{path}:{line}", "platform": platform, "fixture_variant": variant,
            "configuration": "ordinary CPython 3.14.5, default regrtest resources"}


def measure(args, work, output):
    staged = work / "staged"
    stage(staged)
    receipt = {"compiler_sha256": sha(args.compiler), "runtime_sha256": sha(args.runtime),
               "worker_sha256": sha(HERE / "status_worker.py"), "suites": {}}
    for suite, manifest in manifests():
        verify_interpreter_tests(manifest)
        module = manifest["module"]
        path = f"Lib/test/test_{suite}.py"
        source = HERE / "upstream" / path
        reference = json.loads((HERE / f"reference-{suite}-linux.json").read_bytes())
        if reference["module"] != module or reference["cpython_commit"] != manifest["commit"]:
            raise ValueError(f"{suite}: reference and manifest differ")
        discovered = list(cases(load_suite(module)))
        variants = [(case.id(), f"{type(case).__module__}.{type(case).__qualname__}") for case in discovered]
        keys = list(occurrence_keys(case.id() for case in discovered))
        refs = reference_cases(reference)
        runs = receipt["suites"][suite] = []
        pair = []
        selected = {case._testMethodName: case for case in discovered
                    if suite == "httplib" and type(case).__name__ == "BasicTest" and case._testMethodName in STATUS}
        if suite == "httplib" and selected.keys() != STATUS.keys():
            raise ValueError("did not discover all four original BasicTest HTTPStatus methods")
        if suite == "httpservers":
            server_cases.reference(work)
        for tool in toolchains(args):
            attempt = {"opt_level": tool.level, "reachable_only": tool.reachable_only,
                       "suite_startup": startup(tool, module, staged / "Lib"), "methods": {}, "hosted": {}}
            if suite == "httpservers":
                attempt["server"] = server_cases.measure(tool, work)
            for name, case in selected.items():
                runner = NativeDefaultMethod(tool, work, source, manifest["files"][path], ("imre", STATUS[name]))
                attempt["methods"][name] = runner.run(case)
                if name in HOSTED:
                    attempt["hosted"][name] = hosted(tool, work, type(case)(name))
            runs.append(attempt)
            if not tool.reachable_only:
                continue
            ordinary, reachable = runs[-2:]
            observations, exceptions = {}, {}
            for case, key, (_, variant) in zip(discovered, keys, variants, strict=True):
                if isinstance(refs[key], dict) and "skipped" in refs[key]:
                    exceptions[(*key, tool.level)] = skip_exception(case, refs[key], path, reference["platform"], variant)
                    continue
                name = case._testMethodName
                if suite == "httpservers" and (server := server_cases.observation(case, ordinary["server"], reachable["server"])):
                    observations[key] = server
                elif name in selected and case.id() == selected[name].id():
                    candidates = [r["methods"][name] for r in (ordinary, reachable)]
                    if all(obs["outcome"] == "pass" for obs in candidates):
                        observations[key] = candidates[1]
                    elif any(obs["outcome"] != "blocked" for obs in candidates):
                        raise AssertionError(f"review divergent original-method outcomes: {candidates}")
                    elif name in HOSTED:
                        if ordinary["hosted"][name] != reachable["hosted"][name]:
                            raise AssertionError(f"hosted operation counts differ: {name}")
                        observations[key] = {"outcome": "pass", "roles": roles(
                            body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                            "evidence": {"operation": STATUS[name], "fixture": "CPython TestCase and HTTPStatus registry; native pipe worker",
                                         "method_sha256": sha256(method_bytes(source, "BasicTest", name)).hexdigest(),
                                         "artifact": "measurements-linux.json"}}
                    else:
                        observations[key] = candidates[1]
                else:
                    observations[key] = {"outcome": "blocked", "roles": dict.fromkeys(ROLES, "unobserved"),
                        "reason": reachable["suite_startup"].splitlines()[-1],
                        "evidence": {"stage": "suite startup", "artifact": "measurements-linux.json"}}
            pair.append(build_report(reference, tool.level, variants, observations, exceptions=exceptions))
        write_reports(output / f"evidence-{suite}-linux.json", pair, reference)
        print(f"{module}: {reference['denominator']} occurrences; O0 {pair[0]['counts']}, O3 {pair[1]['counts']}", flush=True)
    (output / "measurements-linux.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    args = options(__doc__)
    if sys.platform != "linux" or args.output is None:
        raise SystemExit("use Linux and supply an --output directory")
    args.output.mkdir(exist_ok=True, parents=True)
    with tempfile.TemporaryDirectory() as tmp:
        measure(args, Path(tmp), args.output)

"""Run isolated argparse cases with their original fixtures."""
import argparse
import ast
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from check_registration import HERE, stage_module, target_ids

sys.path.insert(0, str(HERE.parents[1]))
from native_methods import OPERATIONS
from suite_native import method_bytes
from suite_evidence import build_report, roles, write_reports
from suite_inventory import cases, load_suite, verify_files


ENTRY = '''import json
import os
import sys
import unittest
import test.test_argparse as target

class RecordingRunner(unittest._MainRunner):
    def _subtest_result(self, description, exc, failure):
        if isinstance(exc, unittest.SkipTest):
            self.skip_reasons.append(description + ": " + str(exc))
        super()._subtest_result(description, exc, failure)

    def _mark(self, short, detail):
        if short == "s":
            self.skip_reasons.append(detail)

def run_case(cls, method, width, expected):
    case = cls()
    case._testMethodName = method
    name = case.id()
    runner = RecordingRunner()
    runner.verbose = 0
    runner.skip_reasons = []
    subtest = case.subTest
    entered = 0
    def observe_subtest(*args, **kwargs):
        nonlocal entered
        entered += 1
        return subtest(*args, **kwargs)
    case.subTest = observe_subtest
    def setup():
        case.setUp()
        assert os.environ.get("COLUMNS") == "80"
    try:
        runner._run(case, name, setup, getattr(case, method), case.tearDown)
    finally:
        case._runner = runner
        try:
            case.doCleanups()
        finally:
            case._runner = None
    assert runner.total == 1 and os.environ.get("COLUMNS") == width
    assert runner.skips == len(runner.skip_reasons)
    if not (runner.failures or runner.errors or runner.skips):
        assert entered == expected, f"{name} entered {entered} of {expected} subtests"
    print(json.dumps([name, runner.failures, runner.errors, runner.skip_reasons, entered]), flush=True)
    return bool(runner.failures or runner.errors)

def main():
    with open(sys.argv[1], "rb") as file:
        selected = json.loads(file.read().decode())
    failed = False
    width = os.environ.get("COLUMNS")
    try:
        for cls_name in dir(target):
            cls = getattr(target, cls_name)
            if not isinstance(cls, type) or not issubclass(cls, target.TestCase):
                continue
            prefix = ".".join([cls.__module__, cls.__qualname__, ""])
            methods = [m for m in dir(cls) if prefix + m in selected]
            if methods:
                cls.setUpClass()
                try:
                    for method in sorted(methods):
                        failed = run_case(cls, method, width, selected[prefix + method]) or failed
                finally:
                    cls.tearDownClass()
    finally:
        target.tearDownModule()
    assert target.RFile.seen == {} and target.WFile.seen == set()
    print(json.dumps(["module-teardown", width]), flush=True)
    if failed:
        sys.exit(1)

main()
'''


def generated_table(module, case):
    """Resolve the generated wrapper and its bound AddTests method on the pinned reference."""
    cls = type(case)
    if type(cls) is not module.ParserTesterMetaclass:
        return None
    name = case._testMethodName
    wrapper = cls.__dict__.get(name)
    sample = module.TestOptionalsSingleDash
    success = sample.test_successes_no_groups_listargs
    if getattr(wrapper, "__code__", None) is not success.__code__:
        return None
    bound, = wrapper.__defaults__
    for exemplar, table in ((success, "successes"), (sample.test_failures_no_groups_listargs, "failures")):
        if bound.__func__.__code__ is exemplar.__defaults__[0].__func__.__code__:
            owner = bound.__self__
            if name != "_".join((bound.__name__, owner._add_arguments.__name__, owner._parse_args.__name__)):
                raise ValueError("unexpected registration on " + case.id())
            return getattr(cls, table)
    raise ValueError("unexpected wrapper body on " + case.id())


def reference_inputs(names, *, additional=False, source_methods=False, all_generated=False):
    from test import test_argparse as module
    from test.support import os_helper

    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    shared = HERE.parent / "shared"
    for mod, root, key in ((module, HERE, "Lib/test/test_argparse.py"),
                           (os_helper, shared, "Lib/test/support/os_helper.py")):
        manifest = json.loads((root / "UPSTREAM.json").read_bytes())
        verify_files(root / "upstream", {key: manifest["files"][key]})
        if Path(mod.__file__).read_bytes() != (root / "upstream" / key).read_bytes():
            raise ValueError("reference module differs from " + key)
    if hasattr(module, "setUpModule") or not hasattr(module, "tearDownModule"):
        raise ValueError("module fixtures changed")
    all_cases = list(cases(load_suite(reference["module"])))
    selected = {}
    empty = set()
    class_declarations = {n.name: n for n in ast.parse(Path(module.__file__).read_bytes()).body
                          if isinstance(n, ast.ClassDef)}
    for case in all_cases:
        if not all_generated and (case.id() in names) == additional:
            continue
        cls = type(case)
        method = getattr(cls, case._testMethodName)
        case_bases = (module.TestCase, unittest.TestCase, object)
        expected_bases = (module.ParserTestCase, *case_bases)
        if source_methods:
            if (cls.__dict__.get(case._testMethodName) is not method
                    or method.__qualname__ != f"{cls.__qualname__}.{case._testMethodName}"
                    or Path(method.__code__.co_filename).resolve() != Path(module.__file__).resolve()):
                raise ValueError("expected original source-defined method on " + case.id())
            if cls is module.TestParseKnownArgs:
                expected_bases = case_bases
        else:
            table = generated_table(module, case)
            if table is None:
                if additional or (all_generated and case.id() not in names):
                    continue
                raise ValueError("expected original parser wrapper on " + case.id())
        if (class_declarations[cls.__name__].decorator_list
                or cls.__mro__ != (cls, *expected_bases)
                or cls.setUp is not module.TestCase.setUp
                or cls.tearDown is not unittest.TestCase.tearDown
                or cls.setUpClass.__func__ is not unittest.TestCase.setUpClass.__func__
                or cls.tearDownClass.__func__ is not unittest.TestCase.tearDownClass.__func__
                or getattr(cls, "__unittest_skip__", False) or getattr(method, "__unittest_skip__", False)
                or getattr(method, "__unittest_expecting_failure__", False)):
            if additional or (all_generated and case.id() not in names):
                continue
            raise ValueError("unaccounted fixture or outcome on " + case.id())
        if case.id() in selected:
            raise ValueError("duplicate selected occurrence: " + case.id())
        selected[case.id()] = 0 if source_methods else len(table)
        if not source_methods and not table:
            empty.add(case.id())
    if all_generated and (not names <= selected.keys() or sum(selected[n] for n in names) != 3606):
        raise ValueError("priority generated inputs changed")
    if additional or all_generated:
        if (len(selected) - (len(names) if all_generated else 0), len(empty)) != (258, 30):
            raise ValueError("additional generated case or empty-table inventory changed")
    elif selected.keys() != names or empty or (not source_methods and sum(selected.values()) != 3606):
        raise ValueError("prioritized case multiplicity or input tables changed")
    inventory = [(case.id(), f"{type(case).__module__}.{type(case).__qualname__}") for case in all_cases]
    return reference, inventory, selected, empty


def digest(path):
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def measure(level, work, compiler, runtime, clang, ordered, inventory, reference, result, empty, method_hashes):
    binary = work / f"suite-O{level}"
    cmd = [compiler, "build", work / "case.py", "--runtime-imports", "--opt-level", str(level),
           "--runtime", runtime, "--clang", clang, "-o", binary]
    subprocess.run(cmd, check=True)
    log = result.with_suffix(f".O{level}.jsonl")
    environment = {key: value for key, value in os.environ.items() if key != "COLUMNS"}
    with log.open("w") as output:
        finished = subprocess.run([binary, work / "ids"], env=environment, stdout=output,
                                  stderr=subprocess.PIPE, timeout=3600)
    rows = [json.loads(line) for line in log.read_text().splitlines()]
    if finished.stderr or not rows or rows.pop() != ["module-teardown", None] or [r[0] for r in rows] != list(ordered):
        raise RuntimeError(f"O{level} incomplete measurements; inspect {log} and {finished.stderr[-1500:]!r}")
    observations = {}
    for name, failures, errors, skips, entered in rows:
        outcome = "error" if errors else "failure" if failures else "skip" if skips else "pass"
        row = dict(outcome=outcome, roles=roles(body="imre", assertions="imre", operation="imre", fixture="imre"),
                   evidence=dict(operation="original generated parse_args body, subtests and assertions",
                                 fixture="native setUpClass, COLUMNS setUp, tearDown, cleanups, tearDownClass and tearDownModule"))
        row["evidence"].update(subtests_entered=entered, subtests_expected=ordered[name])
        if outcome == "pass" and entered != ordered[name]:
            raise ValueError("incomplete native subtest loop on " + name)
        if name in method_hashes:
            operation, assertion_role = OPERATIONS[name]
            row["roles"]["assertions"] = assertion_role
            row["evidence"].update(operation=operation, method_sha256=method_hashes[name])
        if name in empty:
            row["roles"].update(operation="none", assertions="none")
            row["evidence"]["operation"] = "empty original input table; parser constructed, no parse or assertion"
        if outcome != "pass":
            row["reason"] = json.dumps([failures, errors, skips])
        observations[name, 1] = row
    counts = Counter(row["outcome"] for row in observations.values())
    if finished.returncode != int(bool(counts["failure"] or counts["error"])):
        raise RuntimeError(f"O{level} status disagrees with test outcomes: {finished.returncode}")
    report = build_report(reference, level, inventory, observations)
    report["measurement"] = dict(subset=("source-defined methods with original fixtures" if method_hashes
                                       else "generated parser cases with original fixtures"),
                                 compiler_sha256=digest(compiler), runtime_sha256=digest(runtime),
                                 binary_sha256=digest(binary), log_sha256=digest(log),
                                 entry_sha256=digest(work / "case.py"),
                                 test_extract_sha256=digest(work / "test/test_argparse.py"),
                                 guard_extract_sha256=digest(work / "test/support/os_helper.py"),
                                 native_exit=finished.returncode)
    print(f"-O{level}: {counts['pass']}/{len(rows)} isolated cases passed", flush=True)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--all-applicable-generated", action="store_true",
                           help="run both the priority and additional generated table cases")
    selection.add_argument("--source-methods", action="store_true",
                           help="run the five source-defined methods with previously host-executed fixtures")
    selection.add_argument("--additional-generated", action="store_true",
                           help="run the other generated parser wrappers with the same fixture lineage")
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or sys.platform != "linux":
        parser.error("use a test-enabled CPython 3.14.5 on Linux")
    reference, inventory, ordered, empty = reference_inputs(
        set(OPERATIONS) if args.source_methods else target_ids(),
        additional=args.additional_generated, source_methods=args.source_methods,
        all_generated=args.all_applicable_generated)
    with tempfile.TemporaryDirectory(prefix="imre-argparse-fixtures-") as temp:
        work = Path(temp)
        stage_module(work, {name.split(".")[-2] for name in ordered}, fixtures=True)
        method_hashes = {}
        if args.source_methods:
            for name in ordered:
                cls, method = name.rsplit(".", 2)[1:]
                source = method_bytes(HERE / "upstream/Lib/test/test_argparse.py", cls, method)
                if method_bytes(work / "test/test_argparse.py", cls, method) != source:
                    raise ValueError("staged body differs from " + name)
                method_hashes[name] = hashlib.sha256(source).hexdigest()
        (work / "case.py").write_text(ENTRY)
        (work / "ids").write_text(json.dumps(ordered))
        compiler = Path(shutil.copy2(args.imre, work / "imre"))
        runtime = Path(shutil.copy2(args.runtime, work / "libimre_runtime.a"))
        with ThreadPoolExecutor(max_workers=2) as pool:
            jobs = [pool.submit(measure, level, work, compiler, runtime, args.clang, ordered,
                                inventory, reference, args.result, empty, method_hashes) for level in (0, 3)]
            reports = [job.result() for job in jobs]
    write_reports(args.result, reports, reference)
    return int(any((report["counts"].get("fully-native", 0) != len(ordered) - len(empty)
                    or report["counts"].get("no-product-operation", 0) != len(empty)) for report in reports))


if __name__ == "__main__":
    sys.exit(main())

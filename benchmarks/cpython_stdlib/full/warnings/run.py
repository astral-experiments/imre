"""Verify native warning filters and account for the entire pinned test_warnings suite."""
import argparse
import ast
from collections import Counter
from contextlib import ExitStack
from functools import partial
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import textwrap
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1]))
import display
from suite_evidence import roles, write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, method_bytes, run_evidence, run_in_fixtures

MODULE = "test.test_warnings"
PACKAGE = "Lib/test/test_warnings/__init__.py"
METHOD = "test_argument_validation"


def validation(tools, directory):
    # Compile the actual filter callsites in the pinned method. The unchanged
    # method decides which call is made and which exception class it expects.
    source = method_bytes(HERE / "upstream" / PACKAGE, "FilterTests", METHOD)
    sites = {}
    lines = ["import sys", "import warnings", "def check(site: str) -> None:", "    try:"]
    for node in ast.walk(ast.parse(textwrap.dedent(source.decode()))):
        if not isinstance(node, ast.Call) or ast.unparse(node.func) not in {
            "self.module.filterwarnings", "self.module.simplefilter"
        }:
            continue
        def value(node):
            return int if isinstance(node, ast.Name) and node.id == "int" else ast.literal_eval(node)
        name = node.func.attr
        key = (name, tuple(map(value, node.args)), tuple((kw.arg, value(kw.value)) for kw in node.keywords))
        assert key not in sites
        index = str(len(sites))
        prefix = "if" if not sites else "elif"
        sites[key] = index
        node.func.value = ast.Name(id="warnings", ctx=ast.Load())
        lines.extend([f"        {prefix} site == {index!r}:", f"            {ast.unparse(node)}"])
    assert len(sites) == 10
    lines += ["        else:", "            raise AssertionError('unknown native call')",
              "    except TypeError as error:", "        print('TypeError')", "        print(str(error))",
              "    except ValueError as error:", "        print('ValueError')", "        print(str(error))",
              "check(sys.argv[1])"]
    exe = tools.build(("\n".join(lines) + "\n").encode(), directory)

    def exercise(case, original):
        calls = Counter()
        def invoke(name, reference, *args, **kwargs):
            site = sites[name, args, tuple(kwargs.items())]
            calls[name] += 1
            proc = subprocess.run([exe, site], text=True, capture_output=True, check=True, timeout=15)
            if proc.stderr:
                raise AssertionError(proc.stderr)
            # Compare the complete message too: the upstream method only
            # requires the class. Return precisely what the native call did.
            native = proc.stdout.splitlines()
            try:
                reference(*args, **kwargs)
            except (ValueError, TypeError) as error:
                expected = [type(error).__name__, str(error)]
            else:
                expected = []
            if native != expected:
                raise AssertionError((name, args, kwargs, native, expected))
            if native:
                raise {"ValueError": ValueError, "TypeError": TypeError}[native[0]](native[1])

        # The original C/Python variant's fixture remains in place. Replace
        # only its two product functions for the duration of the original body.
        with ExitStack() as stack:
            for name in ("filterwarnings", "simplefilter"):
                original_function = getattr(case.module, name)
                stack.enter_context(patch.object(case.module, name, partial(invoke, name, original_function)))
            original()
        assert calls == {"filterwarnings": 7, "simplefilter": 3}, calls
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"operation": "warnings.filterwarnings and warnings.simplefilter",
                             "native_calls": sum(calls.values()), "opt_level": tools.level,
                             "method_sha256": hashlib.sha256(source).hexdigest(),
                             "fixture": "Original C or Python warning module and CPython unittest lifecycle; calls are routed to the compiled native operations.",
                             "assertions": "Unchanged CPython assertRaises checks each native exception; its complete class and message also match the original warning function."}}
    return exercise


def child_filters(tools, directory):
    source = HERE / "filters.py"
    exe = tools.build(source.read_bytes(), directory)
    outputs = []
    for args in ([sys.executable, "-I", source], [exe]):
        proc = subprocess.run(args, text=True, capture_output=True, check=True, timeout=30)
        pids, stdout = {}, []
        for line in proc.stdout.splitlines():
            match = re.fullmatch(r"([a-z-]+) (\d+)", line)
            if match:
                assert match[2] not in pids
                pids[match[2]] = match[1]
                stdout.append(match[1])
            else:
                stdout.append(line)
        warnings = [pids[pid] for pid in re.findall(r"subprocess\.py:1139: ResourceWarning: subprocess (\d+) is still running", proc.stderr)]
        assert proc.stderr.count("ResourceWarning: subprocess") == len(warnings)
        assert proc.stderr.count("OverflowError: Python int too large to convert to C ssize_t") == 1
        assert not re.search(r"(?<!Overflow)Error:", proc.stderr), proc.stderr
        outputs.append((stdout, warnings))
    assert outputs[0] == outputs[1], outputs
    assert outputs[0][1] == ["default-after-reset", "matches", "message-must-start",
                              "module-is-case-sensitive", "other-category", "prepend-duplicate", "reset-again"]


def rejected_patterns(tools, directory):
    for expression, assignment, diagnostic in (
        ("pattern", "pattern = 'subprocess'\n", "requires a fixed string pattern"),
        ("'.*İs'", "", "require ASCII literals"),
        ("'(?=subprocess)'", "", "unsupported warning filter regex"),
    ):
        source = f"import warnings\n{assignment}warnings.filterwarnings('ignore', message={expression})\nprint('ok')\n"
        reference = subprocess.run([sys.executable, "-I", "-c", source], capture_output=True, text=True, check=True)
        assert reference.stdout == "ok\n" and reference.stderr == ""
        try:
            tools.build(source.encode(), directory)
        except NativeCompileError as error:
            if diagnostic not in str(error):
                raise
        else:
            raise AssertionError(f"accepted unsupported message regex: {expression}")


def skips(level):
    entries = {}
    for implementation in ("C", "Py"):
        for cls, method, line, condition, reason in (
            ("AsyncTests", "test_async_context", 1665, "not sys.flags.context_aware_warnings", "requires context aware warnings"),
            ("AsyncTests", "test_async_task_inherit", 1705, "not sys.flags.context_aware_warnings", "requires context aware warnings"),
            ("ThreadTests", "test_threaded_context", 1761,
             "not (sys.flags.context_aware_warnings and sys.flags.thread_inherit_context)", "requires thread-safe warnings flags"),
        ):
            fixture = f"{MODULE}.{implementation}{cls}"
            entries[(f"{fixture}.{method}", 1, level)] = {
                "kind": "upstream-skip", "fixture_variant": fixture, "platform": "linux",
                "configuration": "CPython 3.14.5 Linux release build with default warning/thread flags",
                "source": f"{PACKAGE}:{line}", "condition": condition, "reason": reason,
            }
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("run with a test-enabled CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    reports = []
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            child_filters(tools, work)
            rejected_patterns(tools, work)
            exercise = validation(tools, work)
            observations = {f"{MODULE}.{implementation}FilterTests.{METHOD}": exercise for implementation in ("C", "Py")}
            display.operation_cases(tools, work)
            worker = work / "display-worker"
            worker.mkdir()
            exe = tools.build((HERE / "display_worker.py").read_bytes(), worker)
            for method in display.METHODS:
                for variant in ("C", "Py"):
                    name = f"{MODULE}.{variant}WarningsDisplayTests.{method}"
                    observations[name] = display.observe(exe, method, level)
                name = f"{MODULE}.CWarningsDisplayTests.{method}"
                _, failures, skipped = run_in_fixtures(
                    load_suite(MODULE), {name: display.observe(exe, method, level, fault=True)})
                if skipped or "incorrect native result" not in str(failures):
                    raise AssertionError(("the original assertion accepted a corrupt result", method, failures))
            report = run_evidence(load_suite(MODULE), observations, reference, level, exceptions=skips(level))
            for record in report["records"]:
                if record["id"] in observations and record["outcome"] != "pass":
                    raise AssertionError(record)
            print(f"-O{level}: {report['counts']}")
            reports.append(report)
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()

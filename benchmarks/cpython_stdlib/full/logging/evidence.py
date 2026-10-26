"""Run unchanged logging methods and write per-occurrence evidence at O0 and O3."""

import argparse
import ast
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
sys.path[:0] = [str(HERE), str(STDLIB)]
import native_config
import native_encoding
import native_filters
import native_files
import native_formatters
import native_handlers
import native_hierarchy
import native_levels
import native_logging
import native_make_record
import native_records
import native_rotation
import native_timed
from suite_evidence import roles, write_reports
from suite_inventory import cases, load_suite, verify_files
from suite_native import (NativeCompileError, Toolchain, compile_blocker, method_bytes,
                          native_method, run_evidence, testcase_fixture)

PREFIX = "test.test_logging."


def observation(body, assertions, fixture, operation, how, level, **evidence):
    return {"roles": roles(body=body, assertions=assertions, operation="imre", fixture=fixture),
            "evidence": {"operation": operation, "fixture": how, "opt_level": level, **evidence}}


def reference_skips(reference, level):
    debug = ("not support.Py_DEBUG", "CPython release build")
    conditions = {
        "ConfigDictTest.test_config_reject_simple_queue_handler_multiprocessing_context": (*debug, 4051),
        "ConfigDictTest.test_config_queue_handler_multiprocessing_context": (*debug, 4073),
        "NTEventLogHandlerTest.test_basic": ("not win32evtlog", "Linux without Win32 event libraries", 7225),
    }
    result = {}
    for short, (condition, configuration, line) in conditions.items():
        case_id = PREFIX + short
        reason = reference["reference"][case_id]["skipped"]
        result[case_id, 1, level] = {"kind": "upstream-skip", "condition": condition, "reason": reason,
            "configuration": configuration, "source": f"Lib/test/test_logging.py:{line}", "platform": reference["platform"],
            "fixture_variant": case_id.rsplit(".", 1)[0]}
    return result


def output_case(toolchain, directory, case_id, original):
    owner, name = case_id.removeprefix(PREFIX).split(".")
    body = method_bytes(SOURCE, owner, name)
    if owner == "CustomLevelsAndFiltersTest":
        constants = f"VERBOSE = {original.VERBOSE}\nLEVEL_RANGE = {list(original.LEVEL_RANGE)!r}\n"
        adapter = native_levels.CUSTOM_CASE.replace("class Case:", constants + "\nclass Case:")
        helper = method_bytes(SOURCE, owner, "log_at_all_levels")
        setup = "".join(f"logging.addLevelName({key}, {value!r})\n"
                        for key, value in original.my_logging_levels.items())
        setup += f"logging.basicConfig(format={original.BaseTest.log_format!r})\n"
    else:
        adapter, helper = native_logging.FIXTURE, b""
        setup = f"logging.basicConfig(level=logging.DEBUG, format={original.BaseTest.log_format!r})\n"
    binary = toolchain.build(adapter.encode() + helper + b"\n" + body + b"\n" +
                             f"{setup}Case().{name}()\n".encode(), directory)

    def execute(case, method):
        child = subprocess.run([binary], capture_output=True, text=True, check=True, timeout=30)
        expected = [tuple(row.split("\t")) for row in child.stdout.splitlines()]
        if not expected:
            raise AssertionError("missing expected log rows from the original method")
        stream = io.StringIO()
        stream.write(child.stderr)
        case.assert_log_lines(expected, stream)
        return observation("imre", "cpython", "mixed", "Native logger level selection and emitted records",
                           "Original CPython hooks and assert_log_lines; native child holds logger state",
                           toolchain.level, method_sha256=hashlib.sha256(body).hexdigest(), rows=len(expected))
    return execute


def known_cases(toolchain, work, original):
    def directory(short):
        d = work / short
        d.mkdir()
        return d

    executions = {}
    for short, adapter, operation in (
        ("LoggerTest.test_root_logger_aliases", native_logging.FIXTURE, "Native logger identity across root aliases"),
        ("ModuleLevelMiscTest.test_get_level_names_mapping", native_levels.MAP_CASE,
         "Native registry snapshots; native content and independent-mutation assertions"),
    ):
        owner = short.split(".")[0]
        executions[PREFIX + short] = native_method(toolchain, directory(short), SOURCE, owner, PREFIX + short,
            adapter.replace("class Case:", "class NativeCase:"), operation=("imre", operation))
    for short in [*(f"BuiltinLevelsTest.{n}" for n in native_logging.METHODS["BuiltinLevelsTest"]),
                  "CustomLevelsAndFiltersTest.test_logger_filter"]:
        executions[PREFIX + short] = output_case(toolchain, directory(short), PREFIX + short, original)

    level_binary = toolchain.build((HERE / "level_driver.py").read_bytes(), directory("levels"))
    def drive_levels(case, method):
        stack = case.enterContext(ExitStack())
        driver = stack.enter_context(native_levels.registry(level_binary))
        operations = []
        invoke = driver.invoke
        def observed(operation, *args):
            operations.append(operation)
            return invoke(operation, *args)
        driver.invoke = observed
        with patch.object(original, "logging", driver):
            method()
        if not operations:
            raise AssertionError("no native level operations")
        return observation("cpython", "cpython", "cpython", "Native persistent level registry",
                           "Original CPython hooks, assertions and registered cleanups; fresh native process",
                           toolchain.level, calls=operations)
    for name in ("test_regression_22386", "test_issue27935", "test_regression_29220"):
        executions[PREFIX + "BuiltinLevelsTest." + name] = drive_levels

    timed_binary = toolchain.build((HERE / "timed_driver.py").read_bytes(), directory("timed"))
    def drive_timed(case, method):
        with ExitStack() as stack:
            driver = native_timed.Driver(timed_binary, stack)
            shim = SimpleNamespace(handlers=SimpleNamespace(TimedRotatingFileHandler=driver.new))
            with patch.object(original, "logging", shim):
                method()
            if not driver.operations:
                raise AssertionError("no native timed file operations")
            return observation("cpython", "cpython", "cpython", "Native timed file handler",
                               "Original CPython hooks, filenames, clock and assertions",
                               toolchain.level, native_calls=driver.operations)
    owner = "TimedRotatingFileHandlerTest"
    names = [n for n in dir(getattr(original, owner)) if n.startswith("test_compute_rollover_")]
    names += ["test_invalid", "test_compute_files_to_delete_same_filename_different_extensions"]
    for name in names:
        executions[PREFIX + owner + "." + name] = drive_timed
    daily = PREFIX + owner + ".test_compute_rollover_daily_attime"
    def native_daily(case, method):
        fixture = native_timed.DAILY_FIXTURE.replace("FILENAME", repr(case.fn))
        return native_method(toolchain, directory("native-daily"), SOURCE, owner, daily, fixture,
            operation=("imre", "Native handler construction, calendar rollover and close"))(case, method)
    executions[daily] = native_daily
    executions.update(native_config.config_cases(toolchain, directory("config"), original))
    executions.update(native_handlers.handler_cases(toolchain, directory("handlers"), original))
    executions.update(native_make_record.record_cases(toolchain, directory("make-record")))
    executions[native_encoding.CASE] = native_encoding.encoding_case(toolchain, directory("encoding"))
    executions[native_encoding.SETTING] = native_encoding.stream_setting_case(toolchain, directory("stream-setting"))
    executions.update(native_filters.filter_cases(toolchain, directory("name-filter")))
    executions.update(native_hierarchy.hierarchy_cases(toolchain, directory("children")))
    executions.update(native_files.file_cases(toolchain, directory("file-handlers")))
    executions.update(native_rotation.rotation_cases(toolchain, directory("size-rotation")))
    formatter = toolchain.build((HERE / "formatter_driver.py").read_bytes(), directory("formatters"))
    executions.update(native_formatters.host_formatter_cases(toolchain, formatter, original))
    executions[native_formatters.PRECISION] = native_formatters.precision_case(toolchain, directory("precision"))
    executions.update(native_formatters.buffering_cases(toolchain, directory("buffering")))
    record_binary = toolchain.build((HERE / "record_driver.py").read_bytes(), directory("records"))
    def optional(case, method):
        with native_records.native_flags(record_binary) as (driver, calls), patch.object(original, "logging", driver):
            method()
        if not calls:
            raise AssertionError("no native record operation")
        return observation("cpython", "cpython", "cpython", "Native record construction, flag writes and field reads",
                           "Original CPython hooks, body and assertions", toolchain.level, native_calls=calls)
    executions[PREFIX + "LogRecordTest.test_optional"] = optional
    representation = PREFIX + "LogRecordTest.test_str_rep"
    executions[representation] = native_method(toolchain, directory("record-repr"), SOURCE, "LogRecordTest",
        representation, native_records.REPR_FIXTURE, operation=("imre", "Native LogRecord construction and repr"))
    return executions


class MethodProbe:
    """Compile the entire source method with its referenced module imports."""

    def __init__(self, source):
        tree = ast.parse(source.read_bytes())
        self.lines = source.read_text().splitlines()
        self.classes = {c.name: {n.name: n for n in c.body if isinstance(n, ast.FunctionDef)}
                        for c in tree.body if isinstance(c, ast.ClassDef)}
        self.imports = {}
        for node in tree.body:
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                for alias in node.names:
                    name = alias.asname or alias.name.split(".")[0]
                    if name in {"logging", "unittest"}:
                        continue
                    prefix = f"from {node.module} import " if isinstance(node, ast.ImportFrom) else "import "
                    self.imports[name] = prefix + ast.unparse(alias)

    def attempt(self, case, toolchain, work):
        name = case._testMethodName
        owner = next(c for c in type(case).__mro__ if name in vars(c))
        node = self.classes.get(owner.__name__, {}).get(name)
        if node is None:
            line = getattr(owner, name).__code__.co_firstlineno
            reason = f"Generated method from {owner.__name__} at test_logging.py:{line} requires its original closure"
            return case.id(), {"outcome": "blocked", "reason": reason,
                               "roles": dict.fromkeys(("body", "assertions", "operation", "fixture"), "unobserved"),
                               "evidence": {"stage": "fixture adapter"}}
        if node.decorator_list:
            decorators = ", ".join(ast.unparse(d) for d in node.decorator_list)
            reason = f"Original method decorators at test_logging.py:{node.lineno} require a fixture adapter: {decorators}"
            result = compile_blocker(reason)
            result["evidence"]["stage"] = "fixture adapter"
            return case.id(), result
        names = {n.id for n in ast.walk(node) if isinstance(n, ast.Name)}
        imports = [self.imports[n] for n in sorted(names & self.imports.keys())]
        if "logging" in names:
            imports.append("import logging")
            attrs = {ast.unparse(n) for n in ast.walk(node) if isinstance(n, ast.Attribute)}
            imports.extend(f"import logging.{pkg}" for pkg in ("handlers", "config") if f"logging.{pkg}" in attrs)
        adapter = "\n".join(imports) + "\n" + testcase_fixture()
        with tempfile.TemporaryDirectory(dir=work) as tmp:
            try:
                native_method(toolchain, Path(tmp), SOURCE, owner.__name__, case.id(), adapter)
            except NativeCompileError as exc:
                lines = [line.strip() for line in str(exc).replace(tmp, "<native-case>").splitlines()
                         if line.strip() and not line.strip().startswith("Compiling ")]
                diagnostic = "\n".join(lines[:4])
                result = compile_blocker(diagnostic)
                result["evidence"].update(stage="native method compilation", source=f"Lib/test/test_logging.py:{node.lineno}")
                # suite_native prefixes the unmodified def with the adapter and one newline.
                location = re.search(r"<native-case>/case\.py:(\d+):(\d+):", diagnostic)
                if location:
                    line = node.lineno + int(location[1]) - len((adapter + "\n").splitlines()) - 1
                    if node.lineno <= line <= node.end_lineno:
                        result["evidence"].update(source=f"Lib/test/test_logging.py:{line}:{location[2]}",
                                                  statement=self.lines[line - 1].strip())
                return case.id(), result
        raise RuntimeError(f"{case.id()} compiled: run the complete method and audit its operation and fixture")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("use a test-enabled CPython 3.14.5 with assertions enabled")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    from test import test_logging
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2], manifest["files"] | manifest["reference_files"])
    reference = json.loads((HERE / f"reference-{sys.platform}.json").read_text())
    reports = []
    probe = MethodProbe(SOURCE)
    for level in (0, 3):
        toolchain = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as temp:
            work = Path(temp)
            selected = known_cases(toolchain, work, test_logging)
            suite = load_suite(manifest["module"])
            discovered = list(cases(suite))
            assert len(set(c.id() for c in discovered)) == len(discovered) == reference["denominator"]
            exceptions = reference_skips(reference, level)
            remaining = [c for c in discovered if c.id() not in selected and (c.id(), 1, level) not in exceptions]
            with ThreadPoolExecutor(max_workers=2) as pool:
                preflight = {(name, 1): obs for name, obs in
                             pool.map(lambda c: probe.attempt(c, toolchain, work), remaining)}
            report = run_evidence(suite, selected, reference, level, preflight=preflight, exceptions=exceptions)
            reports.append(report)
            print(f"-O{level}: {report['counts']}", flush=True)
    write_reports(args.result, reports, reference)
    return int(any(r["counts"].get("failed") or r["counts"].get("unexpected-skip") for r in reports))


if __name__ == "__main__":
    sys.exit(main())

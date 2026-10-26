"""Exercise the original Logger.makeRecord methods in their real CPython test fixtures."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
SOURCE = STDLIB / "upstream/Lib/test/test_logging.py"
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures, testcase_fixture
from suite_evidence import roles

PREFIX = "test.test_logging.LoggerTest.test_make_record_with_extra_"
ADD, COLLIDE = PREFIX + "no_overwrite", PREFIX + "overwrite"
SETUP = '''    logger: logging.Logger

    def __init__(self) -> None:
        self.logger = logging.getLogger("blah")
'''
MEMBER = '''    def assertIn(self, key: str, container) -> None:
        if key not in container:
            raise AssertionError(key)
'''
FIXTURE = ("Original CPython LoggerTest setup, recording handler, teardown and cleanups; "
           "native creation uses a logger with the observed name. makeRecord does not dispatch to handlers.")


def blocked_at(exc, work, statement, expression, reason):
    lines = (work / "case.py").read_text().splitlines()
    (line, source), = [(n, text) for n, text in enumerate(lines, 1) if text.strip() == statement]
    anchors = [(expression, reason)]
    if statement == "self.assertIn('valid_key', result.__dict__)":
        anchors.append(("result.__dict__", "unsupported native unittest membership container"))
    elif statement == "for key in ('message', 'asctime') + tuple(rv.__dict__.keys()):":
        anchors.append(("rv.__dict__.keys()", "supported dictionary methods are clear(), copy(), "
                        "update(), get(key[, default]), pop(key[, default]), loop-head items(), "
                        "and keys() in a key union"))
    for word, diagnostic in anchors:
        column = source.index(word) + 1
        if any(text.endswith(f"case.py:{line}:{column}: Imre: {diagnostic}")
               for text in str(exc).splitlines()):
            return diagnostic
    raise exc


def host_collisions(binary, case, method):
    from test import test_logging
    import logging
    if logging.getLogRecordFactory() is not logging.LogRecord:
        raise AssertionError("unexpected original record factory")
    calls = []
    with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True, encoding="utf-8") as child:
        def call(command, *args):
            child.stdin.write("\n".join(map(str, (command, *args))) + "\n")
            child.stdin.flush()
            reply = child.stdout.readline()
            if not reply:
                raise RuntimeError(f"native {command} failed: {child.stderr.read()}")
            calls.append([command, *args, reply.rstrip("\n")])
            return reply.rstrip("\n")

        class Fields:
            def __init__(self, index):
                self.index = index

            def keys(self):
                return ast.literal_eval(call("keys", self.index))

        class Record:
            def __init__(self, index):
                self.index = index

            @property
            def __dict__(self):
                return Fields(self.index)

        def factory(name, level, fn, lno, msg, args, exc_info, func, sinfo):
            if any(v is not None for v in (fn, lno, msg, args, exc_info, func, sinfo)):
                raise AssertionError("unexpected record fixture operands")
            return Record(int(call("factory", name, level)))

        def make(name, level, fn, lno, msg, args, exc_info, func=None, extra=None, sinfo=None):
            if any(v is not None for v in (fn, lno, msg, args, exc_info, func, sinfo)):
                raise AssertionError("unexpected logger fixture operands")
            (key, value), = extra.items()
            reply = call("make", name, level, key, value)
            if reply.startswith("key-error "):
                raise KeyError(ast.literal_eval(reply.removeprefix("key-error ")))
            return Record(int(reply.removeprefix("record ")))

        try:
            with patch.object(test_logging, "logging", SimpleNamespace(_logRecordFactory=factory)), \
                    patch.object(case, "logger", SimpleNamespace(makeRecord=make)):
                method()
        finally:
            if child.poll() is None:
                child.stdin.write("stop\n")
                child.stdin.close()
            if child.wait(timeout=15) or child.stderr.read():
                raise RuntimeError("native record driver failed")
    factory_call, keys, *attempts = calls
    if (factory_call[0] != "factory" or keys[0] != "keys"
            or [entry[3] for entry in attempts] != ["message", "asctime", *ast.literal_eval(keys[-1])]
            or not all(entry[0] == "make" and entry[-1].startswith("key-error ") for entry in attempts)):
        raise AssertionError(calls)
    return calls


def record_cases(toolchain, work):
    result = {}
    for case_id in (ADD, COLLIDE):
        folder = work / case_id.rsplit(".", 1)[1]
        folder.mkdir()

        def execute(case, method, case_id=case_id, folder=folder):
            if case.logger.name != "blah" or case.logger.handlers != [case.recording]:
                raise AssertionError("unexpected original LoggerTest fixture")
            description = ("Create a native record with extra and check the live dictionary"
                           if case_id == ADD else "Create the default record, read its keys and reject each protected extra")
            stock = "import logging\n" + testcase_fixture() + SETUP.replace(
                "self.logger =", "super().__init__()\n        self.logger =")
            try:
                run = native_method(toolchain, folder, SOURCE, "LoggerTest", case_id, stock,
                                    operation=("imre", description))
            except NativeCompileError as exc:
                if case_id == ADD:
                    statement = "self.assertIn('valid_key', result.__dict__)"
                    blocker = blocked_at(exc, folder, statement, "self.assertIn", "unknown class method")
                    run = native_method(toolchain, folder, SOURCE, "LoggerTest", case_id,
                                        "import logging\nclass NativeCase:\n" + SETUP + MEMBER,
                                        operation=("imre", description))
                    observed = run(case, method)
                    observed["observation"]["evidence"].update(
                        assertion="Native dictionary membership on the two original operands",
                        shared_assertion_blocker=blocker, native_body_statement=statement)
                else:
                    statement = "for key in ('message', 'asctime') + tuple(rv.__dict__.keys()):"
                    blocker = blocked_at(exc, folder, statement, "rv.__dict__.keys()",
                        "supported dictionary methods are clear(), get(key[, default]), pop(key[, default]), loop-head items(), and keys() in a key union")
                    binary = toolchain.build((HERE / "make_record_driver.py").read_bytes(), folder)
                    calls = host_collisions(binary, case, method)
                    observed = {"observation": {"roles": roles(body="cpython", assertions="cpython",
                               operation="imre", fixture="mixed"), "evidence": {"operation": description,
                               "opt_level": toolchain.level, "native_calls": calls,
                               "native_body_diagnostic": blocker, "native_body_statement": statement}}}
            else:
                observed = run(case, method)
            observed["observation"]["evidence"]["fixture"] = FIXTURE
            return observed
        result[case_id] = execute
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("requires the pinned test-enabled CPython 3.14.5")
    sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(args.cpython_source, manifest["files"] | manifest["reference_files"])
    controls = {name: subprocess.run([sys.executable, "-I", "-B", HERE / name], capture_output=True, check=True)
                for name in ("make_record_values.py", "make_record_operands.py")}
    report = {"cpython_commit": manifest["commit"], "denominator": 272, "levels": {}}
    for level in (0, 3):
        tool = Toolchain(args.imre, args.runtime, args.clang, level)
        suite = load_suite(manifest["module"])
        assert len(list(cases(suite))) == report["denominator"]
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            compared = {}
            for name, oracle in controls.items():
                child = work / Path(name).stem
                child.mkdir()
                native = tool.build((HERE / name).read_bytes(), child)
                actual = subprocess.run([native], capture_output=True, timeout=30)
                if (actual.returncode, actual.stdout, actual.stderr) != (oracle.returncode, oracle.stdout, oracle.stderr):
                    raise AssertionError((name, level, actual, oracle))
                compared[name] = {"observations": len(actual.stdout.splitlines()),
                                  "stdout_sha256": hashlib.sha256(actual.stdout).hexdigest()}
            selected = record_cases(tool, work)
            evidence, failures, skips = run_in_fixtures(suite, selected)
            if failures or skips or set(evidence) != set(selected):
                raise AssertionError((failures, skips, evidence))
            report["levels"][str(level)] = {"original_methods": {k:v[0]["observation"] for k,v in evidence.items()},
                "controls": compared}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print("Both original Logger.makeRecord methods exercised native operations at O0 and O3; see their body and assertion roles in the report.")


if __name__ == "__main__":
    main()

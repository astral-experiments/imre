"""Run unchanged logging.config routines with original fixtures and native operations."""

import argparse
import builtins
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_evidence import roles
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain, run_in_fixtures

EXPECTED = {
    "ConfigDictTest.test_config12_failure": ("ValueError", lambda _: "dictionary doesn't specify a version"),
    "ConfigDictTest.test_config13_failure": ("ValueError", lambda cfg: f"Unsupported version: {cfg['version']}"),
    "ConfigFileTest.test_exception_if_confg_file_is_empty": ("RuntimeError", lambda path: f"{path} is an empty file"),
    "ConfigFileTest.test_exception_if_config_file_does_not_exist": ("FileNotFoundError", lambda path: f"{path} doesn't exist"),
}


def config_cases(toolchain, directory, original):
    binary = toolchain.build((HERE / "config_driver.py").read_bytes(), directory)
    def drive(case, method):
        short = case.id().removeprefix("test.test_logging.")
        expected_type, expected_text = EXPECTED[short]
        calls = []
        def invoke(kind, value):
            if kind == "file":
                command, data = [binary, "file", value], None
            else:
                # These original configs contain only JSON data. All parsing,
                # validation, lookup and exception creation occur in Imre.
                command, data = [binary, "dict"], json.dumps(value) + "\n"
            child = subprocess.run(command, input=data, capture_output=True, text=True, cwd=directory, timeout=30)
            reply = child.stdout.splitlines()
            calls.append((value, reply, child.returncode, child.stderr))
            if len(reply) == 2 and reply[0] in {"FileNotFoundError", "RuntimeError", "NotImplementedError", "ValueError"}:
                raise getattr(builtins, reply[0])(reply[1])
        config = SimpleNamespace(dictConfig=lambda value: invoke("dict", value), fileConfig=lambda value: invoke("file", value))
        with patch.object(original, "logging", SimpleNamespace(config=config)):
            method()
        # The upstream dictionary tests use assertRaises(Exception). A compiler,
        # transport or adapter failure must not satisfy that broad assertion.
        if len(calls) != 1:
            raise AssertionError(f"expected one configuration call: {calls!r}")
        value, reply, code, stderr = calls[0]
        if code or stderr or reply != [expected_type, expected_text(value)]:
            raise AssertionError(f"wrong native outcome: {code}, {reply!r}, {stderr!r}")
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"operation": "Native logging.config." + ("dictConfig" if isinstance(value, dict) else "fileConfig"),
                             "fixture": "Original CPython hooks, including the original temporary file when requested",
                             "opt_level": toolchain.level, "exception_type": expected_type}}
    return {"test.test_logging." + name: drive for name in EXPECTED}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
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
    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
        with tempfile.TemporaryDirectory(dir=work) as exe:
            sample = (HERE / "config_values.py").read_bytes()
            binary = tools.build(sample, Path(exe))
            missing, empty, valid = [work / s for s in ("missing.ini", "empty.ini", "valid.ini")]
            empty.touch()
            valid.write_text("[formatters]\nkeys=\n[handlers]\nkeys=\n[loggers]\nkeys=root,app\n"
                             "[logger_root]\nlevel=CRITICAL\nhandlers=\n[logger_app]\nqualname=service\n"
                             "level=INFO\nhandlers=\npropagate=0\n")
            outputs = [subprocess.run([*cmd, missing, empty, valid], capture_output=True, check=True, timeout=30)
                       for cmd in ([sys.executable, HERE / "config_values.py"], [binary])]
            if (outputs[0].stdout, outputs[0].stderr) != (outputs[1].stdout, outputs[1].stderr):
                raise AssertionError(outputs)
        callbacks = config_cases(tools, work, test_logging)
        suite = load_suite(manifest["module"])
        denominator = len(list(cases(suite)))
        observed, errors, skips = run_in_fixtures(suite, callbacks)
        if errors or skips or set(observed) != set(callbacks):
            raise AssertionError((errors, skips, set(observed)))
        args.result.write_text(json.dumps({"opt_level": args.opt_level, "denominator": denominator,
                                           "evidence": observed}, indent=2) + "\n")
        print(f"-O{args.opt_level}: {len(observed)} original bodies, native configuration, CPython fixtures and assertions")


if __name__ == "__main__":
    main()

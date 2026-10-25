"""Execute original BasicConfigTest bodies against a persistent native root."""
from ast import literal_eval
from contextlib import chdir
import re
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles
from suite_native import NativeCompileError, native_method, testcase_fixture

PREFIX = "test.test_logging.BasicConfigTest."
HERE = Path(__file__).resolve().parent


def option_cases(tools, work, source, prepare, setup):
    from test import test_logging
    binary = tools.build((HERE / "basic_config_driver.py").read_bytes(), work)

    def execute(case, method):
        if test_logging.logging.root.handlers or test_logging.logging.root.level != case.original_logging_level:
            raise AssertionError("the CPython fixture did not start with an empty root")
        desc = {
            "test_incompatible": "Create one native StreamHandler and run five basicConfig calls, including four native ValueErrors",
            "test_encoding_errors": "Emit through the native root with an ASCII file handler and the ignore policy",
            "test_encoding_errors_default": "Read the native FileHandler error policy and emit ASCII with default backslash replacement",
            "test_encoding": "Configure and inspect a native FileHandler, emit UTF-8 through the root, and close it before the original file assertion",
        }[case._testMethodName]
        attempt = work / case._testMethodName
        attempt.mkdir()
        fixture = prepare + "\nimport os\n" + testcase_fixture() + setup.replace("LEVEL", repr(case.original_logging_level))
        try:
            run_native = native_method(tools, attempt, source, "BasicConfigTest", case.id(), fixture,
                                       operation=("imre", desc))
        except NativeCompileError as exc:
            diagnostic = str(exc).strip().replace(str(attempt), "<native-case>")
            location = re.search(r"case\.py:(\d+):", diagnostic)
            statement = (attempt / "case.py").read_text().splitlines()[int(location[1]) - 1].strip() if location else ""
            if statement not in {"assertRaises = self.assertRaises", "self.assertIsInstance(handler, logging.FileHandler)"}:
                raise
        else:
            with chdir(attempt):
                return run_native(case, method)
        calls = []
        handle = object()
        with subprocess.Popen([binary, str(case.original_logging_level)], cwd=work, text=True,
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as child:
            def send(op, *values):
                lines = [op, *map(str, values)]
                if any("\n" in line or "\r" in line for line in lines):
                    raise AssertionError("invalid native transport argument")
                child.stdin.write("\n".join(lines) + "\n")
                child.stdin.flush()
                answer = child.stdout.readline()
                if not answer:
                    raise AssertionError("native logging did not reply to " + op)
                answer = answer.removesuffix("\n")
                calls.append([op, answer])
                return answer

            def checked(op, *values):
                answer = send(op, *values)
                if answer.startswith("ValueError:"):
                    raise ValueError(answer.removeprefix("ValueError:"))
                if answer != "ok":
                    raise AssertionError(answer)

            class Handler:
                def __init__(self, index):
                    self.index = index

                def close(self):
                    checked("handler-close", self.index)

            class FileHandler(Handler):
                @property
                def encoding(self):
                    return send("handler-encoding", self.index)

                @property
                def errors(self):
                    return literal_eval(send("handler-errors", self.index))

            class RootHandlers:
                def __len__(self):
                    return int(send("count"))

                def __getitem__(self, index):
                    is_file = send("handler-kind", index)
                    if is_file not in ("True", "False"):
                        raise AssertionError(is_file)
                    return (FileHandler if is_file == "True" else Handler)(index)

            class NativeLogging:
                root = SimpleNamespace(handlers=RootHandlers())

                @property
                def INFO(self):
                    return int(send("info"))

                @property
                def DEBUG(self):
                    return int(send("debug-level"))

                def StreamHandler(self):
                    checked("new")
                    return handle

                def debug(self, message):
                    checked("debug", message)

                def basicConfig(self, **options):
                    if "stream" in options and options["stream"] is not sys.stderr:
                        raise AssertionError("unknown native stream")
                    if "handlers" in options and options["handlers"] != [handle]:
                        raise AssertionError("unknown native handler")
                    names = ",".join(options)
                    args = [v for k, v in options.items() if k not in {"stream", "handlers"} and v is not None]
                    if "filename" in options and options["filename"] is None:
                        names = names.replace("filename", "filename=None")
                    checked(names, *args)

            proxy = NativeLogging()
            proxy.FileHandler = FileHandler
            try:
                with chdir(work), patch.object(test_logging, "logging", proxy):
                    method()
                if send("count") != "1" or (work / "test.log").exists():
                    raise AssertionError("unexpected root handler count or leftover file")
            finally:
                if child.poll() is None:
                    child.stdin.write("stop\n")
                child.stdin.close()
                if child.wait(timeout=15) or child.stderr.read():
                    raise AssertionError("native basicConfig child failed")
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="mixed"),
                "evidence": {"operation": desc, "fixture": "Original CPython setup and cleanup; native root starts at the saved level",
                             "native_calls": calls, "opt_level": tools.level,
                             "native_body_diagnostic": diagnostic, "native_body_statement": statement}}
    return {PREFIX + name: execute for name in ("test_incompatible", "test_encoding", "test_encoding_errors", "test_encoding_errors_default")}

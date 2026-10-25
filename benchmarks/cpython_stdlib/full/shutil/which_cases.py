"""Run unchanged shutil methods with native assertions and transfer their real host fixtures."""

from pathlib import Path
import sys
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from native_shutil import driver_source, invoke
from suite_inventory import cases, load_suite
from suite_native import native_method
from suite_evidence import roles


def which_fixture(case):
    fields = {key: getattr(case, key) for key in ("file", "filepath", "dir", "other_dir")}
    kind = type(case.file).__name__
    return ("import shutil\n\nclass NativeCase:\n"
            + "".join(f"    {key}: {kind}\n" for key in fields)
            + "    def __init__(self) -> None:\n"
            + "".join(f"        self.{key} = {value!r}\n" for key, value in fields.items())
            + f"    def assertEqual(self, actual: {kind} | None, expected: {kind}) -> None:\n"
              "        assert actual is not None\n"
              "        assert actual == expected\n"
              "    def assertIsNone(self, actual: str | None) -> None:\n"
              "        assert actual is None\n")


def executors(tools, work, source, module):
    executions = {}
    calls = {}
    which_driver = work / "which_driver"
    which_driver.mkdir()
    binary = tools.build(driver_source().encode(), which_driver)

    def host_which(case, original):
        name = case.id()
        calls[name] = 0
        def which(*args, **kwargs):
            calls[name] += 1
            return invoke(binary, *args, **kwargs)
        # Keep internal host fixture operations on the original shutil module.
        with patch.object(module, "shutil", SimpleNamespace(which=which)):
            original()
        assert calls[name] > 0
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"operation": "shutil.which", "native_calls": calls[name], "opt_level": tools.level,
                             "fixture": "original TestWhich fixture, environment, executable files and assertions"}}

    suite = load_suite("test.test_shutil")
    for case in cases(suite):
        if isinstance(case, module.TestWhich):
            executions[case.id()] = host_which
    def register(owner, method):
        case_id = f"test.test_shutil.{owner}.{method}"
        def run(case, original):
            directory = work / f"{owner}.{method}"
            directory.mkdir()
            execute = native_method(tools, directory, source, "TestWhich",
                                    case_id, which_fixture(case), operation=("imre", "shutil.which"))
            return execute(case, original)
        executions[case_id] = run

    for owner in ("TestWhich", "TestWhichBytes"):
        for method in ("test_basic", "test_absolute_cmd", "test_nonexistent_file"):
            register(owner, method)
    return executions, calls

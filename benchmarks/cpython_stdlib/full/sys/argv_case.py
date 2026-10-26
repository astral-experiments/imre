"""Run the exact test_orig_argv command and keep the two file-launch controls separate."""

import ast
import hashlib
import os
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles

CASE_ID = "test.test_sys.SysModuleTest.test_orig_argv"


def launch_controls(args, toolchain, folder):
    child_source = args[4].encode()
    folder.mkdir()
    binary = toolchain.build(child_source, folder)
    entry = str(folder / "case.py")
    compiler = str(toolchain.imre.resolve())
    runtime, clang = str(toolchain.runtime.resolve()), str(toolchain.clang.resolve())
    paths = {str(binary): "<native-child>", entry: "<child-source>", compiler: "<imre>",
             runtime: "<runtime>", clang: "<clang>"}
    flags = ["--runtime", runtime, "--clang", clang, "--opt-level", str(toolchain.level), "--quiet", "--no-cache"]
    controls = {}
    for mode, invocation, argv0 in (
        ("executable", [str(binary), *args[5:]], str(binary)),
        ("imre run", [compiler, "run", entry, *flags, "--", *args[5:]], entry),
    ):
        run = subprocess.run(invocation, capture_output=True, text=True, check=True, timeout=45)
        argv, orig_argv = map(ast.literal_eval, run.stdout.splitlines())
        if run.stderr or argv != [argv0, *args[5:]] or orig_argv != invocation:
            raise AssertionError((mode, argv, orig_argv, run.stderr))
        controls[mode] = {"returncode": run.returncode, "argv": [paths.get(x, x) for x in argv],
                          "orig_argv": [paths.get(x, x) for x in orig_argv]}
    return controls


def native_command_env(toolchain):
    build_flags = ["--opt-level", str(toolchain.level), "--runtime", str(toolchain.runtime.resolve()),
                   "--clang", str(toolchain.clang.resolve()), "--no-cache"]
    return {**os.environ, "IMRE_ENCODED_FLAGS": "\x1f".join(build_flags)}


def observer(module, toolchain, folder, method_sha256):
    compiler = str(toolchain.imre.resolve())

    def execute(case, original):
        subjects = {}
        for exe, subject in ((sys.executable, "cpython"), (compiler, "imre")):
            calls = []

            def capture(command, **kwargs):
                if command[:5] != [exe, "-I", "-X", "utf8", "-c"]:
                    raise AssertionError(command)
                env = native_command_env(toolchain) if subject == "imre" else None
                result = subprocess.run(command, **kwargs, env=env, timeout=45)
                calls.append((command, result))
                return result

            with patch.object(module, "sys", SimpleNamespace(executable=exe)), \
                    patch.object(module, "subprocess", SimpleNamespace(run=capture)):
                original()
            (command, result), = calls
            argv, orig = map(ast.literal_eval, result.stdout.splitlines())
            if result.stderr or orig != command:
                raise AssertionError((command, result))
            subjects[subject] = {"argv": argv, "orig_argv": [f"<{subject}>", *orig[1:]],
                                 "returncode": result.returncode}
        native = subjects["imre"]
        args = native["orig_argv"][1:]
        if native["argv"] != subjects["cpython"]["argv"] or args != subjects["cpython"]["orig_argv"][1:]:
            raise AssertionError(subjects)
        proof = {"operation": "sys.argv and sys.orig_argv in the real native -I -X utf8 -c child",
                 "fixture": "original CPython SysModuleTest and reap_children teardown",
                 "assertions": "original CPython comparison of both printed lists with repr",
                 "method_sha256": method_sha256, "child_sha256": hashlib.sha256(args[4].encode()).hexdigest(),
                 "native_calls": 2, "opt_level": toolchain.level, "original_child": native,
                 "stock_cpython_control": subjects["cpython"],
                 "independent_launch_controls": launch_controls(args, toolchain, folder)}
        return {"execution": "host-driven-native", "body": "CPython; original child: Imre",
                "assertions": "CPython", "fixture_execution": "CPython module/class/instance hooks",
                "calls": ["sys.argv", "sys.orig_argv"],
                "observation": {"roles": roles(body="cpython", assertions="cpython",
                                               operation="imre", fixture="cpython"), "evidence": proof}}
    return execute

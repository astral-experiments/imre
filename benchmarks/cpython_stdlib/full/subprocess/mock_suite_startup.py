"""Build the untouched suite, then isolate its mock import and original waitpid patch."""

import argparse
import ast
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from textwrap import dedent

from signal_race_originals import (
    HERE,
    SOURCE,
    Toolchain,
    build_waitpid_extension,
    hash_file,
    verify_files,
    verify_interpreter_tests,
)

IMPORT = "import test.test_subprocess as target\n"
RUNNER = IMPORT + (
    "import unittest\nsuite = unittest.defaultTestLoader.loadTestsFromModule(target)\n"
    "unittest.TextTestRunner(verbosity=2).run(suite)\n"
)
MOCK_IMPORT = """from unittest import mock
calls = mock.Mock(name="waitpid")
calls.assert_not_called()
calls(1234, 1)
calls.assert_called_once_with(1234, 1)
print(calls.call_count)
"""


def native(tc, work, entry, *, reachable=False, site=None, args=()):
    binary = work / "program"
    build = subprocess.run(
        [
            tc.imre,
            "build",
            entry,
            "--module-root",
            entry.parent,
            "--runtime",
            tc.runtime,
            "--clang",
            tc.clang,
            "--opt-level",
            str(tc.level),
            "--no-cache",
            "--quiet",
            "-o",
            binary,
            *(["--reachable-only"] if reachable else []),
            *(["--extension-path", site] if site else []),
        ],
        capture_output=True,
        text=True,
        timeout=180,
    )
    if build.returncode:
        return dict(
            phase="build",
            exit=build.returncode,
            ran=False,
            diagnostic=(build.stdout + build.stderr)
            .strip()
            .replace(str(work), "<work>"),
        )
    proc = subprocess.run([binary, *args], capture_output=True, text=True, timeout=240)
    return dict(
        phase="execute",
        ran=True,
        exit=proc.returncode,
        stdout=proc.stdout,
        stderr=proc.stderr.replace(str(work), "<work>"),
    )


def original_patch():
    lines = SOURCE.read_text().splitlines(keepends=True)
    method = next(
        n
        for n in ast.walk(ast.parse("".join(lines)))
        if isinstance(n, ast.FunctionDef) and n.name == "test_stopped"
    )
    [context] = [n for n in method.body if isinstance(n, ast.With)]
    return dedent("".join(lines[context.lineno - 1 : context.end_lineno]))


def stage_tests(git, pin, work):
    with subprocess.Popen(
        ["git", "-C", git, "archive", "--format=tar", pin, "--", "Lib/test"],
        stdout=subprocess.PIPE,
    ) as archive:
        with tarfile.open(fileobj=archive.stdout, mode="r|") as source:
            source.extractall(work, filter="data")
        if archive.wait() != 0:
            raise RuntimeError("could not stage pinned Lib/test")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-git", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    from _testcapi import W_STOPCODE

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    ref = json.loads((HERE / "reference-linux.json").read_text())
    assert sys.implementation.name == "cpython" and sys.version_info[:3] == (3, 14, 5)
    output = dict(
        cpython_commit=manifest["commit"],
        denominator=ref["denominator"],
        toolchain={
            k: hash_file(getattr(args, k)) for k in ("imre", "runtime", "clang")
        },
        original_patch=original_patch(),
        levels=[],
    )
    with tempfile.TemporaryDirectory(prefix="imre-subprocess-startup-") as tmp:
        work = Path(tmp)
        stage_tests(args.cpython_git, manifest["commit"], work)
        lib = work / "Lib"
        verify_files(
            lib / "test",
            {
                k.removeprefix("Lib/test/"): v
                for k, v in manifest["files"].items()
                if k.startswith("Lib/test/")
            },
        )
        entry = lib / "imre_suite.py"
        probe = work / "probe"
        probe.mkdir()
        imported = probe / "import_mock.py"
        imported.write_text(MOCK_IMPORT)
        direct = probe / "patch.py"
        direct.write_text(
            "from unittest import mock\nimport fixture_waitpid\nimport subprocess\nimport sys\n"
            "proc = subprocess.Popen([sys.argv[1], '-c', 'pass'])\n"
            "assert fixture_waitpid.raw_wait(proc.pid, 0) == proc.pid << 16\nstatus = int(sys.argv[2])\n"
            + original_patch()
            + "assert returncode == -3\n"
            "later = subprocess.Popen([sys.argv[1], '-c', 'raise SystemExit(7)'])\n"
            "assert later.wait() == 7\nprint(returncode, later.returncode)\n"
        )
        argv = [sys.executable, str(W_STOPCODE(3))]
        oracle = work / "oracle"
        oracle.mkdir()
        (oracle / direct.name).write_bytes(direct.read_bytes())
        (oracle / "fixture_waitpid.py").write_text(
            "import os\ndef raw_wait(pid, flags):\n"
            "    child, status = os.waitpid(pid, flags)\n    return (child << 16) | status\n"
        )
        control = subprocess.run(
            [sys.executable, "-E", "-B", oracle / direct.name, *argv],
            capture_output=True,
            text=True,
            check=True,
            timeout=15,
        )
        assert control.stdout == "-3 7\n" and not control.stderr
        output["reference_direct_patch_stdout"] = control.stdout
        site = build_waitpid_extension(
            Toolchain(args.imre, args.runtime, args.clang, 0), probe
        )
        for level in (0, 3):
            tc = Toolchain(args.imre, args.runtime, args.clang, level)
            suite = []
            for mode, text in [("import", IMPORT), ("runner", RUNNER)]:
                entry.write_text(text)
                for reachable in (False, True):
                    suite.append(
                        dict(
                            entry=mode,
                            reachable_only=reachable,
                            **native(tc, work, entry, reachable=reachable),
                        )
                    )
            rec = native(tc, work, imported)
            if rec["ran"]:
                assert (rec["exit"], rec["stdout"], rec["stderr"]) == (0, "1\n", "")
            patch = native(tc, work, direct, site=site, args=argv)
            if patch["ran"]:
                assert (patch["exit"], patch["stdout"], patch["stderr"]) == (
                    0,
                    control.stdout,
                    "",
                )
            output["levels"].append(
                dict(
                    opt_level=level,
                    unchanged_suite=suite,
                    shared_mock_import_and_call=rec,
                    native_direct_patch=patch,
                )
            )
    args.result.write_text(json.dumps(output, indent=2) + "\n")


if __name__ == "__main__":
    main()

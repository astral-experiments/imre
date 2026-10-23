"""Live conformance oracles, failure cases, and filesystem-effect checks."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import signal
import stat
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "examples" / "conformance_report"
REVISION = "835b70f3f795f8e061b1f7621178515b0a77a0ad"
SENTINEL = b"previous complete report must survive\n"
_offline = runpy.run_path(str(ROOT / "benchmarks" / "conformance_validate.py"))
environment = _offline["environment"]
python_command = _offline["python_command"]


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def original_command(python: str | Path) -> list[str]:
    # original_live.py imports its sibling original.py, so deliberately retain
    # the script directory while disabling user-site packages and bytecode.
    return [str(python), "-s", "-B", str(APP / "original_live.py")]


def error_kind(stderr: bytes) -> str | None:
    matches = re.findall(
        r"(TOMLDecodeError|JSONDecodeError|UnicodeDecodeError|FileNotFoundError|"
        r"FileExistsError|IsADirectoryError|NotADirectoryError|PermissionError|"
        r"AssertionError|ValueError|KeyError|CalledProcessError|TimeoutExpired|OSError):",
        stderr.decode("utf-8", errors="replace"),
    )
    if not matches:
        return None
    return "ValueError" if matches[-1] in {"TOMLDecodeError", "JSONDecodeError"} else matches[-1]


def tree_identity(directory: Path) -> dict[str, str]:
    result = {}
    for path in sorted(directory.rglob("*")):
        name = path.relative_to(directory).as_posix()
        if path.is_symlink():
            result[name] = "symlink:" + os.readlink(path)
        elif path.is_dir():
            result[name] = "directory:" + oct(stat.S_IMODE(path.stat().st_mode))
        else:
            assert path.is_file(), (path, "unexpected special filesystem entry")
            result[name] = "file:" + oct(stat.S_IMODE(path.stat().st_mode)) + ":" + sha256(path.read_bytes())
    return result


def run_process(command: list[str], *, directory: Path, env: dict[str, str], timeout: float = 120) -> subprocess.CompletedProcess:
    """Reap the process and kill its child group if the outer deadline fails."""
    with subprocess.Popen(command, cwd=directory, env=env, stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True) as process:
        try:
            stdout, stderr = process.communicate(b"", timeout=timeout)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.communicate()
            raise
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def live_arguments(suite: Path, old_ty: Path, new_ty: Path, output: Path, *, format: str = "github", timeout: float = 15.0) -> list[str]:
    return ["--suite", str(suite), "--old-ty", str(old_ty), "--new-ty", str(new_ty),
            "--output", str(output), "--suite-revision", REVISION, "--format", format,
            "--force-summary-table", "--timeout", str(timeout)]


def run_checked(command: list[str], arguments: list[str], directory: Path, expected: bytes | str, *, extra_environment: dict[str, str] | None = None, output_is_directory: bool = False) -> dict:
    output = Path(arguments[arguments.index("--output") + 1])
    if not output.is_absolute():
        output = directory / output
    if output_is_directory:
        output.mkdir(exist_ok=True)
        (output / "preserve").write_bytes(SENTINEL)
    else:
        output.write_bytes(SENTINEL)
    before = tree_identity(directory)
    env = {**environment(directory / "home"), "PYTHONPATH": ""}
    env.update(extra_environment or {})
    started = time.perf_counter_ns()
    completed = run_process([*command, *arguments], directory=directory, env=env)
    elapsed = time.perf_counter_ns() - started
    assert completed.stdout == b"", (command, completed.stdout)
    if isinstance(expected, bytes):
        assert completed.returncode == 0, (command, completed.stderr.decode(errors="replace"))
        assert completed.stderr == b"", (command, completed.stderr)
        assert output.is_file() and not output.is_symlink(), (command, "output is not a regular file")
        assert output.read_bytes() == expected, (command, "full report differs")
    else:
        assert completed.returncode != 0, (command, "expected failure", expected)
        assert error_kind(completed.stderr) == expected, (command, expected, completed.stderr.decode(errors="replace"))
        if output_is_directory:
            assert output.is_dir() and not output.is_symlink(), (command, "failed run replaced output directory")
        else:
            assert output.is_file() and not output.is_symlink(), (command, "failed run replaced output type")
            assert output.read_bytes() == SENTINEL, (command, "failed run replaced report")
    after = tree_identity(directory)
    if isinstance(expected, bytes):
        before.pop(output.relative_to(directory).as_posix())
        after.pop(output.relative_to(directory).as_posix())
    assert before == after, (command, "unexpected filesystem effects", before, after)
    return {"command": [*command, *arguments], "elapsed_ns": elapsed,
            "returncode": completed.returncode, "stdout_sha256": sha256(completed.stdout),
            "stderr_sha256": sha256(completed.stderr), "stderr_bytes": len(completed.stderr),
            "stdout_hex": completed.stdout.hex(), "stderr_hex": completed.stderr.hex(),
            "report_sha256": None if output_is_directory else sha256(output.read_bytes()),
            "report_bytes": None if output_is_directory else output.stat().st_size,
            "output_kind": "directory" if output_is_directory else "file",
            "error": None if isinstance(expected, bytes) else expected,
            "filesystem_guards_passed": True}


def expected_report(command: list[str], arguments: list[str], directory: Path) -> bytes:
    output = Path(arguments[arguments.index("--output") + 1])
    if not output.is_absolute():
        output = directory / output
    before = tree_identity(directory)
    completed = run_process([*command, *arguments], directory=directory,
                            env={**environment(directory / "home"), "PYTHONPATH": ""})
    assert completed.returncode == 0 and not completed.stdout and not completed.stderr, completed.stderr
    assert output.is_file() and not output.is_symlink(), "upstream output is not a regular file"
    report = output.read_bytes()
    output.unlink()
    assert tree_identity(directory) == before, "upstream adapter left unexpected files"
    return report


def make_fixture(directory: Path, python: str | Path) -> tuple[Path, Path]:
    suite = directory / "fixture-suite"
    (suite / "src").mkdir(parents=True)
    (suite / "tests").mkdir()
    (suite / "src" / "test_groups.toml").write_text('[cases]\nname = "Cases # literal"\nhref = "https://example.invalid/cases"\n', encoding="utf-8")
    (suite / "tests" / "cases_main.py").write_bytes(
        'value: int = "🦀"  # E?[café+]: incompatible\r\n# E: comment only\r'.encode()
    )
    (suite / "tests" / "cases_types.pyi").write_text('other: int = "x"  # E: stub\n', encoding="utf-8")
    (suite / "tests" / "support.py").write_text("support = True\n", encoding="utf-8")
    analyzer = directory / "fixture-ty"
    analyzer.write_text(f"#!{Path(python).absolute()} -B\n" + '''import json, os, pathlib, sys, time
mode = os.environ.get("RELAR_LIVE_FIXTURE", "ok")
expected = ["check", "--python-version=3.12", "--output-format=gitlab",
    "--ignore=assert-type-unspellable-subtype", "--error=ambiguous-protocol-member",
    "--error=invalid-enum-member-annotation", "--error=invalid-legacy-positional-parameter",
    "--error=mismatched-type-name", "--error=invalid-named-tuple-override",
    "--error=deprecated", "--error=redundant-final-classvar", "--exit-zero"]
assert sys.argv[1:13] == expected, sys.argv
suite = pathlib.Path.cwd()
assert sys.argv[13] == "--extra-search-path=" + str(suite / "tests"), sys.argv
assert [pathlib.Path(p).name for p in sys.argv[14:]] == ["cases_main.py", "cases_types.pyi"], sys.argv
if mode == "mutate-source":
    pathlib.Path(sys.argv[14]).write_bytes(b"\\xff")
if mode == "exit":
    print("intentional failure", file=sys.stderr)
    sys.exit(7)
if mode == "timeout":
    if "RELAR_LIVE_PID" in os.environ:
        pathlib.Path(os.environ["RELAR_LIVE_PID"]).write_text(str(os.getpid()))
    print("captured before timeout", flush=True)
    time.sleep(60)
if mode == "json":
    print("{invalid")
    sys.exit()
if mode == "utf8":
    sys.stdout.buffer.write(b"\\xff")
    sys.exit()
path = "tests/../tests/cases_main.py" if mode != "unknown-path" else "tests/missing.py"
print(json.dumps([{"check_name": "invalid-assignment", "description": "invalid-assignment: incompatible",
    "severity": "major", "location": {"path": path, "positions": {
        "begin": {"line": 1, "column": 1}, "end": {"line": 1, "column": 2}}}}]))
''', encoding="utf-8")
    analyzer.chmod(0o755)
    return suite, analyzer


def validate(commands: dict[str, list[str]], suite: Path, old_ty: Path, new_ty: Path, artifact: Path, python: str | Path) -> dict:
    artifact.mkdir(parents=True, exist_ok=False)
    (artifact / "home").mkdir()
    sources_before = tree_identity(suite)
    binaries_before = {str(path): sha256(path.read_bytes()) for path in (old_ty, new_ty)}
    oracle = original_command(python)
    records = []
    for name, old, new, format in (("unchanged-github", new_ty, new_ty, "github"),
                                   ("changed-github", old_ty, new_ty, "github"),
                                   ("changed-diff", old_ty, new_ty, "diff")):
        directory = artifact / name
        directory.mkdir()
        (directory / "home").mkdir()
        arguments = live_arguments(suite, old, new, directory / "report.md", format=format)
        expected = expected_report(oracle, arguments, directory)
        for engine, command in commands.items():
            records.append({"case": name, "engine": engine, **run_checked(command, arguments, directory, expected)})
    fixture_root = artifact / "fixtures"
    fixture_root.mkdir()
    fixture_suite, analyzer = make_fixture(fixture_root, python)
    (fixture_root / "suite-link").symlink_to(fixture_suite, target_is_directory=True)
    (fixture_root / "ty-link").symlink_to(analyzer)
    malformed_suites = []
    for name, contents, error in (
        ("toml-syntax", b"[cases\n", "ValueError"),
        ("toml-duplicate", b'[cases]\nname="x"\nhref="y"\n[cases]\n', "ValueError"),
        ("toml-schema", b'[cases]\nname="x"\n', "ValueError"),
        ("toml-type", b'[cases]\nname=1\nhref="y"\n', "ValueError"),
        ("toml-utf8", b"\xff", "UnicodeDecodeError"),
        ("no-cases", b'[unknown]\nname="x"\nhref="y"\n', "ValueError"),
    ):
        malformed = fixture_root / name
        shutil.copytree(fixture_suite, malformed)
        (malformed / "src/test_groups.toml").write_bytes(contents)
        malformed_suites.append((name, malformed, error))
    for name, contents, error in (("source-utf8", b"\xff", "UnicodeDecodeError"),
                                  ("no-annotations", b"# E: comment only\n", "AssertionError")):
        malformed = fixture_root / name
        shutil.copytree(fixture_suite, malformed)
        for source in (malformed / "tests").glob("cases_*"):
            source.write_bytes(contents)
        malformed_suites.append((name, malformed, error))
    fixture_before = tree_identity(fixture_root)
    pid_directory = artifact / "timeout-pids"
    pid_directory.mkdir()
    for mode, error in (("ok", None), ("exit", "CalledProcessError"),
                        ("timeout", "TimeoutExpired"), ("json", "ValueError"),
                        ("utf8", "UnicodeDecodeError"), ("unknown-path", "KeyError")):
        directory = artifact / ("fixture-" + mode)
        directory.mkdir()
        (directory / "home").mkdir()
        arguments = live_arguments(fixture_suite, analyzer, analyzer, directory / "report.md",
                                   timeout=0.2 if mode == "timeout" else 15.0)
        expected = error if error is not None else expected_report(oracle, arguments, directory)
        for engine, command in commands.items():
            pid_path = pid_directory / engine
            records.append({"case": "fixture-" + mode, "engine": engine,
                            **run_checked(command, arguments, directory, expected,
                                          extra_environment={"RELAR_LIVE_FIXTURE": mode, "RELAR_LIVE_PID": str(pid_path)})})
            if mode == "timeout":
                pid = int(pid_path.read_text())
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    pass
                else:
                    raise AssertionError((engine, "timed-out child remains alive", pid))
                records[-1]["timed_out_child_reaped"] = True
    directory = artifact / "fixture-relative-symlinks"
    directory.mkdir()
    (directory / "home").mkdir()
    arguments = live_arguments(Path("../fixtures/suite-link"), Path("../fixtures/ty-link"),
                               Path("../fixtures/ty-link"), Path("report.md"))
    expected = expected_report(oracle, arguments, directory)
    for engine, command in commands.items():
        records.append({"case": "fixture-relative-symlinks", "engine": engine,
                        **run_checked(command, arguments, directory, expected)})
    directory = artifact / "fixture-mutate-source"
    directory.mkdir()
    (directory / "home").mkdir()
    arguments = live_arguments(fixture_suite, analyzer, analyzer, directory / "report.md")
    expected = expected_report(oracle, arguments, directory)
    source = fixture_suite / "tests/cases_main.py"
    original_bytes = source.read_bytes()
    for engine, command in commands.items():
        try:
            records.append({"case": "fixture-mutate-source", "engine": engine,
                            **run_checked(command, arguments, directory, expected,
                                          extra_environment={"RELAR_LIVE_FIXTURE": "mutate-source"})})
            assert source.read_bytes() == b"\xff", (engine, "fixture analyzer never ran")
        finally:
            source.write_bytes(original_bytes)
    for name, malformed, error in malformed_suites:
        directory = artifact / ("fixture-" + name)
        directory.mkdir()
        (directory / "home").mkdir()
        arguments = live_arguments(malformed, analyzer, analyzer, directory / "report.md")
        for engine, command in commands.items():
            records.append({"case": "fixture-" + name, "engine": engine,
                            **run_checked(command, arguments, directory, error,
                                          extra_environment={"RELAR_LIVE_FIXTURE": "exit"})})
    directory = artifact / "fixture-publication-error"
    directory.mkdir()
    (directory / "home").mkdir()
    arguments = live_arguments(fixture_suite, analyzer, analyzer, directory / "report.md")
    for engine, command in commands.items():
        records.append({"case": "fixture-publication-error", "engine": engine,
                        **run_checked(command, arguments, directory, "IsADirectoryError",
                                      output_is_directory=True)})
    assert tree_identity(fixture_root) == fixture_before, "fixtures changed"
    assert tree_identity(suite) == sources_before, "typing checkout changed"
    assert {str(path): sha256(path.read_bytes()) for path in (old_ty, new_ty)} == binaries_before, "ty binaries changed"
    return {"schema": 2, "kind": "live-conformance-validation", "status": "passed",
            "suite": str(suite), "suite_revision": REVISION, "suite_files": sources_before,
            "ty_sha256": binaries_before, "records": records, "input_guards_passed": True}


def main() -> None:
    if sys.flags.optimize:
        raise SystemExit("validation requires assertions")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", required=True, type=Path)
    parser.add_argument("--old-ty", required=True, type=Path)
    parser.add_argument("--new-ty", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--relar", type=Path)
    parser.add_argument("--codon", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    commands = {"original": original_command(args.python), "port": python_command(args.python)}
    commands.update({name: [str(getattr(args, name).resolve(strict=True))]
                     for name in ("relar", "codon", "rust") if getattr(args, name) is not None})
    report = validate(commands, args.suite.resolve(strict=True), args.old_ty.resolve(strict=True),
                      args.new_ty.resolve(strict=True), args.output.resolve(), args.python)
    (args.output / "validation.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": report["status"], "checks": len(report["records"])}))


if __name__ == "__main__":
    main()

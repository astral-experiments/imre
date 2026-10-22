"""Full-output oracles and differential checks for the offline conformance tool."""

from __future__ import annotations

import argparse
import copy
import hashlib
import importlib.util
import json
import random
import re
import subprocess
import sys
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path


if sys.flags.optimize:
    raise SystemExit("validation requires Python assertions; do not use -O")

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "examples" / "conformance_report"
SENTINEL = b"previous report must survive\n"
REVISION = "835b70f3f795f8e061b1f7621178515b0a77a0ad"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


@dataclass(frozen=True)
class Workload:
    manifest: Path
    old: Path
    new: Path
    format: str = "github"
    force_summary_table: bool = True

    def arguments(self) -> list[str]:
        args = ["--manifest", str(self.manifest), "--old", str(self.old),
                "--new", str(self.new), "--format", self.format]
        if self.force_summary_table:
            args.append("--force-summary-table")
        return args


@dataclass(frozen=True)
class Expected:
    report: str | None = None
    error: str | None = None


@dataclass(frozen=True)
class ComputeExpected:
    stdout: bytes = b""
    error: str | None = None
    report: str | None = None


def _python_command(python: str | Path, module: str) -> list[str]:
    bootstrap = (
        "import runpy,sys;root=sys.argv.pop(1);"
        "sys.path[:0]=[root+'/examples',root+'/stdlib'];"
        f"runpy.run_module({module!r},run_name='__main__')"
    )
    return [str(python), "-P", "-s", "-B", "-c", bootstrap, str(ROOT)]


def python_command(python: str | Path) -> list[str]:
    return _python_command(python, "conformance_report.__main__")


def python_compute_command(python: str | Path) -> list[str]:
    return _python_command(python, "conformance_report.compute")


def original_command(python: str | Path) -> list[str]:
    return [str(python), "-P", "-s", "-B", str(APP / "original.py")]


def original_compute_command(python: str | Path) -> list[str]:
    return [*original_command(python), "--compute"]


def environment(home: Path) -> dict[str, str]:
    return {
        "PATH": "/usr/bin:/bin", "HOME": str(home), "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8", "TZ": "UTC", "PYTHONHASHSEED": "0",
        "PYTHON_JIT": "0", "CODON_PYTHON": "/nonexistent/relar-native-codon-libpython.so",
    }


@lru_cache(maxsize=1)
def upstream():
    path = APP / "upstream" / "conformance.py"
    spec = importlib.util.spec_from_file_location("_conformance_validation_upstream", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@dataclass
class SourceSnapshot:
    """Supply retained source text while preserving the recorded path identity."""

    path: Path
    text: str

    @property
    def name(self) -> str:
        return self.path.name

    def read_text(self) -> str:
        return self.text

    def resolve(self) -> Path:
        # The manifest explicitly contains canonical absolute identities.
        return self.path


def _inputs(workload: Workload, source_contents: dict[str, str] | None):
    manifest = json.loads(workload.manifest.read_text(encoding="utf-8"))
    if not isinstance(manifest["suite_revision"], str):
        raise ValueError("suite revision must be a string")
    aliases = {}
    for item in manifest["diagnostic_paths"]:
        raw, path = item["raw"], item["path"]
        if not isinstance(raw, str) or not isinstance(path, str) or raw in aliases:
            raise ValueError("invalid or duplicate diagnostic path alias")
        aliases[raw] = path
    sources = []
    seen = set()
    for filename in manifest["test_files"]:
        if not isinstance(filename, str) or filename in seen:
            raise ValueError("invalid or duplicate conformance test path")
        seen.add(filename)
        if source_contents is None:
            text = Path(filename).read_text(encoding="utf-8")
        else:
            text = source_contents[filename].replace("\r\n", "\n").replace("\r", "\n")
        sources.append(SourceSnapshot(Path(filename), text))
    old = workload.old.read_text(encoding="utf-8")
    new = workload.new.read_text(encoding="utf-8")
    return manifest, aliases, sources, old, new


def error_kind(error: BaseException) -> str:
    if isinstance(error, (json.JSONDecodeError, UnicodeError)):
        return "ValueError" if isinstance(error, json.JSONDecodeError) else type(error).__name__
    return type(error).__name__


def oracle(workload: Workload, *, source_contents: dict[str, str] | None = None) -> Expected:
    """Use the unchanged upstream parser, models, grouping, metrics and renderers.

    source_contents makes this oracle portable: an archive can supply source
    bytes without changing the absolute path strings printed in diff reports.
    It replaces only source I/O; all report computation remains upstream.
    """
    try:
        manifest, aliases, sources, old_text, new_text = _inputs(workload, source_contents)
        original = upstream()
        original.CONFORMANCE_DIR_WITH_README = (
            f"https://github.com/python/typing/blob/{manifest['suite_revision']}/conformance/"
        )
        original.CONFORMANCE_URL = original.CONFORMANCE_DIR_WITH_README + "tests/{filename}#L{line}"
        expected = original.collect_expected_diagnostics(sources)
        for item in expected:
            item.location.path = item.location.path.path

        def diagnostics(text, source):
            result = []
            for item in json.loads(text):
                if item["severity"] != "major":
                    continue
                item["location"]["path"] = aliases[item["location"]["path"]]
                diagnostic = original.TyDiagnostic.from_gitlab_output(item, source=source)
                diagnostic.location.path = Path(item["location"]["path"])
                result.append(diagnostic)
            return result

        grouped = original.group_diagnostics_by_key(
            old=diagnostics(old_text, original.Source.OLD),
            new=diagnostics(new_text, original.Source.NEW), expected=expected,
        )
        stats = original.collect_file_stats(grouped, sources)
        rendered = "\n\n".join(filter(None, [
            original.render_summary(grouped, stats, force_summary_table=workload.force_summary_table),
            original.render_file_stats_table(stats),
            original.render_test_cases(grouped, format=workload.format),
        ]))
        return Expected(report=rendered)
    except Exception as error:
        return Expected(error=error_kind(error))


def compute_oracle(
    workload: Workload, rounds: int, *, source_contents: dict[str, str] | None = None,
) -> ComputeExpected:
    if rounds < 0:
        return ComputeExpected(error="ValueError")
    if rounds == 0:
        try:
            _inputs(workload, source_contents)
        except Exception as error:
            return ComputeExpected(error=error_kind(error))
        return ComputeExpected(stdout=b"0\n", report="")
    expected = oracle(workload, source_contents=source_contents)
    if expected.error is not None:
        return ComputeExpected(error=expected.error)
    assert expected.report is not None
    return ComputeExpected(stdout=f"{len(expected.report) * rounds}\n".encode(), report=expected.report)


def captured_error(stderr: bytes) -> str | None:
    text = stderr.decode("utf-8", errors="replace")
    matches = re.findall(
        r"(JSONDecodeError|UnicodeDecodeError|FileNotFoundError|IsADirectoryError|"
        r"NotADirectoryError|PermissionError|AssertionError|ValueError|KeyError|OSError):",
        text,
    )
    if not matches:
        return None
    return "ValueError" if matches[-1] == "JSONDecodeError" else matches[-1]


def check_result(
    completed: subprocess.CompletedProcess[bytes], expected: Expected, *,
    output: Path | None = None, previous_output: bytes | None = None,
) -> dict:
    if expected.error is None:
        assert expected.report is not None
        encoded = expected.report.encode("utf-8")
        assert completed.returncode == 0, completed.stderr.decode(errors="replace")
        assert completed.stderr == b"", completed.stderr
        assert completed.stdout == (encoded + b"\n" if output is None else b""), "stdout differs"
        if output is not None:
            assert output.read_bytes() == encoded, "report bytes differ"
    else:
        assert completed.returncode != 0, "expected failure"
        assert completed.stdout == b"", "failed command wrote stdout"
        assert captured_error(completed.stderr) == expected.error, completed.stderr.decode(errors="replace")
        if output is not None:
            if previous_output is None:
                assert not output.exists(), "failed command created a report"
            else:
                assert output.read_bytes() == previous_output, "failed command changed the previous report"
    return {
        "returncode": completed.returncode,
        "stdout_sha256": digest(completed.stdout), "stdout_bytes": len(completed.stdout),
        "stderr_sha256": digest(completed.stderr), "stderr_bytes": len(completed.stderr),
        "error": expected.error,
        "report_sha256": None if output is None or not output.is_file() else digest(output.read_bytes()),
    }


def check_compute_result(
    completed: subprocess.CompletedProcess[bytes], expected: ComputeExpected, *,
    output: Path | None = None, previous_output: bytes | None = None,
) -> dict:
    if expected.error is None:
        assert completed.returncode == 0, completed.stderr.decode(errors="replace")
        assert completed.stdout == expected.stdout, (completed.stdout, expected.stdout)
        assert completed.stderr == b"", completed.stderr
        if output is not None:
            assert expected.report is not None
            assert output.read_bytes() == expected.report.encode(), "final compute report differs"
    else:
        assert completed.returncode != 0 and completed.stdout == b""
        assert captured_error(completed.stderr) == expected.error, completed.stderr.decode(errors="replace")
        if output is not None:
            if previous_output is None:
                assert not output.exists()
            else:
                assert output.read_bytes() == previous_output, "failed compute changed previous output"
    return {
        "returncode": completed.returncode,
        "stdout_sha256": digest(completed.stdout), "stdout_bytes": len(completed.stdout),
        "stderr_sha256": digest(completed.stderr), "stderr_bytes": len(completed.stderr),
        "error": expected.error,
        "report_sha256": None if output is None or not output.is_file() else digest(output.read_bytes()),
    }


def materialize_real(
    destination: Path, prepared_directory: Path, *, source_root: Path | None = None,
    format: str = "github", force_summary_table: bool = True,
) -> Workload:
    """Copy selected source bytes and unchanged captures; rebase only identities."""
    preparation = json.loads((prepared_directory / "preparation.json").read_text())
    manifest = json.loads((prepared_directory / "manifest.json").read_text())
    assert preparation["guards_passed"]
    assert digest((prepared_directory / "manifest.json").read_bytes()) == preparation["manifest_sha256"]
    old_root = Path(preparation["suite_directory"])
    source_root = old_root if source_root is None else source_root
    destination.mkdir(parents=True, exist_ok=False)
    new_root = destination.resolve() / "sources"
    for filename in manifest["test_files"]:
        relative = Path(filename).relative_to(old_root)
        data = (source_root / relative).read_bytes()
        assert digest(data) == preparation["source_sha256"][relative.as_posix()]
        target = new_root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    manifest["test_files"] = [
        str(new_root / Path(filename).relative_to(old_root)) for filename in manifest["test_files"]
    ]
    for alias in manifest["diagnostic_paths"]:
        alias["path"] = str(new_root / Path(alias["path"]).relative_to(old_root))
    for role in ("old", "new"):
        data = (prepared_directory / f"{role}.json").read_bytes()
        assert digest(data) == preparation["captures"][role]["stdout_sha256"]
        (destination / f"{role}.json").write_bytes(data)
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return Workload(destination / "manifest.json", destination / "old.json",
                    destination / "new.json", format, force_summary_table)


def workload_metadata(workload: Workload) -> dict:
    manifest = json.loads(workload.manifest.read_text())
    return {
        "suite_revision": manifest["suite_revision"], "format": workload.format,
        "force_summary_table": workload.force_summary_table,
        "test_files": len(manifest["test_files"]),
        "test_source_bytes": sum(Path(path).stat().st_size for path in manifest["test_files"]),
        "old_diagnostics": len(json.loads(workload.old.read_text())),
        "new_diagnostics": len(json.loads(workload.new.read_text())),
        "old_json_bytes": workload.old.stat().st_size, "new_json_bytes": workload.new.stat().st_size,
        "manifest_sha256": digest(workload.manifest.read_bytes()),
        "old_sha256": digest(workload.old.read_bytes()), "new_sha256": digest(workload.new.read_bytes()),
        "source_sha256": {path: digest(Path(path).read_bytes()) for path in manifest["test_files"]},
    }


def diagnostic(
    filename: str, line: int, *, column: int = 1, code: str = "rule",
    message: str = "rule: diagnostic", severity: str = "major", end: int | None = None,
) -> dict:
    return {
        "check_name": code, "description": message, "severity": severity,
        "location": {"path": "raw/" + filename, "positions": {
            "begin": {"line": line, "column": column},
            "end": {"line": line if end is None else end, "column": column + 1},
        }},
    }


@dataclass(frozen=True)
class Case:
    name: str
    sources: dict[str, str]
    old: list[dict] | str
    new: list[dict] | str
    error: str | None = None
    mutation: str = ""
    codon: bool = True


def cases() -> list[Case]:
    anchor = diagnostic("alpha.py", 1)
    source = "\n".join([
        "anchor()  # E: anchor", "added()  # E", "removed()  # E",
        "unexpected()", "removed_unexpected()", "changed()  # E",
        "optional_added()  # E?", "optional_removed()  # E?",
        "optional_changed()  # E?", "tagged()  # E[one]", "tagged()  # E[one]",
        "multiple()  # E[many+]", "multiple()  # E[many+]",
        "optional_multiple()  # E?[opt+]", "optional_multiple()  # E?[opt+]",
        "mixed()  # E[mixed]", "mixed()  # E?[mixed]", "",
    ])
    old = [anchor, *[diagnostic("alpha.py", line) for line in (3, 5, 6, 8, 9, 10, 12, 13, 16)]]
    new = [anchor, *[diagnostic("alpha.py", line) for line in (2, 4, 7, 10, 11, 12, 14, 15, 17)],
           diagnostic("alpha.py", 6, message="rule: changed rule: text"),
           diagnostic("alpha.py", 9, message="rule: optional change"),
           diagnostic("quiet.py", 1)]
    unicode_source = (
        "anchor() # E\r\n# E: ignored comment\r\n"
        "é()  #\u001cE?: 😀\r\nfallback() # E[]: empty tag\r\n"
        "fallback() # E[bad++]: invalid tag\r\n"
        "tag() # E[α tag+]: description\r\n"
        "tag() # E[α tag+]\r\n"
    )
    same = [anchor, diagnostic("alpha.py", 1, code="other", message="other: second")]
    reordered = copy.deepcopy(list(reversed(same)))
    for item in reordered:
        item["location"]["positions"]["end"]["line"] += 100
    result = [
        Case("unchanged", {"alpha.py": "x() # E\n", "quiet.py": ""}, same, reordered),
        Case("all-classifications", {"alpha.py": source, "quiet.py": "quiet()\n", "empty.py": ""}, old, new),
        Case("unicode-and-fallback", {"alpha.py": unicode_source}, [anchor],
             [anchor, *[diagnostic("alpha.py", line, message="rule: é 😀\nnext") for line in (3, 4, 5, 6)]]),
        Case("same-basename-tags", {"left/same.py": "x() # E[tag]\ny() # E?\n",
                                   "right/same.py": "x() # E[tag]\ny() # E?\n"},
             [diagnostic("left/same.py", 1)],
             [diagnostic("right/same.py", 1), diagnostic("left/same.py", 2)]),
        Case("filtered-and-extra-fields", {"alpha.py": "x() # E\n"},
             [anchor, diagnostic("unmapped.py", 2, severity="minor")],
             [{**anchor, "future_metadata": {"nested": [1, 2, 3]}},
              diagnostic("unmapped.py", 3, severity="other")]),
        Case("wide-integer-positions", {"alpha.py": "x() # E\n"},
             [anchor], [anchor, diagnostic("alpha.py", 2**100, column=-(2**90))], codon=False),
        Case("no-annotations", {"alpha.py": "# E: comment only\nx()\n"}, [], [],
             error="AssertionError"),
        Case("no-new-true-positive", {"alpha.py": "x() # E\n"}, [anchor], [],
             error="AssertionError"),
        Case("missing-path-alias", {"alpha.py": "x() # E\n"}, [anchor],
             [anchor, diagnostic("unmapped.py", 2)], error="KeyError"),
        Case("unknown-test-file", {"alpha.py": "x() # E\n"}, [anchor],
             [anchor, diagnostic("unmapped.py", 2)], error="KeyError", mutation="extra-alias"),
        Case("malformed-json", {"alpha.py": "x() # E\n"}, "[{", [anchor],
             error="ValueError"),
        Case("duplicate-path", {"alpha.py": "x() # E\n"}, [anchor], [anchor],
             error="ValueError", mutation="duplicate-path"),
        Case("duplicate-alias", {"alpha.py": "x() # E\n"}, [anchor], [anchor],
             error="ValueError", mutation="duplicate-alias"),
        Case("missing-source", {"alpha.py": "x() # E\n"}, [anchor], [anchor],
             error="FileNotFoundError", mutation="missing-source"),
    ]
    return result


def materialize_case(case: Case, directory: Path, *, format: str = "github",
                     force_summary_table: bool = True) -> Workload:
    directory.mkdir(parents=True, exist_ok=False)
    files = []
    aliases = []
    for name, source in case.sources.items():
        path = directory.resolve() / "sources" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(source.encode())
        files.append(str(path))
        aliases.append({"raw": "raw/" + name, "path": str(path)})
    manifest = {"suite_revision": REVISION, "test_files": files, "diagnostic_paths": aliases}
    if case.mutation == "extra-alias":
        aliases.append({"raw": "raw/unmapped.py", "path": str(directory.resolve() / "unmapped.py")})
    elif case.mutation == "duplicate-path":
        files.append(files[0])
    elif case.mutation == "duplicate-alias":
        aliases.append(aliases[0])
    elif case.mutation == "missing-source":
        Path(files[0]).unlink()
    (directory / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
    for role, records in (("old", case.old), ("new", case.new)):
        text = records if isinstance(records, str) else json.dumps(records, ensure_ascii=False)
        (directory / f"{role}.json").write_text(text, encoding="utf-8")
    return Workload(directory / "manifest.json", directory / "old.json", directory / "new.json",
                    format, force_summary_table)


def parser_differentials(count: int = 30_000) -> int:
    sys.path[:0] = [str(ROOT / "examples"), str(ROOT / "stdlib")]
    from conformance_report.parse import scan_annotation
    generator = random.Random(7441)
    tokens = ["", "#", "# E", "#E", " E", "\tE", "x", "?", "[", "]", "+", "tag",
              ":", " ", "\t", "\x1f", "\u2003", "é", "😀", "E", "code()", "\\", '"']
    pattern = upstream().CONFORMANCE_ERROR_PATTERN
    for _ in range(count):
        line = "".join(generator.choice(tokens) for _ in range(generator.randrange(1, 18)))
        match = pattern.search(line)
        expected = None if match is None else (
            match.start(), match.end(), bool(match["optional"]), match["tag"],
            bool(match["multi"]), match["description"] or "Missing",
        )
        result = scan_annotation(line)
        actual = None if result is None else (
            result.start, result.end, result.optional, result.tag, result.multi, result.description,
        )
        assert actual == expected, (line, expected, actual)
    return count


def report_differentials(directory: Path, count: int = 100) -> int:
    sys.path[:0] = [str(ROOT / "examples"), str(ROOT / "stdlib")]
    from conformance_report.app import render_report
    from conformance_report.parse import read_inputs
    from conformance_report import original
    generator = random.Random(4263)
    source = {
        "alpha.py": "anchor() # E\nx() # E[tag]\ny() # E[tag]\nx() # E[multi+]\n"
                    "y() # E[multi+]\nx() # E?\nx() # E?[opt+]\ny() # E?[opt+]\nx()\n",
        "βeta.py": "# E: comment only\nx() # E?: unicode\nx() # E[bad++]\n"
                   "x() # E[mixed]\nx() # E?[mixed]\nx()\n",
        "quiet.py": "quiet()\n",
    }
    checks = 0
    for index in range(count):
        old, new = [], [diagnostic("alpha.py", 1)]
        for records in (old, new):
            for _ in range(generator.randrange(35)):
                records.append(diagnostic(
                    generator.choice(list(source)), generator.randrange(1, 10),
                    column=generator.randrange(4), code=generator.choice(["a", "b", "c"]),
                    message=generator.choice(["a: first", "b: é 😀\nnext", "c: c: duplicate"]),
                    severity=generator.choice(["major", "major", "minor"]),
                ))
        if index % 10 == 0:
            old = list(new)
            generator.shuffle(old)
        case = Case(f"random-{index}", source, old, new)
        workload = materialize_case(case, directory / case.name)
        inputs = read_inputs(str(workload.manifest), str(workload.old), str(workload.new))
        original_inputs = original.read_inputs(str(workload.manifest), str(workload.old), str(workload.new))
        for format in ("github", "diff"):
            for force in (False, True):
                selected = Workload(workload.manifest, workload.old, workload.new, format, force)
                expected = oracle(selected)
                assert expected.error is None, expected
                assert render_report(inputs, format, force) == expected.report, (index, format, force)
                assert original.render_report(original.load_upstream(), original_inputs, format, force) == expected.report
                checks += 1
    return checks


def source_hashes() -> dict[str, str]:
    files = [Path(__file__), *APP.rglob("*"), *(ROOT / "stdlib" / "relar").rglob("*.py")]
    return {str(path): digest(path.read_bytes()) for path in sorted(set(files))
            if path.is_file() and "__pycache__" not in path.parts}


def input_tree(directory: Path) -> dict[str, dict]:
    result = {}
    for path in sorted(directory.rglob("*")):
        name = path.relative_to(directory).as_posix()
        if path.is_symlink():
            result[name] = {"kind": "symlink", "target": str(path.readlink())}
        elif path.is_dir():
            result[name] = {"kind": "directory"}
        else:
            result[name] = {"kind": "file", "sha256": digest(path.read_bytes())}
    return result


def validate(args: argparse.Namespace) -> dict:
    directory = args.retain_dir.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    home = directory / "home"
    home.mkdir()
    env = environment(home)
    if args.asan:
        env["ASAN_OPTIONS"] = "detect_leaks=1:halt_on_error=1"
    engines = {
        "original": original_command(args.python),
        "python-port": python_command(args.python),
    }
    compute_engines = {
        "original": original_compute_command(args.python),
        "python-port": python_compute_command(args.python),
    }
    if args.binary:
        engines["relar"] = [str(args.binary.resolve())]
    if args.compute_binary:
        compute_engines["relar"] = [str(args.compute_binary.resolve())]
    if args.rust_binary:
        engines["rust"] = [str(args.rust_binary.resolve())]
        compute_engines["rust"] = [str(args.rust_binary.resolve()), "--compute"]
    if args.codon_binary:
        engines["codon"] = [str(args.codon_binary.resolve())]
    if args.codon_compute_binary:
        compute_engines["codon"] = [str(args.codon_compute_binary.resolve())]
    guarded_sources = source_hashes()
    executables = {Path(command[0]).resolve() for command in [*engines.values(), *compute_engines.values()]}
    guarded_binaries = {str(path): digest(path.read_bytes()) for path in sorted(executables)}
    record = {
        "schema": 1, "status": "failed", "source_sha256": guarded_sources,
        "binary_sha256": guarded_binaries, "environment": env,
        "application_commands": engines, "compute_commands": compute_engines,
        "checks": [], "cases": [], "parser_differentials": 0, "report_differentials": 0,
        "comparison_exclusions": {"codon": ["integers outside the signed 64-bit range"]},
    }
    counter = 0

    def run(command, label):
        nonlocal counter
        counter += 1
        completed = subprocess.run(command, env=env, capture_output=True, timeout=60)
        prefix = directory / "captures" / f"{counter:04d}-{label}"
        prefix.parent.mkdir(exist_ok=True)
        prefix.with_suffix(".stdout").write_bytes(completed.stdout)
        prefix.with_suffix(".stderr").write_bytes(completed.stderr)
        return completed, prefix

    def check_workload(name, workload, *, codon=True, declared_error=None, compute=True):
        expected = oracle(workload)
        assert expected.error == declared_error, (name, expected)
        input_files = [workload.manifest, workload.old, workload.new]
        manifest = json.loads(workload.manifest.read_text())
        input_files.extend(Path(path) for path in manifest["test_files"])
        before = {str(path): digest(path.read_bytes()) if path.is_file() else None for path in input_files}
        before_tree = input_tree(workload.manifest.parent)
        record["cases"].append({
            "name": name, "arguments": workload.arguments(), "input_sha256": before,
            "input_directory": str(workload.manifest.parent), "input_tree": before_tree,
            "expected_error": expected.error,
            "expected_report_sha256": None if expected.report is None else digest(expected.report.encode()),
        })
        destination = directory / "outputs" / name
        destination.mkdir(parents=True)
        for engine, command in engines.items():
            if engine == "codon" and not codon:
                continue
            output = destination / f"{engine}.md"
            output.write_bytes(SENTINEL)
            selected = [*command, *workload.arguments(), "--output", str(output)]
            completed, prefix = run(selected, f"{name}-{engine}")
            checked = check_result(completed, expected, output=output, previous_output=SENTINEL)
            assert input_tree(workload.manifest.parent) == before_tree, (name, engine, "input tree changed")
            record["checks"].append({"kind": "application", "case": name, "engine": engine,
                                     "command": selected, "capture_prefix": str(prefix),
                                     "output": str(output), **checked})
        if compute:
            for rounds in (0, 3):
                checksum = compute_oracle(workload, rounds)
                for engine, command in compute_engines.items():
                    if engine == "codon" and not codon:
                        continue
                    output = destination / f"compute-{engine}-{rounds}.md"
                    output.write_bytes(SENTINEL)
                    selected = [*command, *workload.arguments(), "--rounds", str(rounds),
                                "--output", str(output)]
                    completed, prefix = run(selected, f"{name}-{engine}-compute-{rounds}")
                    checked = check_compute_result(completed, checksum, output=output,
                                                   previous_output=SENTINEL)
                    assert input_tree(workload.manifest.parent) == before_tree, (name, engine, "input tree changed")
                    record["checks"].append({"kind": "compute", "case": name, "engine": engine,
                                             "rounds": rounds, "command": selected,
                                             "capture_prefix": str(prefix), "output": str(output),
                                             **checked})
        after = {str(path): digest(path.read_bytes()) if path.is_file() else None for path in input_files}
        assert before == after, (name, "input files changed")

    try:
        record["parser_differentials"] = parser_differentials()
        record["report_differentials"] = report_differentials(directory / "differentials")
        for case in cases():
            base = materialize_case(case, directory / "fixtures" / case.name)
            modes = [("github", False)] if case.error else [
                (format, force) for format in ("github", "diff") for force in (False, True)
            ]
            for format, force in modes:
                workload = Workload(base.manifest, base.old, base.new, format, force)
                name = f"{case.name}-{format}-{'forced' if force else 'default'}"
                check_workload(name, workload, codon=case.codon, declared_error=case.error,
                               compute=force or case.error is not None)
        for index, prepared in enumerate(args.prepared):
            base = materialize_real(directory / "real" / str(index), prepared,
                                    source_root=args.source_root)
            for format in ("github", "diff"):
                workload = Workload(base.manifest, base.old, base.new, format, True)
                check_workload(f"real-{index}-{format}", workload)
        # Cover print's single added newline independently of file-output mode.
        base = materialize_case(cases()[1], directory / "stdout-fixture", format="diff")
        expected = oracle(base)
        for engine, command in engines.items():
            selected = [*command, *base.arguments()]
            completed, prefix = run(selected, f"stdout-{engine}")
            checked = check_result(completed, expected)
            record["checks"].append({"kind": "stdout", "case": "all-classifications",
                                     "engine": engine, "command": selected,
                                     "capture_prefix": str(prefix), **checked})
        # Rendering succeeds before a destination open fails. Existing contents
        # of that directory must survive, and the failed command stays quiet.
        bad_output = directory / "output-is-directory"
        bad_output.mkdir()
        (bad_output / "sentinel").write_bytes(SENTINEL)
        for engine, command in engines.items():
            selected = [*command, *base.arguments(), "--output", str(bad_output)]
            completed, prefix = run(selected, f"output-directory-{engine}")
            checked = check_result(completed, Expected(error="IsADirectoryError"))
            assert sorted(path.name for path in bad_output.iterdir()) == ["sentinel"]
            assert (bad_output / "sentinel").read_bytes() == SENTINEL
            record["checks"].append({"kind": "output-error", "case": "output-is-directory",
                                     "engine": engine, "command": selected,
                                     "capture_prefix": str(prefix), **checked})
        for engine, command in compute_engines.items():
            selected = [*command, *base.arguments(), "--rounds", "3", "--output", str(bad_output)]
            completed, prefix = run(selected, f"compute-output-directory-{engine}")
            checked = check_compute_result(completed, ComputeExpected(error="IsADirectoryError"))
            assert sorted(path.name for path in bad_output.iterdir()) == ["sentinel"]
            assert (bad_output / "sentinel").read_bytes() == SENTINEL
            record["checks"].append({"kind": "output-error", "case": "compute-output-is-directory",
                                     "engine": engine, "command": selected,
                                     "capture_prefix": str(prefix), **checked})
        assert guarded_sources == source_hashes(), "application or oracle source changed"
        assert guarded_binaries == {str(path): digest(path.read_bytes()) for path in sorted(executables)}
        record["guards_passed"] = True
        record["status"] = "passed"
        record["counts"] = {
            kind: sum(item["kind"] == kind for item in record["checks"])
            for kind in ("application", "compute", "stdout", "output-error")
        }
    finally:
        (directory / "record.json").write_text(json.dumps(record, indent=2) + "\n")
    return record


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", type=Path, default=Path(sys.executable))
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--compute-binary", type=Path)
    parser.add_argument("--rust-binary", type=Path)
    parser.add_argument("--codon-binary", type=Path)
    parser.add_argument("--codon-compute-binary", type=Path)
    parser.add_argument("--prepared", type=Path, action="append", default=[])
    parser.add_argument("--source-root", type=Path)
    parser.add_argument("--retain-dir", type=Path, required=True)
    parser.add_argument("--asan", action="store_true")
    args = parser.parse_args()
    result = validate(args)
    print(json.dumps({key: result[key] for key in (
        "status", "counts", "parser_differentials", "report_differentials", "guards_passed",
    )}, indent=2))


if __name__ == "__main__":
    main()

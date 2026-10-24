"""Independent filesystem, bounded-parser, and graph oracle for manifest_planner."""

import argparse
import base64
import hashlib
import json
import posixpath
import runpy
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_ROOT = ROOT / "examples"
PACKAGE = PACKAGE_ROOT / "manifest_planner"
ENTRY = PACKAGE / "__main__.py"
batch = SimpleNamespace(**runpy.run_path(str(ROOT / "benchmarks/batch_validate.py")))
BOOTSTRAP = (
    "import runpy, sys; sys.path.insert(0, sys.argv[1]); "
    "sys.argv = ['manifest_planner', *sys.argv[2:]]; "
    "runpy.run_module('manifest_planner', run_name='__main__', alter_sys=True)"
)


@dataclass(frozen=True)
class Case:
    label: str
    files: dict[str, bytes]
    arguments: tuple[str, ...] = ("manifest.txt", "report.txt")
    directories: tuple[str, ...] = ()
    hardlinks: dict[str, str] = field(default_factory=dict)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def package_sources() -> list[Path]:
    return sorted((*PACKAGE.rglob("*.py"), *batch.package_sources()))


def python_command(python: str) -> list[str]:
    return [python, "-I", "-c", BOOTSTRAP, str(PACKAGE_ROOT)]


def encoded_files(files: dict[str, bytes]) -> dict[str, dict]:
    return {
        name: {
            "base64": base64.b64encode(data).decode(),
            "sha256": digest(data),
            "bytes": len(data),
        }
        for name, data in sorted(files.items())
    }


def case_record(case: Case) -> dict:
    return {
        "label": case.label,
        "arguments": list(case.arguments),
        "files": encoded_files(case.files),
        "directories": list(case.directories),
        "hardlinks": case.hardlinks,
    }


def case_from_record(record: dict) -> Case:
    files = {}
    for name, item in record["files"].items():
        data = base64.b64decode(item["base64"], validate=True)
        if digest(data) != item["sha256"] or len(data) != item["bytes"]:
            raise ValueError(f"captured file differs: {name}")
        files[name] = data
    return Case(
        record["label"],
        files,
        tuple(record["arguments"]),
        tuple(record["directories"]),
        record["hardlinks"],
    )


def directory_names(case: Case) -> set[str]:
    result = {".", *case.directories}
    for name in (*case.files, *case.hardlinks, *case.directories):
        parent = posixpath.dirname(name)
        while parent:
            result.add(parent)
            parent = posixpath.dirname(parent)
    return result


def oracle(case: Case) -> dict:
    """Simulate sequential opens/inodes, then invoke only independent oracles."""
    nodes = {name: [data] for name, data in case.files.items()}
    for name, target in case.hardlinks.items():
        nodes[name] = nodes[target]
    directories = directory_names(case)
    completed = invalid = cleaned = 0
    requests = []
    category = message = None

    def path_name(path: str) -> str:
        if "\0" in path:
            raise ValueError("invalid payload path")
        if path == "":
            raise FileNotFoundError
        prefix = []
        for component in path.split("/")[:-1]:
            if component == "..":
                if not prefix:
                    raise FileNotFoundError
                prefix.pop()
            elif component not in ("", "."):
                prefix.append(component)
            current = "/".join(prefix) or "."
            if current in nodes:
                raise NotADirectoryError
            if current not in directories:
                raise FileNotFoundError
        normalized = posixpath.normpath(path)
        parent = posixpath.dirname(normalized)
        if parent in nodes:
            raise NotADirectoryError
        if parent and parent not in directories:
            raise FileNotFoundError
        if normalized in directories:
            raise IsADirectoryError
        return normalized

    def read(path: str) -> str:
        name = path_name(path)
        if name not in nodes:
            raise FileNotFoundError
        return nodes[name][0].decode("utf-8")

    try:
        if len(case.arguments) != 2:
            raise ValueError("usage: manifest_planner MANIFEST REPORT")
        paths = read(case.arguments[0]).splitlines()
        report_name = path_name(case.arguments[1])
        if report_name not in nodes:
            nodes[report_name] = [b""]
        report = nodes[report_name]
        report[0] = b""

        def write(text: str) -> None:
            report[0] += text.encode("utf-8")

        for request_id, path in enumerate(paths):
            # The real program flushes this prefix before opening the payload.
            write(f"REQUEST\n{request_id}\n")
            error_kind = error_message = None
            try:
                text = read(path)
                tasks, changes = batch.parse_payload(tuple(text.splitlines()))
            except UnicodeDecodeError:
                error_kind, error_message = "UnicodeDecodeError", "invalid UTF-8"
            except OSError:
                error_kind, error_message = "OSError", "cannot read payload"
            except ValueError:
                error_kind, error_message = "ValueError", "invalid payload path"
            except batch.PayloadError as error:
                error_kind, error_message = error.kind, error.message
            if error_kind is not None:
                write(f"INVALID\n{error_kind}\n{error_message}\n")
                invalid += 1
                requests.append(
                    {"id": request_id, "kind": "invalid", "error": error_kind}
                )
            else:
                result = batch.incremental.oracle(tasks, changes)
                write(result["output"])
                completed += 1
                requests.append(
                    {
                        "id": request_id,
                        "kind": result["kind"],
                        "targets": len(tasks),
                        "affected": len(result["order"]),
                    }
                )
            cleaned += 1
            write(f"END\n{request_id}\n")
        write(
            f"SUMMARY\nCOMPLETED\n{completed}\nINVALID\n{invalid}\nCLEANED\n{cleaned}\n"
        )
    except (OSError, ValueError) as error:
        category, message = type(error).__name__, str(error)
    return {
        "returncode": int(category is not None),
        "error_category": category,
        "usage_message": message if message and message.startswith("usage:") else None,
        "stdout": "",
        "files": {name: value[0] for name, value in nodes.items()},
        "directories": sorted(directories - {"."}),
        "requests": requests,
        "completed": completed,
        "invalid": invalid,
        "cleaned": cleaned,
    }


def materialize(case: Case, directory: Path) -> None:
    """Create only safe relative fixtures; requested input paths may be invalid."""
    for name in (*case.files, *case.hardlinks, *case.directories):
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or "\0" in name:
            raise ValueError("fixture creation must stay inside its isolated directory")
    for name in sorted(directory_names(case)):
        (directory / name).mkdir(parents=True, exist_ok=True)
    for name, data in case.files.items():
        (directory / name).write_bytes(data)
    for name, target in case.hardlinks.items():
        (directory / name).hardlink_to(directory / target)


def snapshot(directory: Path) -> tuple[dict[str, bytes], list[str]]:
    files = {}
    directories = []
    for path in sorted(directory.rglob("*")):
        name = path.relative_to(directory).as_posix()
        if path.is_symlink():
            raise AssertionError(f"unexpected symlink: {name}")
        if path.is_dir():
            directories.append(name)
        elif path.is_file():
            files[name] = path.read_bytes()
        else:
            raise AssertionError(f"unexpected filesystem entry: {name}")
    return files, directories


def check_result(case: Case, expected: dict, process, directory: Path) -> dict:
    files, directories = snapshot(directory)
    stdout = process.stdout.decode("utf-8")
    stderr = process.stderr.decode("utf-8")
    category = (
        stderr.strip().splitlines()[-1].split(":", 1)[0] if stderr.strip() else None
    )
    if (
        process.returncode != expected["returncode"]
        or stdout != expected["stdout"]
        or category != expected["error_category"]
        or files != expected["files"]
        or directories != expected["directories"]
        or (expected["usage_message"] and expected["usage_message"] not in stderr)
    ):
        raise AssertionError(
            f"{case.label}: status/output/filesystem mismatch: {process.returncode}, {category}; stderr={stderr!r}"
        )
    return {
        "returncode": process.returncode,
        "stdout_sha256": digest(process.stdout),
        "stdout_bytes": len(process.stdout),
        "stderr": stderr,
        "files": {
            name: {"sha256": digest(data), "bytes": len(data)}
            for name, data in sorted(files.items())
        },
        "directories": directories,
    }


def cases() -> list[Case]:
    valid = b"1\nrecovered 7\n1\nrecovered\n"
    result = [Case("empty-manifest", {"manifest.txt": b""})]
    for case in batch.incremental.cases():
        text = case.stdin_override or batch.incremental.input_text(
            case.tasks, case.changes
        )
        result.append(
            Case(
                "graph-" + case.label,
                {"manifest.txt": b"payload.txt\n", "payload.txt": text.encode()},
            )
        )
    for label, text, _, _ in batch.incremental.parser_cases():
        result.append(
            Case(
                "recover-" + label,
                {
                    "manifest.txt": b"bad.txt\ngood.txt\n",
                    "bad.txt": text.encode(),
                    "good.txt": valid,
                },
            )
        )
    result.extend(
        [
            Case(
                "spaces-and-unicode-path",
                {
                    "manifest.txt": " spaced payload café.txt \n".encode(),
                    " spaced payload café.txt ": valid,
                },
            ),
            Case(
                "missing-directory-and-nul",
                {
                    "manifest.txt": b"missing\nfolder\nbad\0path\ngood.txt\n",
                    "good.txt": valid,
                },
                directories=("folder",),
            ),
            Case(
                "empty-path-is-request",
                {"manifest.txt": b"\ngood.txt\n\n", "good.txt": valid},
            ),
            Case(
                "invalid-utf8-then-valid",
                {
                    "manifest.txt": b"bad.txt\ngood.txt\n",
                    "bad.txt": b"\xff\xfe",
                    "good.txt": valid,
                },
            ),
            Case(
                "utf8-bom-is-data",
                {
                    "manifest.txt": b"bad.txt\ngood.txt\n",
                    "bad.txt": b"\xef\xbb\xbf" + valid,
                    "good.txt": valid,
                },
            ),
            Case(
                "all-line-boundaries",
                {
                    "manifest.txt": "a\r\nb\rc\nd\ve\ff\x1cg\x1dh\x1ei\x85j\u2028k\u2029".encode(),
                    **{
                        name: valid.replace(b"\n", separator.encode())
                        for name, separator in zip(
                            "abcdefghijk",
                            [
                                "\n",
                                "\r\n",
                                "\r",
                                "\v",
                                "\f",
                                "\x1c",
                                "\x1d",
                                "\x1e",
                                "\x85",
                                "\u2028",
                                "\u2029",
                            ],
                        )
                    },
                },
            ),
            Case(
                "unit-separator-is-not-line-boundary",
                {"manifest.txt": b"a\x1fb\n", "a\x1fb": valid},
            ),
            Case(
                "repeated-entry",
                {"manifest.txt": b"good.txt\ngood.txt\ngood.txt", "good.txt": valid},
            ),
            Case(
                "normalized-relative-path",
                {"manifest.txt": b"nested/../good.txt\n", "good.txt": valid},
                directories=("nested",),
            ),
            Case(
                "path-components-must-exist",
                {
                    "manifest.txt": b"missing/../good.txt\nfile/../good.txt\ngood.txt/\ngood.txt\n",
                    "good.txt": valid,
                    "file": b"ordinary file",
                },
            ),
            Case(
                "report-is-manifest",
                {"manifest.txt": b"good.txt\n", "good.txt": valid},
                ("manifest.txt", "manifest.txt"),
            ),
            Case(
                "report-is-payload",
                {
                    "manifest.txt": b"report.txt\ngood.txt\n",
                    "report.txt": valid,
                    "good.txt": valid,
                },
            ),
            Case(
                "report-hardlink-payload",
                {
                    "manifest.txt": b"alias.txt\ngood.txt\n",
                    "report.txt": valid,
                    "good.txt": valid,
                },
                hardlinks={"alias.txt": "report.txt"},
            ),
            Case("missing-manifest-preserves-report", {"report.txt": b"untouched\n"}),
            Case(
                "directory-manifest-preserves-report",
                {"report.txt": b"untouched\n"},
                directories=("manifest.txt",),
            ),
            Case(
                "invalid-utf8-manifest-preserves-report",
                {"manifest.txt": b"\xff", "report.txt": b"untouched\n"},
            ),
            Case(
                "report-is-directory",
                {"manifest.txt": b"good.txt\n", "good.txt": valid},
                directories=("report.txt",),
            ),
            Case(
                "missing-report-parent",
                {"manifest.txt": b""},
                ("manifest.txt", "absent/report.txt"),
            ),
            Case(
                "file-report-parent",
                {"manifest.txt": b"", "parent": b"file"},
                ("manifest.txt", "parent/report.txt"),
            ),
            Case("no-arguments", {}, ()),
            Case("one-argument", {"manifest.txt": b""}, ("manifest.txt",)),
            Case(
                "extra-argument",
                {"manifest.txt": b"", "report.txt": b"untouched"},
                ("manifest.txt", "report.txt", "extra"),
            ),
            Case(
                "repeated-failures-then-valid",
                {
                    "manifest.txt": ("bad.txt\nmissing\n" * 32 + "good.txt\n").encode(),
                    "bad.txt": b"25\n" + b"ok 1\n" * 24 + b"bad x\n0\n",
                    "good.txt": valid,
                },
            ),
        ]
    )
    return result


def validate_devices(commands: dict[str, list[str]], timeout: float) -> dict:
    """Check the application recovery boundary with a real Linux failing device."""
    if sys.platform != "linux" or not Path("/dev/full").is_char_device():
        return {"cases": 0, "skipped": "/dev/full is unavailable", "checks": []}
    case = Case(
        "report-device-write-is-terminal",
        {"manifest.txt": b"good.txt\n", "good.txt": b"1\nready 7\n1\nready\n"},
        ("manifest.txt", "/dev/full"),
    )
    expected = {
        "returncode": 1,
        "error_category": "OSError",
        "usage_message": None,
        "stdout": "",
        "files": case.files,
        "directories": [],
    }
    runs = {}
    for name, command in commands.items():
        with tempfile.TemporaryDirectory(prefix="imre-manifest-device-") as temporary:
            directory = Path(temporary)
            materialize(case, directory)
            process = subprocess.run(
                [*command, *case.arguments],
                input=b"",
                capture_output=True,
                cwd=directory,
                timeout=timeout,
                check=False,
            )
            runs[name] = check_result(case, expected, process, directory)
    return {
        "cases": 1,
        "passed_by_engine": {name: 1 for name in commands},
        "checks": [
            {
                **case_record(case),
                "expected": {
                    **{key: value for key, value in expected.items() if key != "files"},
                    "files": encoded_files(expected["files"]),
                },
                "results": runs,
            }
        ],
    }


def validate_commands(commands: dict[str, list[str]], timeout: float = 120) -> dict:
    records = []
    passed = {name: 0 for name in commands}
    completed = invalid = terminal = 0
    for case in cases():
        expected = oracle(case)
        completed += expected["completed"]
        invalid += expected["invalid"]
        terminal += expected["returncode"] != 0
        runs = {}
        for name, command in commands.items():
            with tempfile.TemporaryDirectory(prefix="imre-manifest-") as temporary:
                directory = Path(temporary)
                materialize(case, directory)
                process = subprocess.run(
                    [*command, *case.arguments],
                    input=b"",
                    capture_output=True,
                    cwd=directory,
                    timeout=timeout,
                    check=False,
                )
                runs[name] = check_result(case, expected, process, directory)
            passed[name] += 1
        records.append(
            {
                **case_record(case),
                "expected": {
                    **{key: value for key, value in expected.items() if key != "files"},
                    "files": encoded_files(expected["files"]),
                },
                "results": runs,
            }
        )
    return {
        "cases": len(records),
        "passed_by_engine": passed,
        "completed_requests": completed,
        "invalid_requests": invalid,
        "terminal_cases": terminal,
        "oracle": "Independent in-memory inode/open model, full directory snapshots, bounded iterator parser, and graphlib/fixed-point/heapq scheduling oracle; compiled packages never imported",
        "checks": records,
        "linux_device_checks": validate_devices(commands, timeout),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--rust-bigint", type=Path)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    commands = {"cpython": python_command(args.python)}
    for name, path in (("imre", args.binary), ("rust_bigint", args.rust_bigint)):
        if path:
            commands[name] = [str(path.resolve())]
    result = validate_commands(commands, args.timeout)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(
        json.dumps(
            {key: value for key, value in result.items() if key != "checks"}, indent=2
        )
    )


if __name__ == "__main__":
    main()

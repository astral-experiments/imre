"""Independently audit retained live-conformance samples; never run native tools.

Strict mode requires recorded files. --portable checks artifacts and explicitly
rebased external inputs, and reports exactly which evidence was checked.
Copied result directories rebase automatically; --rebase OLD=NEW maps sources
or other retained inputs. Embedded build records remain available in either mode.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import math
import os
from pathlib import Path
import stat
import sys


ENGINES = ("relar", "port", "original", "codon", "rust")
BASES = ("unchanged-github", "changed-github", "changed-diff")
PAIRS = (("relar", "rust"), ("codon", "rust"), ("original", "relar"),
         ("port", "relar"), ("codon", "relar"), ("original", "rust"), ("port", "rust"))
EMPTY = hashlib.sha256(b"").hexdigest()
SENTINEL = b"previous complete report must survive\n"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha(value: str) -> None:
    assert isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value), value


def option(arguments: list[str], flag: str) -> str:
    assert arguments.count(flag) == 1, (flag, arguments)
    return arguments[arguments.index(flag) + 1]


def close(actual: float, expected: float) -> None:
    assert isinstance(actual, (int, float)) and not isinstance(actual, bool)
    assert math.isfinite(actual) and math.isclose(actual, expected, rel_tol=1e-12, abs_tol=1e-15), (actual, expected)


def balanced(orders: list[list[str]]) -> None:
    assert len(orders) == 10 and len({tuple(order) for order in orders}) == 10
    assert all(len(order) == 5 and set(order) == set(ENGINES) for order in orders)
    for position in range(5):
        assert Counter(order[position] for order in orders) == dict.fromkeys(ENGINES, 2)
    assert Counter(pair for order in orders for pair in zip(order, order[1:])) == {
        (left, right): 2 for left in ENGINES for right in ENGINES if left != right}


class Evidence:
    def __init__(self, portable: bool, rebases: list[tuple[Path, Path]]):
        self.portable = portable
        self.rebases = sorted(rebases, key=lambda pair: len(pair[0].parts), reverse=True)
        self.checked: set[str] = set()
        self.missing: set[str] = set()
        self.skipped_external: set[str] = set()

    def path(self, value: str | Path) -> Path:
        path = Path(value).absolute()
        for old, new in self.rebases:
            if path.is_relative_to(old):
                return new / path.relative_to(old)
        return path

    def read(self, value: str | Path, expected: str | None = None, size: int | None = None,
             *, required: bool = False, external: bool = False) -> bytes | None:
        if expected is not None:
            sha(expected)
        if self.portable and external and not any(Path(value).absolute().is_relative_to(old) for old, _ in self.rebases):
            self.skipped_external.add(str(value))
            return None
        path = self.path(value)
        if not path.is_file():
            assert self.portable and not required, (path, "required evidence is missing")
            self.missing.add(str(path))
            return None
        data = path.read_bytes()
        if expected is not None:
            assert digest(data) == expected, (path, "SHA-256 differs")
        assert size is None or len(data) == size, (path, "byte count differs")
        self.checked.add(str(path))
        return data

    def tree(self, value: str | Path, expected: dict[str, str]) -> bool:
        directory = self.path(value)
        if not directory.is_dir():
            assert self.portable, (directory, "retained tree is missing")
            self.missing.add(str(directory))
            return False
        actual = {}
        for path in sorted(directory.rglob("*")):
            name = path.relative_to(directory).as_posix()
            if path.is_symlink():
                actual[name] = "symlink:" + os.readlink(path)
            elif path.is_dir():
                actual[name] = "directory:" + oct(stat.S_IMODE(path.stat().st_mode))
            else:
                assert path.is_file(), (path, "unexpected special filesystem entry")
                data = path.read_bytes()
                self.checked.add(str(path))
                actual[name] = "file:" + oct(stat.S_IMODE(path.stat().st_mode)) + ":" + digest(data)
        assert actual == expected, (directory, "retained filesystem differs")
        return True


def validation(record: dict, path: Path, evidence: Evidence) -> dict | None:
    data = evidence.read(path, record["validation_sha256"])
    if data is None:
        return None
    value = json.loads(data)
    assert value["kind"] == "live-conformance-validation" and value["status"] == "passed"
    assert value["input_guards_passed"] is True and value["suite_revision"] == record["suite_revision"]
    assert value["suite_files"] == record["suite_files"]
    errors = {"fixture-exit": "CalledProcessError", "fixture-timeout": "TimeoutExpired",
              "fixture-json": "ValueError", "fixture-utf8": "UnicodeDecodeError",
              "fixture-unknown-path": "KeyError", "fixture-toml-utf8": "UnicodeDecodeError",
              "fixture-publication-error": "IsADirectoryError"}
    errors.update({"fixture-" + name: "ValueError" for name in
                   ("toml-syntax", "toml-duplicate", "toml-schema", "toml-type", "no-cases")})
    expected_cases = {*BASES, "fixture-ok", "fixture-relative-symlinks", *errors}
    cases = {}
    for row in value["records"]:
        key = (row["case"], row["engine"])
        assert key not in cases and row["engine"] in ENGINES
        cases[key] = row
        assert row["filesystem_guards_passed"] is True
        assert row["error"] == errors.get(row["case"])
        stdout, stderr = bytes.fromhex(row["stdout_hex"]), bytes.fromhex(row["stderr_hex"])
        assert digest(stdout) == row["stdout_sha256"] and digest(stderr) == row["stderr_sha256"]
        assert len(stderr) == row["stderr_bytes"] and stdout == b""
        assert (row["returncode"] == 0) == (row["error"] is None)
        if row["error"] is None:
            assert stderr == b""
        if row["case"] == "fixture-timeout":
            assert row["error"] == "TimeoutExpired" and row["timed_out_child_reaped"] is True
        output = Path(option(row["command"], "--output"))
        if not output.is_absolute():
            output = path.parent / "validation" / row["case"] / output
        if row["output_kind"] == "file":
            if row["error"] is not None:
                assert row["report_sha256"] == digest(SENTINEL) and row["report_bytes"] == len(SENTINEL)
            evidence.read(output, row["report_sha256"], row["report_bytes"])
        else:
            assert row["output_kind"] == "directory" and row["case"] == "fixture-publication-error"
            assert row["report_sha256"] is None and row["report_bytes"] is None
            evidence.read(output / "preserve", digest(SENTINEL), len(SENTINEL))
    names = {name for name, _ in cases}
    assert names == expected_cases
    assert set(cases) == {(name, engine) for name in names for engine in ENGINES}
    for path, expected in value["ty_sha256"].items():
        assert record["inputs"][path]["sha256"] == expected
    return cases


def provenance(record: dict, evidence: Evidence) -> str:
    kinds = {}
    for path, build in record["build_records"].items():
        data = evidence.read(path, record["inputs"][path]["sha256"], external=True)
        assert data is None or json.loads(data) == build, (path, "embedded build differs")
        kind = build.get("kind", "rust" if build.get("entrypoint") == "conformance_report" else "codon")
        assert kind not in kinds
        kinds[kind] = build
        assert build.get("status", "passed") == "passed"
        assert build.get("input_guards_passed", kind == "codon") is True
    required = {"live-conformance-program-build", "rust", "codon", "native-benchmark-platform-build"}
    assert required <= kinds.keys()
    program, rust, codon, platform = (kinds[name] for name in
                                    ("live-conformance-program-build", "rust", "codon", "native-benchmark-platform-build"))
    mode = program["mode"]
    assert mode in ("normal", "thinlto") and program["source_matches_revision"] is True
    assert program["target"] == rust["target"] == platform["target"]
    assert rust["thinlto"] == platform["thinlto"] == (mode == "thinlto")
    assert codon["domain"]["python_interop"] is False and codon["release"] == "v0.20.4"
    assert codon["native_platform"]["sha256"] == platform["library"]["sha256"]
    for commands, suffix in ((record["commands"], ""), (record["compute_commands"], "-compute")):
        assert set(commands) == set(ENGINES)
        for engine, build in (("relar", program), ("codon", codon)):
            assert record["inputs"][commands[engine][0]]["sha256"] == build["binaries"]["conformance_report" + suffix]["sha256"]
        assert record["inputs"][commands["rust"][0]]["sha256"] == rust["binary_sha256"]
    for path, item in record["inputs"].items():
        data = evidence.read(path, item["sha256"], item["bytes"], external=True)
        if data is not None:
            current = evidence.path(path)
            assert stat.S_IMODE(current.stat().st_mode) == item["mode"], (current, "permissions differ")
            assert current.resolve() == evidence.path(item["resolved_path"]).resolve(), (current, "resolution differs")
    for binary, libraries in record["linked_libraries"].items():
        for path, item in libraries.items():
            assert record["inputs"][path]["sha256"] == item["sha256"]
            assert record["inputs"][path]["bytes"] == item["bytes"]
            if binary != record["commands"]["original"][0]:
                assert "libpython" not in Path(path).name.lower()
    return mode


def audit(path: Path, *, portable: bool = False, rebases: list[tuple[Path, Path]] = ()) -> dict:
    path = path.resolve(strict=True)
    raw = path.read_bytes()
    record = json.loads(raw)
    assert record["schema"] == 1 and record["kind"] == "live-conformance-benchmark"
    assert record["status"] == "passed" and record["input_guards_passed"] is True, "partial/failed timing run"
    assert record["source_matches_revision"] is True
    samples, warmups, rounds = (record[name] for name in ("samples", "warmups", "compute_rounds"))
    assert type(samples) is int and samples > 0 and samples % 10 == 0
    assert type(warmups) is int and warmups >= 0 and type(rounds) is int and rounds > 0
    orders = record["orders"]
    balanced(orders)
    workloads = {item["name"]: item for item in record["workloads"]}
    names = [base + suffix for base in BASES for suffix in ("/live", "/compute-0", f"/compute-{rounds}")]
    assert list(workloads) == names and len(record["workloads"]) == len(names)
    original_root = Path(option(workloads[names[0]]["arguments"], "--output")).parents[2]
    evidence = Evidence(portable, [(original_root, path.parent), *rebases])
    sample_bytes = evidence.read(original_root / "samples.jsonl", record["samples_sha256"], required=True)
    assert sample_bytes.endswith(b"\n"), "truncated samples.jsonl"
    rows = [json.loads(line) for line in sample_bytes.splitlines()]
    assert rows == record["rows"], "embedded rows differ from retained samples"
    assert len(rows) == len(names) * (samples + warmups) * len(ENGINES)
    validation_rows = validation(record, original_root / "validation.json", evidence)
    mode = provenance(record, evidence)
    saved_summaries = {item["workload"]: item for item in record["summaries"]}
    assert set(saved_summaries) == set(names) and len(record["summaries"]) == len(names)
    report_evidence, capture_evidence, summaries = 0, 0, []
    offset = 0
    for base in BASES:
        prepared = workloads[base + "/live"]["preparation"]
        assert prepared["guards_passed"] is True and prepared["suite_revision"] == record["suite_revision"]
        capture_root = original_root / base / "captures"
        assert str(capture_root) in record["frozen_inputs"]
        capture_evidence += evidence.tree(capture_root, record["frozen_inputs"][str(capture_root)])
        data = evidence.read(capture_root / "preparation.json")
        assert data is None or json.loads(data) == prepared
        evidence.read(capture_root / "manifest.json", prepared["manifest_sha256"])
        for side in ("old", "new"):
            capture = prepared["captures"][side]
            assert capture["returncode"] == 0
            for stream, suffix in (("stdout", ".json"), ("stderr", ".stderr")):
                evidence.read(capture_root / (side + suffix), capture[stream + "_sha256"], capture[stream + "_bytes"])
        assert (prepared["captures"]["old"]["stdout_sha256"] == prepared["captures"]["new"]["stdout_sha256"]) == base.startswith("unchanged")
        first = rows[offset]
        report_hash, report_size = first["report_sha256"], first["report_bytes"]
        sha(report_hash)
        assert type(report_size) is int and report_size > 0
        report = evidence.read(option(workloads[base + "/live"]["arguments"], "--output"), report_hash, report_size)
        report_evidence += report is not None
        for suffix, count in (("/live", None), ("/compute-0", 0), (f"/compute-{rounds}", rounds)):
            name = base + suffix
            item = workloads[name]
            assert item["preparation"] == prepared
            assert option(item["arguments"], "--output") == str(original_root / base / "output/report.md")
            assert option(item["arguments"], "--format") == ("diff" if base.endswith("diff") else "github")
            if count is not None:
                assert option(item["arguments"], "--rounds") == str(count)
            expected_hash, expected_size = (EMPTY, 0) if count == 0 else (report_hash, report_size)
            stdout = b"" if count is None else b"0\n" if count == 0 else (
                str(len(report.decode("utf-8")) * count) + "\n").encode() if report is not None else None
            values = {engine: [] for engine in ENGINES}
            stdout_hashes = set()
            kind = "live" if count is None else "report-compute"
            for index in range(-warmups, samples):
                for position, engine in enumerate(orders[index % len(orders)]):
                    row = rows[offset]
                    offset += 1
                    assert type(row["round"]) is int and type(row["position"]) is int and type(row["warmup"]) is bool
                    assert (row["workload"], row["kind"], row["engine"], row["round"], row["position"], row["warmup"]) == (name, kind, engine, index, position, index < 0)
                    assert type(row["elapsed_ns"]) is int and row["elapsed_ns"] > 0
                    assert row["returncode"] == 0 and row["filesystem_guards_passed"] is True
                    sha(row["stdout_sha256"])
                    assert (row["report_sha256"], row["report_bytes"]) == (expected_hash, expected_size)
                    assert stdout is None or row["stdout_sha256"] == digest(stdout)
                    stdout_hashes.add(row["stdout_sha256"])
                    if index >= 0:
                        values[engine].append(row["elapsed_ns"])
            assert len(stdout_hashes) == 1, (name, "engine stdout differs")
            saved = saved_summaries[name]
            assert saved["kind"] == kind and set(saved["engines"]) == set(ENGINES)
            medians = {}
            for engine, elapsed in values.items():
                assert len(elapsed) == samples
                elapsed.sort()
                median = (elapsed[(samples - 1) // 2] + elapsed[samples // 2]) / 2 / 1e9
                summary = saved["engines"][engine]
                assert summary["samples"] == samples
                for key, expected in (("median_seconds", median), ("minimum_seconds", elapsed[0] / 1e9),
                                      ("maximum_seconds", elapsed[-1] / 1e9)):
                    close(summary[key], expected)
                medians[engine] = median
            if count is None and validation_rows is not None:
                for engine in ENGINES:
                    validated = validation_rows[(base, engine)]
                    assert validated["report_sha256"] == report_hash and validated["report_bytes"] == report_size
            summaries.append({"workload": name, "samples_per_engine": samples,
                              "report_sha256": expected_hash, "report_bytes": expected_size,
                              "median_ms": {engine: value * 1000 for engine, value in medians.items()},
                              "ratios": {left + "_over_" + right: medians[left] / medians[right] for left, right in PAIRS}})
    assert offset == len(rows) and set(record["frozen_inputs"]) == {str(original_root / base / "captures") for base in BASES}
    return {"status": "passed", "mode": mode, "benchmark_sha256": digest(raw),
            "source_revision": record["source_revision"], "rows": len(rows), "warmups_per_engine": warmups,
            "measured_rows": len(names) * samples * len(ENGINES),
            "scope": {"sample_math_and_balance": True, "native_programs_executed": False,
                      "builds_reproduced": False, "oracle_reports_recomputed": False, "portable": portable,
                      "validation_record_checked": validation_rows is not None,
                      "report_files_checked": report_evidence, "capture_trees_checked": capture_evidence,
                      "nonzero_compute_checksums_recomputed": report_evidence,
                      "files_hash_checked": len(evidence.checked), "unavailable_paths": len(evidence.missing),
                      "external_paths_not_checked": len(evidence.skipped_external)},
            "workloads": summaries}


def main() -> None:
    if sys.flags.optimize:
        raise SystemExit("audit requires assertions")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("benchmarks", nargs="+", type=Path)
    parser.add_argument("--portable", action="store_true")
    parser.add_argument("--rebase", action="append", default=[], metavar="OLD=NEW")
    args = parser.parse_args()
    rebases = []
    for value in args.rebase:
        old, separator, new = value.partition("=")
        if not separator or not old or not new:
            parser.error("--rebase requires OLD=NEW")
        rebases.append((Path(old).absolute(), Path(new).absolute()))
    print(json.dumps({"kind": "live-conformance-audit", "status": "passed",
                      "audits": [audit(path, portable=args.portable, rebases=rebases) for path in args.benchmarks]}, indent=2))


if __name__ == "__main__":
    main()

"""Replay frozen dataclass benchmark evidence without executing measured engines.

Run only after timing has completed. Reuse the prior independently reviewed
stream/filesystem checker and reconstruct the oracle from the measured Git tree.
"""
import argparse
import collections
from datetime import datetime
import gzip
import json
from pathlib import Path
import runpy
import statistics
import struct
import subprocess
import sys
import tempfile
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit("audit requires assertions")
sys.dont_write_bytecode = True
ROOT = Path("/home/dev-user/code/oss/imre")
REVISION = "f4aee96d255b4b4772e8d3fd2ba7169844cc58eb"
ENGINES = ("imre", "port", "original", "rust")
PREFIX = Path("/tmp/2026-10-21-dataclass-records")
LEGACY = Path("benchmarks/results/2026-10-21-memory-report-artifact-audit.py")


def report(result):
    lines = [
        "# Dataclass records and the real memory-report program",
        "",
        f"Measured source: `{REVISION}`.",
        "",
        "The port now uses fixed-layout dataclasses, keyword-only fields, fresh list defaults, "
        "a class method returning `Self`, a read-only property, and frozen difference records. "
        "The upstream script and Rust reference remain unchanged. The supported workload "
        "is the existing two-file comparison, with the same typed JSON boundary.",
        "",
        "All four 65,024-item comparisons meet the target of at most 2× Rust's time. "
        "Each input file contains 65,024 items; these are distinct-name copies of captured ty reports.",
        "",
        "| Build | Case | Imre (ms) | Rust (ms) | Imre/Rust | Upstream CPython/Imre | Typed CPython/Imre |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    for mode, rows in result["modes"].items():
        for row in rows:
            if row["label"].endswith("-256"):
                s = row["seconds"]
                lines.append(f"| {mode} | {row['label']} | {s['imre']*1000:.3f} | {s['rust']*1000:.3f} | "
                             f"{row['imre_over_rust']:.3f}× | {row['original_over_imre']:.2f}× | {row['port_over_imre']:.2f}× |")
    lines += [
        "", "Empty-report controls are reported without subtraction:", "",
        "| Build | Imre (ms) | Rust (ms) | Upstream CPython (ms) | Typed CPython (ms) |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for mode, rows in result["modes"].items():
        s = rows[0]["seconds"]
        lines.append(f"| {mode} | {s['imre']*1000:.3f} | {s['rust']*1000:.3f} | {s['original']*1000:.3f} | {s['port']*1000:.3f} |")
    lines += [
        "",
        "Times are whole-process medians of 12 measured rounds after two warmups, pinned to CPU 31 "
        "on AMD EPYC Milan. Four Williams orders balance positions and directed neighbors within "
        "measured rounds; warmups and transitions between rounds are not balanced. Each process "
        "starts, reads and decodes both files, compares and stably ranks changes, renders Markdown, "
        "and writes/closes the complete report. The unchanged upstream script also echoes file-mode "
        "output to stderr. Timing includes process and stream capture overhead; output verification "
        "is outside timing. Empty controls do not model the nonempty parse/compare cost.",
        "",
        "Both build modes passed the complete 72-case application validation across the four engines "
        "(264 executions per mode; upstream runs only its shared domain). The independent audit replays "
        "all retained streams, file hashes and final file bytes, verifies measured source/build/tool "
        "identities and round balance, and recomputes statistics and ratios from raw nanoseconds. "
        "CPython is 3.14.5 with PGO/LTO, the GIL enabled, and no JIT or debug build. Native code is "
        "optimized at level 3; ThinLTO uses matching reference settings and a 1,000-instruction LLVM budget.",
        "",
        "These measurements establish the target for this program and input set. The independent "
        "audit executes no engines, does not reconstruct historical process isolation, and makes "
        "no new CPU or allocation profiling claim.",
    ]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timings-complete", action="store_true", required=True)
    parser.add_argument("--output", type=Path, default=Path(str(PREFIX) + "-audit.json"))
    parser.add_argument("--report", type=Path, default=Path(str(PREFIX) + ".md"))
    args = parser.parse_args()
    helper_bytes = subprocess.check_output(["git", "-C", str(ROOT), "show", f"{REVISION}:{LEGACY}"])
    assert (ROOT / LEGACY).read_bytes() == helper_bytes
    audit = SimpleNamespace(**runpy.run_path(str(ROOT / LEGACY)))
    audit.guard(__file__)
    audit.guard(ROOT / LEGACY, audit.sha(helper_bytes))
    build_path = Path("/tmp/imre-dataclass-build/build-record.json")
    build = audit.load(build_path)
    assert build["revision"] == REVISION and build["source_matches_revision"] and build["input_guards_passed"]
    for relative, digest in build["source_sha256"].items():
        assert audit.sha(audit.source(REVISION, relative)) == digest, relative
    counts = collections.Counter()
    modes, identities = {}, {}
    context_path = Path("/tmp/imre-dataclass-timing-context.json")
    context = audit.load(context_path)
    assert set(context["commands"]) == {"normal", "thinlto"}
    previous_completed = datetime.fromisoformat(context["started_at"])
    with tempfile.TemporaryDirectory(prefix="dataclass-data-audit-") as temporary:
        oracle_root = Path(temporary)
        first = audit.load("/tmp/imre-dataclass-prepared-normal.json")
        for name, path in first["source_and_tool_paths"].items():
            if name.startswith(("helper:", "example:", "stdlib:", "fixture:", "upstream:")):
                relative = Path(path).relative_to(ROOT)
                data = audit.source(REVISION, relative)
                assert audit.sha(data) == first["source_and_tool_sha256"][name]
                destination = oracle_root / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(data)
        benchmark = runpy.run_path(str(oracle_root / "benchmarks/memory_benchmark.py"))
        validator = benchmark["validator"]
        empty = validator.case("empty-control", validator.report(), validator.report())
        workloads = [(benchmark["describe"](empty, 0, "empty-schema-valid-reports"), empty), *benchmark["workloads"]([32, 256])]
        application = validator.cases()
        assert len(application) == 72 and len(workloads) == 7
        for mode in ("normal", "thinlto"):
            timing_path = Path(f"/tmp/imre-dataclass-timing-{mode}.json")
            timing = audit.load(timing_path)
            prep_path = Path(f"/tmp/imre-dataclass-prepared-{mode}.json")
            prep = audit.load(prep_path, timing["preparation_record_sha256"])
            assert timing["preparation"] == prep
            assert timing["revision"] == prep["revision"] == REVISION
            assert not timing["dirty"] and not prep["dirty"]
            assert timing["input_guards_passed"] and prep["input_guards_passed"]
            started = datetime.fromisoformat(timing["started_at_utc"])
            completed = datetime.fromisoformat(timing["completed_at_utc"])
            assert previous_completed <= started < completed <= datetime.fromisoformat(context["completed_at"])
            assert datetime.fromisoformat(prep["completed_at_utc"]) <= started
            previous_completed = completed
            assert timing["samples"] == 12 and timing["warmups"] == 2 and timing["copies"] == [32, 256]
            assert timing["cpu"]["affinity"] == [31] and timing["cpu"]["model"] == "AMD EPYC-Milan Processor"
            python = timing["python"]
            assert python["implementation"] == "cpython" and python["version_number"] == "3.14.5"
            assert python["gil_enabled"] and not any(python[key] for key in ("free_threaded_build", "jit_enabled", "debug_build"))
            assert all(python["pgo"].values()) and python["lto"]["lto_flag_present"] and python["lto"]["configure_option"] == "--with-lto"
            assert timing["python_environment"] == {"PYTHONHASHSEED": "0", "PYTHONPATH": "", "PYTHONHOME": None}
            assert prep["thinlto"] == (mode == "thinlto") and prep["effective_target"] == "x86_64-unknown-linux-gnu"
            paths, hashes = prep["source_and_tool_paths"], prep["source_and_tool_sha256"]
            assert paths.keys() == hashes.keys()
            for name, path in paths.items():
                audit.captured(path, hashes[name], REVISION)
            assert Path(paths["imre_build_record"]) == build_path
            assert build["sha256"]["compiler"] == hashes["compiler"] and build["sha256"][mode + "_runtime"] == hashes["runtime"]
            for name in ("binary", "llvm", "validation"):
                audit.guard(prep[name], prep[name + "_sha256"])
            expected_command = [paths["compiler"], "build", str(ROOT / "examples/memory_report/__main__.py"), "--module-root", str(ROOT / "examples"), "-o", prep["binary"], "--runtime", paths["runtime"], "--clang", paths["clang"], "--opt-level", "3", "--emit-llvm", prep["llvm"], "--target", prep["effective_target"]]
            if mode == "thinlto":
                expected_command += ["--lto-linker", paths["lto_linker"]]
            assert prep["compile_command"] == expected_command
            timing_command = ["python3", "benchmarks/memory_benchmark.py", "--compiler", paths["compiler"], "--runtime", paths["runtime"], "--imre-build-record", str(build_path), "--rust", paths["rust"], "--rust-build-record", paths["rust_build_record"], "--python", paths["python"], "--clang", paths["clang"], "--target", prep["effective_target"], "--artifact-dir", str(timing_path.with_suffix("")), "--output", str(timing_path)]
            if mode == "thinlto":
                timing_command += ["--lto-linker", paths["lto_linker"]]
            timing_command += ["--prepared-record", str(prep_path), "--samples", "12", "--warmups", "2", "--copies", "32", "256", "--cpu", "31"]
            assert context["commands"][mode] == timing_command
            log = audit.raw(timing_path.with_suffix(".log")).decode()
            assert log.rstrip().endswith("Saved " + str(timing_path))
            assert prep["native_elf"] == {"bits": 64, "byte_order": "little", "machine": 62}
            for executable in (prep["binary"], paths["rust"]):
                with Path(executable).open("rb") as stream:
                    header = stream.read(20)
                assert header[:6] == b"\x7fELF\x02\x01" and struct.unpack("<H", header[18:20])[0] == 62
            rust = audit.load(paths["rust_build_record"])
            assert rust == prep["rust_build_record"] and rust["binary_sha256"] == hashes["rust"] and rust["input_guards_passed"]
            assert rust["thinlto"] == (mode == "thinlto") and rust["llvm_budget"] == (1000 if mode == "thinlto" else None)
            assert rust["rustc_verbose"].strip() == build["toolchain"].strip()
            for key, value in {"CARGO_PROFILE_RELEASE_OPT_LEVEL": "3", "CARGO_PROFILE_RELEASE_LTO": "thin" if mode == "thinlto" else "off", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16"}.items():
                assert rust["environment"][key] == value
            for relative, digest in rust["sources"].items():
                assert audit.sha(audit.source(REVISION, "benchmarks/reference/memory_report/" + relative)) == digest
            previous = json.loads(gzip.decompress(audit.raw(ROOT / f"benchmarks/results/2026-10-21-memory-report-{mode}.json.gz")))
            previous_hashes = previous["preparation"]["source_and_tool_sha256"]
            unchanged = [name for name in hashes if name.startswith(("helper:", "fixture:", "upstream:", "rust_source:")) or name in ("python", "clang", "rust", "rust_build_record", "lto_linker")]
            assert all(hashes[name] == previous_hashes[name] for name in unchanged)
            validation = audit.load(prep["validation"])
            assert validation["cases"] == prep["validation_cases"] == 72 and validation["engine_executions"] == 264
            assert tuple(validation["engines"]) == ENGINES
            for retained, (case, error, original) in zip(validation["results"], application, strict=True):
                assert retained["case"] == validator.case_record(case) and retained["expected_error"] == error
                assert set(retained["engines"]) == set(ENGINES) - (set() if original else {"original"})
                expected = validator.oracle(case, error)
                for engine, capture in retained["engines"].items():
                    audit.check_result(validator, case, expected, {**capture, "_engine": engine})
                    counts["preparation_executions"] += 1
            commands = benchmark["commands_for"](prep, paths)
            commands = {engine: [arg.replace(str(oracle_root), str(ROOT)) for arg in command] for engine, command in commands.items()}
            orders = [list(order) for order in benchmark["engine_orders"](commands)]
            assert timing["engine_orders"] == orders
            rows = [timing["startup_control"], *timing["workloads"]]
            directory = timing_path.with_suffix("")
            assert {entry.name for entry in directory.iterdir()} == {row["label"] for row in rows}
            computed = []
            for row, (metadata, case) in zip(rows, workloads, strict=True):
                assert {key: row[key] for key in metadata} == metadata
                assert set(row["gates"]) == set(ENGINES)
                assert {entry.name for entry in (directory / row["label"]).iterdir()} == set(ENGINES)
                expected = validator.oracle(case)
                for engine, capture in row["gates"].items():
                    assert capture["command"] == [*commands[engine], *case.arguments] and capture["elapsed_ns"] > 0
                    audit.check_result(validator, case, expected, {**capture, "_engine": engine}, directory / row["label"] / engine)
                    counts["gates"] += 1
                for phase, count in (("warmups", 2), ("samples", 12)):
                    assert len(row[phase]) == count
                    for index, capture in enumerate(row[phase]):
                        assert set(capture) == {"index", "order", *ENGINES} and capture["index"] == index and capture["order"] == orders[index % 4]
                        for engine in ENGINES:
                            assert type(capture[engine]["elapsed_ns"]) is int and capture[engine]["elapsed_ns"] > 0
                            assert {key: value for key, value in capture[engine].items() if key != "elapsed_ns"} == {key: value for key, value in row["gates"][engine].items() if key != "elapsed_ns"}
                            counts[phase] += 1
                positions = collections.Counter((engine, position) for capture in row["samples"] for position, engine in enumerate(capture["order"]))
                neighbors = collections.Counter(pair for capture in row["samples"] for pair in zip(capture["order"], capture["order"][1:]))
                assert len(positions) == 16 and set(positions.values()) == {3}
                assert len(neighbors) == 12 and set(neighbors.values()) == {3}
                seconds = {}
                for engine in ENGINES:
                    values = [capture[engine]["elapsed_ns"] / 1e9 for capture in row["samples"]]
                    q = statistics.quantiles(values, n=4, method="inclusive")
                    recomputed = {"median_seconds": statistics.median(values), "min_seconds": min(values), "max_seconds": max(values), "p25_seconds": q[0], "p75_seconds": q[2]}
                    assert recomputed == row["summary"][engine]
                    seconds[engine] = recomputed["median_seconds"]
                for left, right in (("original", "imre"), ("port", "imre"), ("imre", "rust"), ("original", "rust"), ("port", "rust")):
                    assert row[left + "_over_" + right] == seconds[left] / seconds[right]
                fractions = {engine: computed[0]["seconds"][engine] / seconds[engine] for engine in ENGINES} if computed else None
                if fractions is not None:
                    assert row["startup_control_fraction"] == fractions
                computed.append({"label": row["label"], "records": row["records"], "seconds": seconds,
                                 "imre_over_rust": seconds["imre"] / seconds["rust"],
                                 "original_over_imre": seconds["original"] / seconds["imre"],
                                 "port_over_imre": seconds["port"] / seconds["imre"],
                                 "startup_control_fraction": fractions})
            modes[mode] = computed
            identities[mode] = {"preparation": audit.guard(prep_path), "timing": audit.guard(timing_path),
                                "binary": prep["binary_sha256"], "runtime": hashes["runtime"], "rust": hashes["rust"],
                                "unchanged_reference_bindings": unchanged}
    targets = [row for rows in modes.values() for row in rows if row["label"].endswith("-256")]
    assert len(targets) == 4 and all(row["records"] == {"old.json": 65024, "project.json": 65024} for row in targets)
    assert all(row["imre_over_rust"] <= 2 for row in targets), targets
    for path, digest in list(audit.INPUTS.items()):
        audit.guard(path, digest)
    result = {"schema": 1, "status": "passed", "measurement_revision": REVISION,
              "scope": "Independent data-only replay of preparation, timing streams/files, raw statistics/order, and Git/build/tool identities; no profiling or measured engine execution.",
              "counts": dict(counts), "modes": modes, "identities": identities,
              "timing_context_sha256": audit.INPUTS[str(context_path)],
              "all_four_large_cases_at_most_twice_rust": True,
              "audit_script_sha256": audit.INPUTS[str(Path(__file__).absolute())], "guarded_inputs": audit.INPUTS,
              "limits": "Reuses the Git-bound independent application oracle and prior check_result helper. Captures cannot reconstruct historical host isolation. Native identities are rehashed; binaries are not executed or rebuilt."}
    with args.output.open("x") as stream:
        json.dump(result, stream, indent=2)
        stream.write("\n")
    with args.report.open("x") as stream:
        stream.write(report(result))
    print(json.dumps({"output": str(args.output), "report": str(args.report), "counts": dict(counts),
                      "large_imre_over_rust": [row["imre_over_rust"] for row in targets]}, indent=2))


if __name__ == "__main__":
    main()

"""Measure complete streaming uv catalogs on Linux using fresh processes and per-child RSS."""

import argparse
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import platform
import runpy
import shutil
import statistics
import subprocess
import sys
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
PACKAGE = ROOT / "examples/python_catalog"
native = SimpleNamespace(**runpy.run_path(str(HERE / "native_tools_benchmark.py")))
catalog = native.catalog
builds = SimpleNamespace(**runpy.run_path(str(HERE / "live_conformance_benchmark.py")))
ENGINES = ("original", "port", "codon", "relar", "rust", "relar_thin", "rust_thin")
SCALES = (1, 10)
FORMATS = ("plain", "gzip")


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def identify(path):
    return {"path": str(path), **native.file_identity(path)}


def oracle_sources():
    return {name: native.file_identity(PACKAGE / name) for name in (
        "upstream_adapter.py", "upstream/fetch-download-metadata.py", "upstream/UPSTREAM.json")}


def prepare(directory, python):
    """Freeze exact repeated raw rows and concatenated copies of the shipped gzip member."""
    raw = catalog.verify_provenance()
    assert raw.endswith(b"\n")
    provenance = json.loads((PACKAGE / "sample/provenance.json").read_text())
    packed = (PACKAGE / "sample/python-build-standalone.ndjson.gz").read_bytes()
    directory.mkdir(parents=True, exist_ok=False)
    inputs = {}
    for scale in SCALES:
        plain = directory / f"{scale}x.ndjson"
        compressed = directory / f"{scale}x.ndjson.gz"
        for path, data in ((plain, raw), (compressed, packed)):
            with path.open("xb") as output:
                for _ in range(scale):
                    output.write(data)
        with gzip.open(compressed, "rb") as stream:
            assert hashlib.file_digest(stream, "sha256").hexdigest() == native.shared.sha256(plain)
        inputs[f"{scale}x-plain"] = identify(plain)
        inputs[f"{scale}x-gzip"] = identify(compressed)

    expected = directory / "expected.json"
    command = [*catalog.original_command(python), str(directory / "1x.ndjson"), str(expected)]
    original = subprocess.run(command, env=native.environment(directory), capture_output=True, check=True)
    assert original.stdout == original.stderr == b"" and len(json.loads(expected.read_bytes())) == 6098
    record = {"schema": 1, "provenance": provenance, "inputs": inputs, "expected": identify(expected),
              "entries": 6098, "oracle_command": command, "oracle_sources": oracle_sources(), "python": identify(python)}
    write_json(directory / "inputs.json", record)
    return record


def engine_orders():
    """Fourteen Williams orders balance positions and directed neighbors for seven engines."""
    count = len(ENGINES)
    forward = [tuple(ENGINES[(index + rotation) % count] for index in (0, 1, 6, 2, 5, 3, 4))
               for rotation in range(count)]
    orders = [*forward, *(tuple(reversed(order)) for order in forward)]
    assert all(Counter(order[i] for order in orders) == dict.fromkeys(ENGINES, 2) for i in range(count))
    neighbors = Counter(pair for order in orders for pair in zip(order, order[1:]))
    assert neighbors == {(a, b): 2 for a in ENGINES for b in ENGINES if a != b}
    return orders


def checked_sources():
    paths = [Path(__file__).resolve(), HERE / "catalog_process_usage.c", HERE / "benchmark.py",
             HERE / "catalog_validate.py", HERE / "native_tools_benchmark.py", HERE / "live_conformance_benchmark.py",
             PACKAGE / "__init__.py", PACKAGE / "__main__.py", PACKAGE / "catalog.py",
             PACKAGE / "upstream_adapter.py", PACKAGE / "upstream/fetch-download-metadata.py",
             ROOT / "stdlib/relar/json.py"]
    return {str(path.relative_to(ROOT)): native.file_identity(path) for path in paths}


def verify_builds(paths, pilot):
    """Bind each measured executable and its settings to the engine on its command line."""
    guarded, records, cores = {}, {}, set()
    for name, binary in paths.items():
        path = binary.parent / "build-record.json"
        if pilot and not path.is_file():
            continue
        record = builds.verify_build_record(path, guarded)
        receipt = identify(path)
        if name.startswith("relar"):
            mode = "thinlto" if name.endswith("_thin") else "normal"
            assert record["kind"] == "streaming-catalog-program-build" and record["mode"] == mode
            assert (pilot or record["source_matches_revision"]) and record["binaries"]["python_catalog"] == builds.identity(binary)
            receipt.update(mode=mode, revision=record["revision"], core=record["core_build_record"]["sha256"])
            cores.add(receipt["core"])
        elif name.startswith("rust"):
            thin = name.endswith("_thin")
            assert record["entrypoint"] == "python_catalog" and record["thinlto"] == thin
            assert record["environment"]["CARGO_PROFILE_RELEASE_LTO"] == ("thin" if thin else "off")
            assert record["binary_sha256"] == native.shared.sha256(binary)
            receipt.update(mode="thinlto" if thin else "normal", target=record["target"],
                           rustc=record["rustc_verbose"])
        else:
            assert name == "codon" and record["binaries"]["python_catalog"] == {
                "path": str(binary), "sha256": native.shared.sha256(binary)}
            for filename, digest in record["native_catalog_io"].items():
                assert builds.digest(Path(filename)) == digest
                guarded[filename] = builds.identity(Path(filename))
            receipt.update(release=record["release"])
        records[name] = receipt
    if not pilot:
        assert records.keys() == paths.keys() and len(cores) == 1
    return records, guarded


def verify_late_members(directory, commands):
    """Require a distinct winning artifact from each of ten rows and compressed members."""
    directory.mkdir()
    rows = catalog.member_coverage_rows()
    plain, packed = directory / "probe.ndjson", directory / "probe.ndjson.gz"
    plain.write_bytes(b"".join(rows))
    packed.write_bytes(b"".join(gzip.compress(row, mtime=0) for row in rows))
    output = directory / "catalog.json"
    expected = None
    matches = []
    for engine, command in commands.items():  # The unchanged upstream CPython adapter goes first.
        for source in (plain, packed):
            output.write_bytes(native.SENTINEL)
            process = subprocess.run([*command, str(source), str(output)], stdin=subprocess.DEVNULL,
                                     capture_output=True, env=native.environment(directory), timeout=30)
            assert process.returncode == 0 and not process.stdout and not process.stderr, (engine, process.stderr)
            actual = output.read_bytes()
            if expected is None:
                assert engine == "original" and source == plain
                assert {entry["url"] for entry in json.loads(actual).values()} == {
                    f"https://example.invalid/member-{i}.tar.gz" for i in range(10)}
                expected = actual
            assert actual == expected, (engine, source, "skipped a member or row")
            matches.append([engine, source.name])
    return {"plain": identify(plain), "gzip": identify(packed), "output": identify(output), "matched": matches}


def benchmark(args):
    out = args.directory.resolve()
    out.mkdir(parents=True, exist_ok=False)
    frozen = args.inputs.resolve(strict=True)
    record = json.loads((frozen / "inputs.json").read_text())
    python = args.python.resolve(strict=True)
    assert identify(python) == record["python"]
    assert record["oracle_sources"] == oracle_sources()
    inputs = {key: Path(item["path"]) for key, item in record["inputs"].items()}
    assert inputs.keys() == {f"{scale}x-{kind}" for scale in SCALES for kind in FORMATS}
    assert record["provenance"] == json.loads((PACKAGE / "sample/provenance.json").read_text())
    for scale in SCALES:
        for kind, size_key in (("plain", "bytes"), ("gzip", "gzip_bytes")):
            assert record["inputs"][f"{scale}x-{kind}"]["bytes"] == scale * record["provenance"][size_key]
    expected = Path(record["expected"]["path"])
    def check_inputs():
        assert record["inputs"] == {key: identify(path) for key, path in inputs.items()}
        assert record["expected"] == identify(expected)
    check_inputs()
    expected_bytes = expected.read_bytes()
    paths = {name: getattr(args, name).resolve(strict=True) for name in ENGINES[2:]}
    commands = {"original": catalog.original_command(python), "port": catalog.python_command(python),
                **{name: [str(path)] for name, path in paths.items()}}
    time_bin = Path("/usr/bin/time").resolve(strict=True)
    compiler = Path(shutil.which(args.cc) or args.cc).resolve(strict=True)
    monitor = out / "process-usage"
    compile_monitor = [str(compiler), "-O2", "-Wall", "-Wextra", "-Werror", "-std=c11",
                       str(HERE / "catalog_process_usage.c"), "-o", str(monitor)]
    subprocess.run(compile_monitor, check=True)
    sources = checked_sources()
    build_records, guarded = verify_builds(paths, args.pilot)
    files = {"python": python, "time": time_bin, "cc": compiler, "monitor": monitor, **paths}
    files.update({f"build-{name}": Path(item["path"]) for name, item in build_records.items()})
    identities = {name: identify(path) for name, path in files.items()}
    libraries = {name: native.linked_libraries(path) for name, path in {"python": python, **paths}.items()}
    orders = engine_orders()
    if not args.pilot and (args.samples < len(orders) or args.samples % len(orders)):
        raise ValueError("full measurements must include complete balanced 14-order blocks")
    cpu = native.shared.pin_cpu(args.cpu)
    environment = native.environment(out)
    result = {"schema": 1, "kind": "streaming-catalog", "completed": False, "pilot": args.pilot,
              "started_utc": native.utc(), "cpu": cpu, "system": platform.uname()._asdict(),
              "python": subprocess.check_output([str(python), "-VV"], text=True).strip(),
              "revision": subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip(),
              "hashes": identities, "libraries": libraries, "sources": sources, "inputs": record,
              "builds": build_records, "guarded_build_inputs": guarded,
              "member_coverage": verify_late_members(out / "member-coverage", commands),
              "compile_monitor": compile_monitor, "environment": environment, "commands": commands,
              "samples_per_case": args.samples, "warmups_per_case": 1, "orders": orders, "workloads": {},
              "method": "Fresh serial processes on one pinned CPU, warm filesystem caches, full input through output. "
                        "CLOCK_MONOTONIC immediately before posix_spawn through wait4 exit. Linux ru_maxrss (KiB) "
                        "belongs to that PID; the small native monitor isolates it from driver memory. GNU time "
                        "independently observes its child tree on every warmup. Every output is compared byte for "
                        "byte outside timing. The 10x archives contain ten identical concatenated gzip members."}
    for label, source in inputs.items():
        work = out / label
        work.mkdir()
        output = work / "catalog.json"
        metrics = work / "usage.txt"
        gnu = work / "gnu-time-rss.txt"

        def observe(engine, crosscheck):
            output.write_bytes(native.SENTINEL)
            command = [str(monitor), str(metrics), *commands[engine], str(source), str(output)]
            if crosscheck:
                command = [str(time_bin), "-f", "%M", "-o", str(gnu), "--", *command]
            process = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, cwd=work,
                                     env=environment, timeout=180, check=False)
            if process.returncode or process.stdout or process.stderr:
                raise RuntimeError((label, engine, process.returncode, process.stdout, process.stderr))
            assert output.read_bytes() == expected_bytes, (label, engine, "different output")
            elapsed_ns, rss_kib = map(int, metrics.read_text().split())
            assert elapsed_ns > 0 and rss_kib > 0
            observation = {"engine": engine, "elapsed_ns": elapsed_ns, "peak_rss_kib": rss_kib}
            if crosscheck:
                # Linux wait4 propagates the largest child high-water mark to GNU time's monitor process.
                independent_rss = int(gnu.read_text())
                assert independent_rss == rss_kib, (engine, independent_rss, rss_kib)
                observation["gnu_time_peak_rss_kib"] = independent_rss
            return observation

        warmup = [observe(engine, True) for engine in reversed(orders[0])]
        results = []
        case = {"warmup": warmup, "samples": results}
        result["workloads"][label] = case
        for i in range(args.samples):
            results.append([observe(engine, False) for engine in orders[i % len(orders)]])
            write_json(out / "results.json", result)
        medians = {engine: {field: statistics.median(
            row[field] for round_ in results for row in round_ if row["engine"] == engine
        ) for field in ("elapsed_ns", "peak_rss_kib")} for engine in ENGINES}
        ratios = {mode: {field: medians["relar" + suffix][field] / medians["rust" + suffix][field]
                         for field in ("elapsed_ns", "peak_rss_kib")}
                  for mode, suffix in (("ordinary", ""), ("thinlto", "_thin"))}
        case.update(medians=medians, relar_over_rust=ratios)
        print(f"{label} {ratios}", flush=True)
    check_inputs()
    assert sources == checked_sources()
    assert identities == {name: identify(path) for name, path in files.items()}
    assert guarded == {name: builds.identity(Path(name)) for name in guarded}, "build evidence changed"
    assert libraries == {name: native.linked_libraries(path) for name, path in {"python": python, **paths}.items()}
    cases = result["workloads"]
    result["peak_rss_10x_over_1x"] = {kind: {
        engine: cases[f"10x-{kind}"]["medians"][engine]["peak_rss_kib"] /
                cases[f"1x-{kind}"]["medians"][engine]["peak_rss_kib"]
        for engine in ENGINES} for kind in FORMATS}
    result["time_target_met"] = all(mode["elapsed_ns"] <= 2 for case in cases.values()
                                    for mode in case["relar_over_rust"].values())
    result["completed"] = True
    result["completed_utc"] = native.utc()
    write_json(out / "results.json", result)
    if not args.pilot and not result["time_target_met"]:
        raise SystemExit("A Relar/Rust median exceeds 2x; all raw data was retained")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    prep = sub.add_parser("prepare")
    run = sub.add_parser("run")
    for item in (prep, run):
        item.add_argument("--directory", required=True, type=Path)
        item.add_argument("--python", type=Path, default=Path(sys.executable))
    run.add_argument("--inputs", required=True, type=Path)
    for engine in ENGINES[2:]:
        run.add_argument("--" + engine.replace("_", "-"), dest=engine, required=True, type=Path)
    run.add_argument("--cpu", type=int)
    run.add_argument("--samples", type=int, default=14)
    run.add_argument("--pilot", action="store_true", help="allow an exploratory partial design")
    run.add_argument("--cc", default="cc")
    args = parser.parse_args()
    if args.action == "prepare":
        prepare(args.directory.resolve(), args.python.resolve(strict=True))
    elif args.samples < 1:
        parser.error("samples must be positive")
    else:
        benchmark(args)


if __name__ == "__main__":
    main()

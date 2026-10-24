"""Check streaming catalog measurements independently, without running measured programs.

Use --strict-files on the measuring host to verify the named artifacts and final
outputs. An exported JSON or JSON.gz can be checked alongside its source checkout.
Supply --sha256 from a trusted receipt to authenticate every byte of that export.
"""

import argparse
from collections import Counter
from datetime import datetime, timedelta
import gzip
import hashlib
import json
from pathlib import Path
import re
from statistics import median


ENGINES = ("original", "port", "codon", "imre", "rust", "imre_thin", "rust_thin")
CASES = ("1x-plain", "1x-gzip", "10x-plain", "10x-gzip")
FIELDS = ("elapsed_ns", "peak_rss_kib")
SOURCES = set("""
benchmarks/stream_catalog_benchmark.py benchmarks/catalog_process_usage.c benchmarks/benchmark.py
benchmarks/catalog_validate.py benchmarks/native_tools_benchmark.py benchmarks/live_conformance_benchmark.py
examples/python_catalog/__init__.py examples/python_catalog/__main__.py examples/python_catalog/catalog.py
examples/python_catalog/upstream_adapter.py examples/python_catalog/upstream/fetch-download-metadata.py
stdlib/imre/json.py""".split())
# Published by the preceding, whole-file catalog comparison against unchanged uv.
CATALOG = (3440812, "753cd906307c1b4f830e9348703e0110ba38fddd97293780f146f78f0ed9a3f6")
# Independent construction of ten different releases and their upstream-rendered catalog.
MEMBERS = {
    "plain": (2520, "eeebfa9fb1c5ec25d9484a989163ce69b6184e113a21ac32c825664a1acfa654"),
    "gzip": (1742, "d8061d3b8c26ee4586064cdb8d4d69a9858892d8e8d8a06e0fa4fab3b163e991"),
    "output": (4263, "a4e7a5b792b69f18ca298b2532f1512b62b12ba7501aa1fd1576629e45a0bbc2"),
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique(pairs):
    result = dict(pairs)
    require(len(result) == len(pairs), "duplicate JSON key")
    return result


def invalid_constant(value):
    raise ValueError(f"non-finite JSON constant: {value}")


def file_id(path):
    require(path.is_file(), f"missing regular file: {path}")
    with path.open("rb") as stream:
        return path.stat().st_size, hashlib.file_digest(stream, "sha256").hexdigest()


def identity(item):
    size, sha = item["bytes"], item["sha256"]
    require(type(size) is int and size >= 0 and re.fullmatch("[0-9a-f]{64}", sha), "bad file identity")
    return size, sha


def numeric_tree(value):
    return all(map(numeric_tree, value.values())) if isinstance(value, dict) else type(value) in (int, float)


def evidence(data, root, strict):
    files, commands, builds = data["hashes"], data["commands"], data["builds"]
    require(set(commands) == set(ENGINES) and set(builds) == set(ENGINES[2:]), "missing or extra engine")
    required_files = {"python", "time", "monitor", "cc", *ENGINES[2:], *("build-" + e for e in ENGINES[2:])}
    require(set(files) == required_files, "missing or extra measured file")
    require(len({files[e]["sha256"] for e in ENGINES[2:]}) == len(ENGINES[2:]), "duplicate native executables")
    python = files["python"]["path"]
    adapter = Path(commands["original"][-1])
    recorded_root = adapter.parents[2]
    require(recorded_root.is_absolute() and ".." not in recorded_root.parts and
            adapter == recorded_root / "examples/python_catalog/upstream_adapter.py", "wrong upstream adapter")
    require(data["compile_monitor"] == [files["cc"]["path"], "-O2", "-Wall", "-Wextra", "-Werror", "-std=c11",
            str(recorded_root / "benchmarks/catalog_process_usage.c"), "-o", files["monitor"]["path"]], "wrong RSS monitor build")
    require(commands["original"] == [python, "-P", "-s", "-B", str(adapter)], "wrong upstream command")
    port = commands["port"]
    require(port[:5] == [python, "-P", "-s", "-B", "-c"] and len(port) == 9 and
            port[6:] == ["python_catalog", str(recorded_root / "examples"), str(recorded_root / "stdlib")],
            "wrong typed CPython command")
    require(port[5] == 'import runpy, sys; module = sys.argv[1]; sys.path[:0] = sys.argv[2:4]; '
            'sys.argv = [module, *sys.argv[4:]]; runpy.run_module(module, run_name=\'__main__\', alter_sys=True)',
            "unexpected CPython bootstrap")
    require(set(data["sources"]) == SOURCES, "wrong source set")
    for name, item in data["sources"].items():
        require(identity(item) == file_id(root / name), f"measured source differs: {name}")
    upstream = json.loads((root / "examples/python_catalog/upstream/UPSTREAM.json").read_bytes())
    require(upstream["revision"] == "83d556d712c7e316932b239c49e86a5de95d124a" and
            upstream["files"]["fetch-download-metadata.py"] == data["sources"][
                "examples/python_catalog/upstream/fetch-download-metadata.py"]["sha256"], "upstream source pin")

    artifacts = {item["path"]: identity(item) for item in files.values()}
    guarded = data["guarded_build_inputs"]
    for engine, build in builds.items():
        binary = files[engine]
        require(commands[engine] == [binary["path"]] and identity(guarded[binary["path"]]) == identity(binary),
                f"{engine} command or verified executable differs")
        receipt = files["build-" + engine]
        require(build["path"] == receipt["path"] and identity(build) == identity(receipt), f"{engine} receipt")
        if engine != "codon":
            require(build["mode"] == ("thinlto" if engine.endswith("_thin") else "normal"), f"{engine} mode")
    require(builds["imre"]["core"] == builds["imre_thin"]["core"] and
            builds["imre"]["revision"] == builds["imre_thin"]["revision"] and
            re.fullmatch("[0-9a-f]{40}", data["revision"]) and re.fullmatch("[0-9a-f]{40}", builds["imre"]["revision"]) and
            re.fullmatch("[0-9a-f]{64}", builds["imre"]["core"]), "inconsistent compiler build")
    require(builds["rust"]["target"] == builds["rust_thin"]["target"] and
            builds["rust"]["rustc"] == builds["rust_thin"]["rustc"] and
            builds["codon"]["release"] == "v0.20.4", "comparison toolchains differ")
    for name, item in guarded.items():
        path = Path(name)
        require(item["path"] == name and path.is_absolute() and ".." not in path.parts, f"invalid build input path: {name}")
        artifacts[name] = identity(item)
        if path.is_relative_to(recorded_root):
            require(file_id(root / path.relative_to(recorded_root)) == identity(item), f"build source differs: {name}")
    required = [root / n for n in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    for pattern in ("crates/*/Cargo.toml", "crates/**/*.rs", "examples/python_catalog/**/*.py", "stdlib/imre/**/*.py",
                    "benchmarks/reference/python_catalog/Cargo.*", "benchmarks/reference/python_catalog/src/*.rs",
                    "benchmarks/reference/python_catalog/build.py"):
        required.extend(root.glob(pattern))
    for path in required:
        name = path.relative_to(root)
        require(str(recorded_root / name) in guarded, f"missing build input {name}")
    require(set(data["libraries"]) == {"python", *ENGINES[2:]}, "missing linked libraries")
    for engine, libs in data["libraries"].items():
        require(libs and (engine == "python" or not any("libpython" in n for n in libs)), f"{engine} linkage")
        for name, item in libs.items():
            require(artifacts.get(name, identity(item)) == identity(item), "inconsistent library identity")
            artifacts[name] = identity(item)

    inputs = data["inputs"]
    sample = root / "examples/python_catalog/sample"
    provenance = json.loads((sample / "provenance.json").read_bytes())
    require(inputs["schema"] == 1 and inputs["provenance"] == provenance and inputs["entries"] == 6098,
            "input provenance or catalog size differs")
    require(provenance["revision"] == "db0d0dbfa44ff1306ea0e571962e0b2a6211e3c9" and
            provenance["release_rows"] == 567 and provenance["raw_artifacts"] == 58721, "unexpected release index")
    require(inputs["python"] == files["python"] and set(inputs["inputs"]) == set(CASES), "fixture set or oracle")
    require(identity(inputs["expected"]) == CATALOG, "catalog differs from the earlier uv oracle")
    packed = (sample / "python-build-standalone.ndjson.gz").read_bytes()
    raw = gzip.decompress(packed)
    for label in CASES:
        scale, kind = label.split("x-")
        content = raw if kind == "plain" else packed
        key = "" if kind == "plain" else "gzip_"
        digest = hashlib.sha256(content)
        require(digest.hexdigest() == provenance[key + "sha256"], f"shipped {kind} fixture differs")
        for _ in range(1, int(scale)):
            digest.update(content)
        item = inputs["inputs"][label]
        require(identity(item) == (int(scale) * len(content), digest.hexdigest()), f"wrong repeated fixture: {label}")
        artifacts[item["path"]] = identity(item)
    require(inputs["oracle_command"] == [*commands["original"], inputs["inputs"]["1x-plain"]["path"],
                                         inputs["expected"]["path"]], "oracle command differs")
    artifacts[inputs["expected"]["path"]] = CATALOG
    coverage = data["member_coverage"]
    require(coverage["matched"] == [[e, name] for e in ENGINES for name in ("probe.ndjson", "probe.ndjson.gz")],
            "missing distinct-member coverage")
    for kind, expected in MEMBERS.items():
        require(identity(coverage[kind]) == expected, f"wrong distinct-member {kind}")
        artifacts[coverage[kind]["path"]] = expected
    if strict:
        directory = Path(files["monitor"]["path"]).parent
        for label, case in data["workloads"].items():
            artifacts[str(directory / label / "catalog.json")] = CATALOG
            last = case["samples"][-1][-1]
            require((directory / label / "usage.txt").read_text().split() == [str(last[f]) for f in FIELDS],
                    f"{label}: retained wait4 observation differs")
            require((directory / label / "gnu-time-rss.txt").read_text().strip() == str(case["warmup"][-1]["peak_rss_kib"]),
                    f"{label}: retained GNU time cross-check differs")
        for name, expected in artifacts.items():
            require(file_id(Path(name)) == expected, f"retained artifact differs: {name}")
    return len(artifacts) if strict else 0


def calculate(data, sample_count):
    require(data["completed"] is True and data["pilot"] is False and data["kind"] == "streaming-catalog"
            and type(data["schema"]) is int and data["schema"] == 1, "expected a completed full streaming measurement")
    require(type(sample_count) is int and sample_count >= 14 and sample_count % 14 == 0
            and type(data["samples_per_case"]) is int and data["samples_per_case"] == sample_count
            and type(data["warmups_per_case"]) is int and data["warmups_per_case"] == 1, "wrong sample count")
    require(len(data["cpu"]["affinity"]) == 1 and set(data["cpu"]["affinity"]) <= set(data["cpu"]["allowed_cpus_before_pinning"])
            and data["system"]["system"] == "Linux", "missing pinned Linux CPU")
    require(data["environment"]["PYTHON_JIT"] == "0" and data["environment"]["PYTHONHASHSEED"] == "0", "environment differs")
    start, end = (datetime.fromisoformat(data[k]) for k in ("started_utc", "completed_utc"))
    require(start.utcoffset() == end.utcoffset() == timedelta(0) and start <= end, "invalid observation interval")
    orders = data["orders"]
    rotations = [[ENGINES[(shift + offset) % 7] for offset in (0, 1, -1, 2, -2, 3, -3)] for shift in range(7)]
    require(orders == rotations + [row[::-1] for row in rotations], "wrong predeclared engine orders")
    for column in zip(*orders):
        require(Counter(column) == dict.fromkeys(ENGINES, 2), "unbalanced engine positions")
    neighbors = Counter(pair for order in orders for pair in zip(order, order[1:]))
    require(neighbors == {(a, b): 2 for a in ENGINES for b in ENGINES if a != b}, "unbalanced neighbors")
    require(set(data["workloads"]) == set(CASES), "incomplete workload matrix")
    medians, ratios = {}, {}
    for label, case in data["workloads"].items():
        require(len(case["samples"]) == sample_count, f"{label}: partial samples")
        for i, rows in enumerate([case["warmup"], *case["samples"]]):
            order = orders[0][::-1] if i == 0 else orders[(i - 1) % 14]
            require([r["engine"] for r in rows] == order, f"{label} round {i}: wrong engines/order")
            for row in rows:
                keys = {"engine", "output_sha256", *FIELDS}
                require(set(row) == (keys if i else keys | {"gnu_time_peak_rss_kib"}),
                        f"{label}: unexpected observation fields")
                require(row["output_sha256"] == CATALOG[1], f"{label}: wrong measured output")
                require(all(type(row[field]) is int and row[field] > 0 for field in FIELDS), f"{label}: invalid units")
                if i == 0:
                    require(type(row["gnu_time_peak_rss_kib"]) is int and row["gnu_time_peak_rss_kib"] == row["peak_rss_kib"],
                            f"{label}: independent RSS disagrees")
        values = {e: {f: median(r[f] for sample in case["samples"] for r in sample if r["engine"] == e)
                      for f in FIELDS} for e in ENGINES}
        relative = {mode: {f: values[a][f] / values[b][f] for f in FIELDS} for mode, a, b in (
            ("ordinary", "imre", "rust"), ("thinlto", "imre_thin", "rust_thin"))}
        require(numeric_tree(case["medians"]) and numeric_tree(case["imre_over_rust"]) and
                values == case["medians"] and relative == case["imre_over_rust"], f"{label}: derived metrics differ")
        medians[label], ratios[label] = values, relative
    growth = {f: {kind: {e: medians["10x-" + kind][e][f] / medians["1x-" + kind][e][f] for e in ENGINES}
                  for kind in ("plain", "gzip")} for f in FIELDS}
    require(numeric_tree(data["peak_rss_10x_over_1x"]) and growth["peak_rss_kib"] == data["peak_rss_10x_over_1x"],
            "peak RSS growth differs")
    require(data["time_target_met"] is True and all(r["elapsed_ns"] <= 2 for c in ratios.values() for r in c.values()),
            "2x wall-time target failed")
    return {"workloads": len(CASES), "measured": sample_count * len(CASES) * len(ENGINES),
            "warmups": len(CASES) * len(ENGINES), "medians": medians, "imre_over_rust": ratios,
            "peak_rss_10x_over_1x": growth["peak_rss_kib"], "elapsed_10x_over_1x": growth["elapsed_ns"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", type=Path)
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--samples", type=int, default=14)
    parser.add_argument("--strict-files", action="store_true")
    parser.add_argument("--sha256", help="trusted SHA-256 of the supplied JSON or JSON.gz file")
    args = parser.parse_args()
    artifact = args.record.read_bytes()
    digest = hashlib.sha256(artifact).hexdigest()
    require(args.sha256 is None or digest == args.sha256, "measurement file SHA-256 differs")
    raw = gzip.decompress(artifact) if artifact.startswith(b"\x1f\x8b") else artifact
    data = json.loads(raw, object_pairs_hook=unique, parse_constant=invalid_constant)
    result = calculate(data, args.samples)
    checked = evidence(data, args.source_root, args.strict_files)
    print(json.dumps({"schema": 1, "record_sha256": digest, "json_sha256": hashlib.sha256(raw).hexdigest(),
                      "strict_files_checked": checked, **result}, indent=2))


if __name__ == "__main__":
    main()

"""Audit retained native-tool comparisons without executing benchmark binaries.

Pass one timing JSON for a preview, or normal and ThinLTO JSONs to check parity.
The upstream oracles recompute complete outputs; timing statistics and balance
checks are independent of the benchmark harness. Run after all timing finishes.
"""

import argparse
import ast
from collections import Counter
from datetime import datetime, timezone
from functools import lru_cache
import hashlib
import json
import math
from pathlib import Path
import runpy
import sys
from types import SimpleNamespace

if sys.flags.optimize or sys.version_info < (3, 14):
    raise SystemExit("audit requires CPython 3.14+ with assertions enabled")

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
ENGINES = ("relar", "port", "original", "codon", "rust")
PACKAGES = {"catalog": "python_catalog", "conformance": "conformance_report"}
PAIRS = (("relar", "rust"), ("codon", "rust"), ("original", "relar"),
         ("port", "relar"), ("codon", "relar"), ("original", "rust"))
SENTINEL = b"replace this previous complete output\n"
GUARDS = {}
TREES = {}


def digest(data):
    return hashlib.sha256(data).hexdigest()


@lru_cache(maxsize=None)
def file_hash(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def bind(path, expected=None):
    path = str(Path(path).absolute())
    actual = file_hash(path)
    assert actual == GUARDS.setdefault(path, actual), (path, "conflicting identity")
    assert expected is None or actual == expected, (path, "SHA-256 differs")
    return actual


def identity(path):
    return {"sha256": bind(path), "bytes": Path(path).stat().st_size}


def tree(directory):
    directory = Path(directory)
    assert directory.is_dir() and not directory.is_symlink(), directory
    files, directories = {}, []
    for path in sorted(directory.rglob("*")):
        assert not path.is_symlink(), path
        name = path.relative_to(directory).as_posix()
        if path.is_dir():
            directories.append(name)
        else:
            assert path.is_file(), path
            files[name] = identity(path)
    result = {"files": files, "directories": directories}
    assert result == TREES.setdefault(str(directory.absolute()), result), (directory, "tree changed")
    return result


def read_json(path):
    return json.loads(read_bytes(path))


def read_bytes(path):
    """Small I/O boundary shared by live auditing and archived evidence replay."""
    bind(path)
    return Path(path).read_bytes()


def api(name):
    path = HERE / (name + "_validate.py")
    bind(path)
    return SimpleNamespace(**runpy.run_path(str(path)))


def summaries(samples):
    """Inclusive quartiles from integer nanoseconds, without harness statistics."""
    result = {}
    for engine in ENGINES:
        values = sorted(sample[engine]["elapsed_ns"] for sample in samples)
        assert all(type(value) is int and value > 0 for value in values)
        def percentile(numerator, denominator):
            lower, remainder = divmod((len(values) - 1) * numerator, denominator)
            upper = min(lower + 1, len(values) - 1)
            return (values[lower] * (denominator - remainder) + values[upper] * remainder) / denominator / 1e9
        result[engine] = {"median_seconds": percentile(1, 2), "min_seconds": values[0] / 1e9,
                          "max_seconds": values[-1] / 1e9, "p25_seconds": percentile(1, 4),
                          "p75_seconds": percentile(3, 4)}
    return result


def close(actual, expected):
    if isinstance(expected, dict):
        assert actual.keys() == expected.keys(), (actual.keys(), expected.keys())
        for key in expected:
            close(actual[key], expected[key])
    else:
        assert math.isfinite(actual) and math.isclose(actual, expected, rel_tol=1e-12, abs_tol=1e-15), (actual, expected)


def balanced(orders):
    assert len(orders) == 10 and len({tuple(order) for order in orders}) == 10
    assert all(len(order) == 5 and set(order) == set(ENGINES) for order in orders)
    for position in range(5):
        assert Counter(order[position] for order in orders) == dict.fromkeys(ENGINES, 2)
    assert Counter(pair for order in orders for pair in zip(order, order[1:])) == {
        (left, right): 2 for left in ENGINES for right in ENGINES if left != right}


def function_bodies(source, restore_calls=False, show_empty=False):
    tree = ast.parse(source)
    if restore_calls:
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
                node.func.id = {"native_python_int": "int", "native_python_sorted": "sorted"}.get(node.func.id, node.func.id)
    result = {}
    def visit(nodes, prefix):
        for node in nodes:
            if isinstance(node, (ast.FunctionDef, ast.ClassDef)):
                name = prefix + node.name
                if isinstance(node, ast.FunctionDef):
                    result[name] = digest(ast.dump(ast.Module(body=node.body, type_ignores=[]), include_attributes=False, show_empty=show_empty).encode())
                visit(node.body, name + ".")
    visit(tree.body, "")
    return result


def provenance(prepared):
    paths, hashes = prepared["source_and_tool_paths"], prepared["source_and_tool_sha256"]
    assert paths.keys() == hashes.keys()
    for name, path in paths.items():
        bind(path, hashes[name])
    for path, expected in prepared["retained_sha256"].items():
        bind(path, expected)
    for package in PACKAGES.values():
        upstream = ROOT / "examples" / package / "upstream"
        for name, expected in read_json(upstream / "UPSTREAM.json")["files"].items():
            bind(upstream / name, expected)
    build = prepared["build_records"]["relar"]
    assert read_json(paths["relar_build_record"]) == build
    assert build["status"] == "passed" and build["input_guards_passed"]
    assert build["source_matches_revision"] or prepared["dirty"]
    assert build["target"] == prepared["effective_target"]
    bind(paths["compiler"], build["sha256"]["compiler"])
    bind(paths["runtime"], build["sha256"]["thinlto_runtime" if prepared["thinlto"] else "normal_runtime"])
    bind(paths["relar_builder"], build["driver_sha256"])
    bind(paths["relar_retained_builder"], build["driver_sha256"])
    for name, expected in build["source_sha256"].items():
        bind(ROOT / name, expected)
    for item in (build, *prepared["build_records"]["rust"].values()):
        settings = item["environment"]
        assert settings["RUSTC"] == item["tool_paths"]["rustc"]
        assert settings["RUSTC_WRAPPER"] == settings["RUSTC_WORKSPACE_WRAPPER"] == ""
        assert settings["CARGO_PROFILE_RELEASE_OPT_LEVEL"] == "3"
        assert settings["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"] == "16"
        for name, expected in item["tool_sha256"].items():
            bind(item["tool_paths"][name], expected)
    assert build["environment"]["CARGO_PROFILE_RELEASE_LTO"] == "off"
    assert build["environment"]["RUSTFLAGS"] == ""
    for step in build["steps"]:
        assert step["returncode"] == 0
        assert step["command"][step["command"].index("--target") + 1] == build["target"]
        for kind in ("log", "messages"):
            bind(step[kind], step[kind + "_sha256"])
    for tool, package in PACKAGES.items():
        rust = prepared["build_records"]["rust"][tool]
        assert read_json(paths[tool + "_rust_build_record"]) == rust and rust["input_guards_passed"]
        assert rust["thinlto"] == prepared["thinlto"]
        assert rust["target"] == rust["environment"]["CARGO_BUILD_TARGET"] == prepared["effective_target"]
        assert rust["command"][rust["command"].index("--target") + 1] == rust["target"]
        assert rust["environment"]["CARGO_PROFILE_RELEASE_LTO"] == ("thin" if prepared["thinlto"] else "off")
        assert rust["llvm_budget"] == (1000 if prepared["thinlto"] else None)
        assert "target-cpu=native" not in rust["build_stderr"]
        bind(paths[tool + "_rust"], rust["binary_sha256"])
        for name, expected in rust["sources"].items():
            bind(HERE / "reference" / package / name, expected)
            assert digest(rust["source_contents"][name].encode()) == expected
        codon = prepared["build_records"]["codon"][tool]
        assert read_json(paths[tool + "_codon_record"]) == codon
        assert codon["status"] == "passed" and codon["release"] == "v0.20.4"
        assert codon["domain"]["python_interop"] is False
        assert codon["environment_overrides"]["CODON_PYTHON"] == "/nonexistent/relar-native-codon-libpython.so"
        bind(codon["compiler"], codon["compiler_sha256"])
        for name, expected in codon["source_inputs"].items():
            bind(name, expected)
        for name, expected in codon["generated"].items():
            bind(Path(codon["environment_overrides"]["CODON_PATH"]) / name, expected)
        for name, expected in codon["application_function_bodies"].items():
            original = read_bytes(ROOT / "examples" / name).decode()
            generated = read_bytes(Path(codon["environment_overrides"]["CODON_PATH"]) / Path(name).with_suffix(".codon")).decode()
            # Python 3.13 changed ast.dump's default handling of empty fields.
            show_empty = function_bodies(original) != expected
            assert function_bodies(original, show_empty=show_empty) == expected
            actual = function_bodies(generated, show_empty=show_empty)
            restored = function_bodies(generated, True, show_empty=show_empty)
            assert {key: actual[key] for key in expected} == codon["generated_application_function_bodies"][name]
            assert all(restored[key] == value for key, value in expected.items()), (name, "undeclared application-body adaptation")
        for item in (*codon["libraries"].values(), codon["native_json"]):
            bind(item["path"], item["sha256"])
        for suffix in ("", "-compute"):
            item = codon["binaries"][package + suffix]
            bind(paths[tool + "_codon" + suffix], item["sha256"])
            command = next(value for value in codon["commands"] if value["name"] == package + suffix)
            assert command["returncode"] == 0
            assert all(flag in command["command"] for flag in ("-release", "-numerics=py", "-disable-native", "-exe"))
            for stream in ("stdout", "stderr"):
                bind(command[stream]["path"], command[stream]["sha256"])
        app = prepared["applications"][tool]
        for prefix, source in (("", "__main__.py"), ("compute_", "compute.py")):
            command = [paths["compiler"], "build", str(ROOT / "examples" / package / source),
                       "--module-root", str(ROOT / "examples"), "-o", app[prefix + "binary"],
                       "--runtime", paths["runtime"], "--clang", paths["clang"], "--opt-level", "3",
                       "--emit-llvm", app[prefix + "llvm"], "--target", prepared["effective_target"]]
            if prepared["thinlto"]:
                command += ["--lto-linker", paths["lto_linker"]]
            assert app[prefix + "compile_command"] == command and app[prefix + "compile_stdout"] == ""
            for path, expected in app[prefix + "dynamic_libraries"].items():
                assert identity(path) == expected
    for libraries in prepared["build_records"]["dynamic_libraries"].values():
        for path, expected in libraries.items():
            assert identity(path) == expected


def descriptions(prepared, catalog, conformance):
    inputs, rows = prepared["inputs"], []
    assert set(inputs) == {"catalog", "conformance-unchanged", "conformance-changed"}
    for item in inputs.values():
        assert tree(item["directory"]) == item["tree"]
    source = Path(inputs["catalog"]["directory"]) / "index.ndjson"
    assert read_bytes(source) == catalog.verify_provenance()
    assert inputs["catalog"]["metadata"] == read_json(catalog.PACKAGE / "sample/provenance.json")
    base = catalog.Case("catalog-real", {"catalog.json": SENTINEL}, (str(source), "catalog.json"))
    for rounds in (None, 0, prepared["compute_rounds"]):
        value = base if rounds is None else catalog.compute_case(base, rounds)
        rows.append({"label": value.label, "tool": "catalog", "input_set": "catalog", "rounds": rounds,
                     "arguments": list(value.arguments), "output": "catalog.json", "case": catalog.case_record(value)})
    for role in ("unchanged", "changed"):
        key = "conformance-" + role
        directory = Path(inputs[key]["directory"])
        base = conformance.Workload(directory / "manifest.json", directory / "old.json", directory / "new.json")
        assert conformance.workload_metadata(base) == inputs[key]["metadata"]
        capture = read_json(prepared["source_and_tool_paths"]["conformance_" + role + ":preparation.json"])
        assert capture["guards_passed"] and capture["suite_revision"] == conformance.REVISION
        original_manifest_path = prepared["source_and_tool_paths"]["conformance_" + role + ":manifest.json"]
        bind(original_manifest_path, capture["manifest_sha256"])
        original_manifest = read_json(original_manifest_path)
        original_root = Path(capture["suite_directory"])
        def rebase(path):
            return str(directory / "sources" / Path(path).relative_to(original_root))
        expected_manifest = {**original_manifest,
            "test_files": [rebase(path) for path in original_manifest["test_files"]],
            "diagnostic_paths": [{**alias, "path": rebase(alias["path"])} for alias in original_manifest["diagnostic_paths"]]}
        assert read_json(base.manifest) == expected_manifest
        for path in original_manifest["test_files"]:
            bind(rebase(path), capture["source_sha256"][Path(path).relative_to(original_root).as_posix()])
        for which in ("old", "new"):
            assert identity(directory / (which + ".json")) == {
                "sha256": capture["captures"][which]["stdout_sha256"], "bytes": capture["captures"][which]["stdout_bytes"]}
        assert (file_hash(str(base.old)) == file_hash(str(base.new))) == (role == "unchanged")
        for format in (("github",) if role == "unchanged" else ("github", "diff")):
            workload = conformance.Workload(base.manifest, base.old, base.new, format, True)
            for rounds in (None, 0, prepared["compute_rounds"]):
                label = key + "-" + format + ("" if rounds is None else "-compute-" + str(rounds))
                arguments = workload.arguments() + ["--output", "report.md"]
                if rounds is not None:
                    arguments += ["--rounds", str(rounds)]
                rows.append({"label": label, "tool": "conformance", "input_set": key, "rounds": rounds,
                             "format": format, "arguments": arguments, "output": "report.md",
                             "workload": {name: str(getattr(workload, name)) for name in ("manifest", "old", "new")}})
    assert len(rows) == len(prepared["workloads"]) == 12
    for expected, row in zip(rows, prepared["workloads"], strict=True):
        assert {key: value for key, value in row.items() if key not in ("gates", "expected_output", "expected_stdout")} == expected
    return rows


def commands(prepared, row, catalog, conformance):
    paths, app = prepared["source_and_tool_paths"], prepared["applications"][row["tool"]]
    compute = row["rounds"] is not None
    if row["tool"] == "catalog":
        port, original = catalog.python_command(paths["python"], compute), catalog.original_command(paths["python"], compute)
    else:
        port = (conformance.python_compute_command if compute else conformance.python_command)(paths["python"])
        original = (conformance.original_compute_command if compute else conformance.original_command)(paths["python"])
    result = {"relar": [app["compute_binary" if compute else "binary"]], "port": port, "original": original,
              "codon": [paths[row["tool"] + "_codon" + ("-compute" if compute else "")]],
              "rust": [paths[row["tool"] + "_rust"], *(["--compute"] if compute else [])]}
    assert all(result[name][1:4] == ["-P", "-s", "-B"] for name in ("port", "original"))
    return {engine: [*command, *row["arguments"]] for engine, command in result.items()}


def expected_output(row, catalog, conformance):
    if row["tool"] == "catalog":
        value = catalog.case_from_record(row["case"])
        expected = (catalog.oracle if row["rounds"] is None else catalog.compute_oracle)(value)
        assert expected["error"] is None
        return expected["files"][row["output"]], expected.get("stdout", "")
    workload = conformance.Workload(*(Path(row["workload"][name]) for name in ("manifest", "old", "new")), row["format"], True)
    expected = conformance.oracle(workload) if row["rounds"] is None else conformance.compute_oracle(workload, row["rounds"])
    assert expected.error is None and expected.report is not None
    return expected.report.encode(), "" if row["rounds"] is None else expected.stdout.decode()


def capture(value, command, row, report, stdout, directories):
    assert value["command"] == command and value["returncode"] == 0
    assert value["stdout"] == stdout and value["stderr"] == ""
    expected = {"files": {row["output"]: {"sha256": digest(report), "bytes": len(report)}}, "directories": []}
    assert {key: value[key] for key in expected} == expected
    assert type(value["elapsed_ns"]) is int and value["elapsed_ns"] > 0
    directory = value["cwd"]
    if directory not in directories:
        assert tree(directory) == expected
        assert read_bytes(Path(directory) / row["output"]) == report
        directories.add(directory)


def validation_records(prepared):
    counts = {}
    for name, item in prepared["validations"].items():
        value = read_json(item["path"])
        if name.startswith("catalog"):
            assert value["cases"] == item["cases"] == len(value["results"])
            assert value["engine_executions"] == item["engine_executions"] == sum(len(case["engines"]) for case in value["results"])
            assert set(value["engines"]) == set(ENGINES)
            counts[name] = value["engine_executions"]
        else:
            assert name == "conformance" and value["status"] == "passed" and value["guards_passed"]
            actual = Counter(check["kind"] for check in value["checks"])
            assert actual == value["counts"] == item["counts"]
            assert value["parser_differentials"] == item["parser_differentials"] == 30000
            assert value["report_differentials"] == item["report_differentials"] == 400
            for group in ("source_sha256", "binary_sha256"):
                for path, expected in value[group].items():
                    bind(path, expected)
            counts[name] = len(value["checks"])
    return counts


def audit_mode(record, catalog, conformance):
    assert record["kind"] == "native-tools-benchmark" and record["input_guards_passed"]
    prepared = record["preparation"]
    assert prepared["kind"] == "native-tools-preparation" and prepared["input_guards_passed"]
    assert record["schema"] == prepared["schema"] == 1
    assert datetime.fromisoformat(prepared["completed_at_utc"]) <= datetime.fromisoformat(record["started_at_utc"]) <= datetime.fromisoformat(record["completed_at_utc"])
    assert digest((json.dumps(prepared, indent=2) + "\n").encode()) == record["preparation_record_sha256"]
    assert record["revision"] == prepared["revision"] and record["dirty"] == prepared["dirty"]
    assert record["compute_rounds"] == prepared["compute_rounds"] > 0
    assert record["samples"] > 0 and record["samples"] % 10 == 0 and record["warmups"] > 0
    assert record["python"] == prepared["python"] and record["python"]["implementation"] == "cpython"
    assert tuple(map(int, record["python"]["version_number"].split(".")[:2])) >= (3, 14)
    assert record["python"]["jit_enabled"] is False
    assert record["environment"] == {"PATH": "/usr/bin:/bin", "HOME": "<engine-working-directory>/home",
        "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "TZ": "UTC", "PYTHONHASHSEED": "0", "PYTHON_JIT": "0",
        "CODON_PYTHON": "/nonexistent/relar-native-codon-libpython.so", "PYTHONPATH": ""}
    assert len(record["cpu"]["affinity"]) == 1
    assert record["cpu"]["affinity"][0] in record["cpu"]["allowed_cpus_before_pinning"]
    provenance(prepared)
    descriptions(prepared, catalog, conformance)
    balanced(record["engine_orders"])
    assert len(record["workloads"]) == len(prepared["workloads"])
    counts = {"preparation_gates": 0, "timing_gates": 0, "warmups": 0, "samples": 0}
    rows, controls, directories = [], {}, set()
    for row, measured in zip(prepared["workloads"], record["workloads"], strict=True):
        assert all(measured[key] == value for key, value in row.items() if key != "gates")
        assert measured["engine_orders"] == record["engine_orders"]
        report, stdout = expected_output(row, catalog, conformance)
        assert row["expected_output"] == {"sha256": digest(report), "bytes": len(report)}
        assert row["expected_stdout"] == stdout
        selected = commands(prepared, row, catalog, conformance)
        for kind, gates in (("preparation_gates", row["gates"]), ("timing_gates", measured["gates"])):
            assert set(gates) == set(ENGINES)
            for engine, value in gates.items():
                capture(value, selected[engine], row, report, stdout, directories)
                counts[kind] += 1
        for phase in ("warmups", "samples"):
            assert len(measured[phase]) == record[phase]
            for index, sample in enumerate(measured[phase]):
                assert sample["index"] == index and sample["order"] == record["engine_orders"][index % 10]
                assert [name for name in sample if name in ENGINES] == sample["order"]
                for engine in sample["order"]:
                    capture(sample[engine], selected[engine], row, report, stdout, directories)
                    assert {key: value for key, value in sample[engine].items() if key != "elapsed_ns"} == {
                        key: value for key, value in measured["gates"][engine].items() if key != "elapsed_ns"}
                    counts[phase] += 1
        stats = summaries(measured["samples"])
        ratios = {left + "_over_" + right: stats[left]["median_seconds"] / stats[right]["median_seconds"] for left, right in PAIRS}
        close(measured["summary"], stats)
        close(measured["ratios"], ratios)
        result = {"label": row["label"], "tool": row["tool"], "rounds": row["rounds"], "summary": stats, "ratios": ratios,
                  "iqr_seconds": {engine: stats[engine]["p75_seconds"] - stats[engine]["p25_seconds"] for engine in ENGINES}}
        group = (row["tool"], row["input_set"], row.get("format"))
        if row["rounds"] == 0:
            controls[group] = result
        elif row["rounds"]:
            amortized = {engine: stats[engine]["median_seconds"] / row["rounds"] for engine in ENGINES}
            fractions = {engine: controls[group]["summary"][engine]["median_seconds"] / stats[engine]["median_seconds"] for engine in ENGINES}
            assert measured["preload_control"] == controls[group]["label"]
            close(measured["amortized_seconds"], amortized)
            close(measured["preload_control_fraction"], fractions)
            result.update(amortized_seconds=amortized, preload_control_fraction=fractions)
        else:
            assert "amortized_seconds" not in measured
        rows.append(result)
    return {"rows": rows, "counts": counts, "validation_executions": validation_records(prepared),
            "revision": record["revision"], "dirty": record["dirty"]}


def cross_mode_input_parity(records):
    """Require identical work and shared source/tool identities across modes."""
    if len(records) != 2:
        return None
    left, right = (record["preparation"] for record in records)
    assert left["revision"] == right["revision"] and left["dirty"] == right["dirty"]
    assert left["inputs"] == right["inputs"]
    assert [{key: value for key, value in row.items() if key != "gates"} for row in left["workloads"]] == [
        {key: value for key, value in row.items() if key != "gates"} for row in right["workloads"]]
    def source_key(name):
        return (name.startswith(("source:", "helper:", "compiler_source:", "conformance_unchanged", "conformance_changed"))
                or "_rust_source:" in name or "_codon_source:" in name)
    assert {name for name in left["source_and_tool_sha256"] if source_key(name)} == {
        name for name in right["source_and_tool_sha256"] if source_key(name)}
    for name in left["source_and_tool_sha256"].keys() & right["source_and_tool_sha256"].keys():
        if (source_key(name) or "_rust_tool:" in name
                or name in ("python", "clang", "compiler", "catalog_codon", "catalog_codon-compute", "conformance_codon", "conformance_codon-compute")):
            assert left["source_and_tool_sha256"][name] == right["source_and_tool_sha256"][name], name
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("records", nargs="+", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    assert not output.exists(), "use a fresh audit output outside retained input/output trees"
    records = [read_json(path) for path in args.records]
    assert 1 <= len(records) <= 2
    for record in records:
        prepared = record["preparation"]
        roots = [ROOT, *(Path(item["directory"]) for item in prepared["inputs"].values()),
                 *(Path(app["binary"]).parent for app in prepared["applications"].values()),
                 *(Path(row["gates"]["relar"]["cwd"]).parents[1] for row in record["workloads"])]
        assert all(not output.is_relative_to(path.resolve()) for path in roots)
    result = {"schema": 1, "kind": "native-tools-audit", "status": "failed",
              "audit_script_sha256": bind(__file__), "records": {str(path.resolve()): bind(path) for path in args.records},
              "measurement_revision": records[0]["revision"],
              "modes": {}, "cross_mode_input_parity": None}
    try:
        catalog, conformance = api("catalog"), api("conformance")
        for record in records:
            mode = "thinlto" if record["preparation"]["thinlto"] else "normal"
            assert mode not in result["modes"]
            result["modes"][mode] = audit_mode(record, catalog, conformance)
        result["cross_mode_input_parity"] = cross_mode_input_parity(records)
        file_hash.cache_clear()
        for directory, expected in list(TREES.items()):
            assert tree(directory) == expected
        assert all(file_hash(path) == expected for path, expected in GUARDS.items()), "audit input changed"
        result.update(status="passed", guarded_inputs=GUARDS, guarded_trees=TREES, input_guards_passed=True)
    except BaseException as error:
        result["error"] = repr(error)
        raise
    finally:
        result["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "modes": list(result["modes"]),
                      "cross_mode_input_parity": result["cross_mode_input_parity"], "output": str(output)}, indent=2))


if __name__ == "__main__":
    main()

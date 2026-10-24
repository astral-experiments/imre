"""Frozen five-engine comparisons for the complete catalog and conformance tools."""

import argparse
from collections import Counter
from datetime import datetime, timezone
import json
from pathlib import Path
import runpy
import subprocess
import sys
import time
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit("benchmark validation requires Python assertions; do not use -O")

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
catalog = SimpleNamespace(**runpy.run_path(str(HERE / "catalog_validate.py")))
conformance = SimpleNamespace(**runpy.run_path(str(HERE / "conformance_validate.py")))
ENGINES = ("imre", "port", "original", "codon", "rust")
PACKAGES = {"catalog": "python_catalog", "conformance": "conformance_report"}
SENTINEL = b"replace this previous complete output\n"


def utc():
    return datetime.now(timezone.utc).isoformat()


def engine_orders():
    """Ten Williams orders balance positions and directed within-round neighbors."""
    forward = [tuple(ENGINES[(index + rotation) % 5] for index in (0, 1, 4, 2, 3))
               for rotation in range(5)]
    orders = [*forward, *(tuple(reversed(order)) for order in forward)]
    assert all(Counter(order[position] for order in orders) == dict.fromkeys(ENGINES, 2)
               for position in range(5))
    neighbors = Counter(pair for order in orders for pair in zip(order, order[1:]))
    assert neighbors == {(left, right): 2 for left in ENGINES for right in ENGINES if left != right}
    return orders


def environment(directory):
    return {**conformance.environment(directory / "home"), "PYTHONPATH": ""}


def checked(command, timeout=120):
    result = subprocess.run(list(map(str, command)), input=b"", capture_output=True,
                            timeout=timeout, env=environment(Path("/tmp")), check=False)
    if result.returncode or result.stderr:
        raise RuntimeError((command, result.returncode, result.stdout, result.stderr))
    return result.stdout.decode()


def file_identity(path):
    return {"sha256": shared.sha256(path), "bytes": path.stat().st_size}


def tree_identity(directory):
    files, directories = {}, []
    for path in sorted(directory.rglob("*")):
        assert not path.is_symlink(), (path, "unexpected symlink")
        name = path.relative_to(directory).as_posix()
        if path.is_dir():
            directories.append(name)
        else:
            assert path.is_file(), (path, "unexpected filesystem entry")
            files[name] = file_identity(path)
    return {"files": files, "directories": directories}


def linked_libraries(binary):
    result = subprocess.run(["ldd", str(binary)], input=b"", capture_output=True,
                            env=environment(Path("/tmp")), check=False, timeout=30)
    assert result.returncode == 0 and not result.stderr, (binary, result.stderr)
    libraries = {}
    for line in result.stdout.decode().splitlines():
        assert "not found" not in line, (binary, line)
        value = line.split("=>", 1)[-1].strip().split(" (", 1)[0]
        if value.startswith("/"):
            path = Path(value).resolve(strict=True)
            libraries[str(path)] = file_identity(path)
    return libraries


def elf_target(path):
    with path.open("rb") as stream:
        header = stream.read(20)
    assert len(header) == 20 and header[:5] == b"\x7fELF\x02"
    byte_order = {1: "little", 2: "big"}[header[5]]
    return {"bits": 64, "byte_order": byte_order,
            "machine": int.from_bytes(header[18:20], byte_order)}


def source_paths(args):
    paths = {name: Path(shared.executable(getattr(args, name))) for name in ("compiler", "python", "clang")}
    paths["runtime"] = args.runtime.resolve(strict=True)
    paths["imre_build_record"] = args.imre_build_record.resolve(strict=True)
    build = json.loads(paths["imre_build_record"].read_text())
    assert build["kind"] == "native-tools-build" and build["status"] == "passed" and build["input_guards_passed"]
    assert build["source_matches_revision"] or args.allow_dirty, "dirty compiler/runtime snapshots require --allow-dirty"
    for name, path in (("imre_builder", HERE / "native_tools_build.py"),
                       ("imre_retained_builder", paths["imre_build_record"].parent / "build.py")):
        assert shared.sha256(path) == build["driver_sha256"]
        paths[name] = path
    for name, expected in build["tool_sha256"].items():
        path = Path(build["tool_paths"][name]).resolve(strict=True)
        assert shared.sha256(path) == expected
        paths["imre_build_tool:" + name] = path
    settings = build["environment"]
    assert settings["RUSTC"] == build["tool_paths"]["rustc"]
    assert settings["RUSTC_WRAPPER"] == settings["RUSTC_WORKSPACE_WRAPPER"] == ""
    assert settings["CARGO_PROFILE_RELEASE_OPT_LEVEL"] == "3"
    assert settings["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"] == "16"
    assert settings["CARGO_PROFILE_RELEASE_LTO"] == "off" and settings["RUSTFLAGS"] == ""
    for step in build["steps"]:
        assert step["returncode"] == 0
        assert step["command"][step["command"].index("--target") + 1] == build["target"]
        for kind in ("log", "messages"):
            path = Path(step[kind]).resolve(strict=True)
            assert shared.sha256(path) == step[kind + "_sha256"]
            paths["imre_build:" + step["name"] + ":" + kind] = path
    assert build["sha256"]["compiler"] == shared.sha256(paths["compiler"])
    runtime_key = "thinlto_runtime" if args.lto_linker else "normal_runtime"
    assert build["sha256"][runtime_key] == shared.sha256(paths["runtime"])
    for name, expected in build["source_sha256"].items():
        path = ROOT / name
        assert shared.sha256(path) == expected, (name, "runtime/compiler source differs")
        paths["compiler_source:" + name] = path
    if args.lto_linker:
        # The Rust toolchain's ld.lld symlink selects the GNU driver flavor.
        paths["lto_linker"] = args.lto_linker.absolute()
        assert paths["lto_linker"].is_file()
    helpers = ("native_tools_benchmark.py", "benchmark.py", "validate.py", "catalog_validate.py",
               "conformance_validate.py", "manifest_validate.py", "batch_validate.py", "incremental_validate.py")
    paths.update({"helper:" + name: HERE / name for name in helpers})
    for directory in (ROOT / "stdlib", HERE / "codon_native",
                      *(ROOT / "examples" / package for package in PACKAGES.values())):
        for path in directory.rglob("*"):
            if path.is_file() and "__pycache__" not in path.parts:
                paths["source:" + path.relative_to(ROOT).as_posix()] = path
    records = {"imre": build, "rust": {}, "codon": {}}
    targets = set()
    for tool, package in PACKAGES.items():
        reference = HERE / "reference" / package
        paths[tool + "_rust_builder"] = reference / "build.py"
        binary = Path(shared.executable(getattr(args, tool + "_rust")))
        record_path = binary.with_name("build-record.json")
        record = json.loads(record_path.read_text())
        paths[tool + "_rust"], paths[tool + "_rust_build_record"] = binary, record_path
        assert record["input_guards_passed"] and record["binary_sha256"] == shared.sha256(binary)
        for name, expected in record["sources"].items():
            path = reference / name
            assert shared.sha256(path) == expected, (tool, name, "Rust source differs")
            paths[tool + "_rust_source:" + name] = path
        settings = record["environment"]
        assert settings["CARGO_PROFILE_RELEASE_OPT_LEVEL"] == "3"
        assert settings["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"] == "16"
        assert settings["CARGO_PROFILE_RELEASE_LTO"] == ("thin" if args.lto_linker else "off")
        assert record["thinlto"] == bool(args.lto_linker)
        assert record["llvm_budget"] == (1000 if args.lto_linker else None)
        assert "target-cpu=native" not in record["build_stderr"]
        host = next(line[6:] for line in record["rustc_verbose"].splitlines() if line.startswith("host: "))
        assert record["target"] == settings["CARGO_BUILD_TARGET"] == host
        assert record["command"][record["command"].index("--target") + 1] == host
        assert settings["RUSTC_WRAPPER"] == settings["RUSTC_WORKSPACE_WRAPPER"] == ""
        assert settings["RUSTC"] == record["tool_paths"]["rustc"]
        for name, expected in record["tool_sha256"].items():
            path = Path(record["tool_paths"][name]).resolve(strict=True)
            assert shared.sha256(path) == expected
            paths[tool + "_rust_tool:" + name] = path
        targets.add(host)
        records["rust"][tool] = record
        codon_record = getattr(args, tool + "_codon_record").resolve(strict=True)
        paths[tool + "_codon_record"] = codon_record
        record = json.loads(codon_record.read_text())
        assert record["status"] == "passed" and record["release"] == "v0.20.4"
        assert record["domain"]["python_interop"] is False
        compiler = Path(record["compiler"]).resolve(strict=True)
        assert shared.sha256(compiler) == record["compiler_sha256"]
        paths[tool + "_codon_compiler"] = compiler
        for name, expected in record["source_inputs"].items():
            path = Path(name).resolve(strict=True)
            assert shared.sha256(path) == expected, (name, "Codon source differs")
            paths[tool + "_codon_source:" + name] = path
        generated = Path(record["environment_overrides"]["CODON_PATH"])
        for name, expected in record["generated"].items():
            path = generated / name
            assert shared.sha256(path) == expected, (name, "Codon generated source differs")
            paths[tool + "_codon_generated:" + name] = path
        native_json = Path(record["native_json"]["path"])
        assert shared.sha256(native_json) == record["native_json"]["sha256"]
        paths[tool + "_codon_json"] = native_json
        for suffix in ("", "-compute"):
            item = record["binaries"][package + suffix]
            path = Path(item["path"]).resolve(strict=True)
            assert shared.sha256(path) == item["sha256"]
            paths[tool + "_codon" + suffix] = path
            command = next(item["command"] for item in record["commands"] if item["name"] == package + suffix)
            assert all(flag in command for flag in ("-release", "-numerics=py", "-disable-native", "-exe"))
        records["codon"][tool] = record
    assert len(targets) == 1
    target = targets.pop()
    assert build["target"] == target
    assert args.target is None or args.target == target
    for role in ("unchanged", "changed"):
        directory = getattr(args, "conformance_" + role).resolve(strict=True)
        for name in ("preparation.json", "manifest.json", "old.json", "new.json", "old.stderr", "new.stderr"):
            paths["conformance_" + role + ":" + name] = directory / name
        record = json.loads((directory / "preparation.json").read_text())
        paths["conformance_" + role + "_license"] = Path(record["suite_directory"]).parent / "LICENSE"
        for name, expected in record["source_sha256"].items():
            path = Path(record["suite_directory"]) / name
            assert shared.sha256(path) == expected
            paths["conformance_" + role + "_suite:" + name] = path
        for name, item in record["tools"].items():
            path = Path(item["path"]).resolve(strict=True)
            assert shared.sha256(path) == item["sha256"]
            paths["conformance_" + role + "_capture_tool:" + name] = path
    libraries = {}
    for name in ("python", "compiler", "clang", "catalog_rust", "conformance_rust",
                 "catalog_codon", "catalog_codon-compute", "conformance_codon", "conformance_codon-compute"):
        libraries[name] = linked_libraries(paths[name])
        for path in libraries[name]:
            paths["library:" + path] = Path(path)
    records["dynamic_libraries"] = libraries
    if args.workload_preparation:
        path = args.workload_preparation.resolve(strict=True)
        previous = json.loads(path.read_text())
        assert previous["input_guards_passed"] and previous["kind"] == "native-tools-preparation"
        assert previous["compute_rounds"] == args.compute_rounds
        paths["workload_preparation"] = path
        for name, item in previous["inputs"].items():
            directory = Path(item["directory"])
            assert tree_identity(directory) == item["tree"]
            for filename in item["tree"]["files"]:
                paths["prepared_input:" + name + ":" + filename] = directory / filename
    return paths, records, target


def commands_for(prepared, paths, tool, compute=False):
    app = prepared["applications"][tool]
    if tool == "catalog":
        port = catalog.python_command(paths["python"], compute)
        original = catalog.original_command(paths["python"], compute)
    else:
        port = (conformance.python_compute_command if compute else conformance.python_command)(paths["python"])
        original = (conformance.original_compute_command if compute else conformance.original_command)(paths["python"])
    return {"imre": [app["compute_binary" if compute else "binary"]], "port": port, "original": original,
            "codon": [str(paths[tool + "_codon" + ("-compute" if compute else "")])],
            "rust": [str(paths[tool + "_rust"]), *(["--compute"] if compute else [])]}


def prepare_workloads(args, artifact):
    inputs, rows = {}, []
    directory = artifact / "inputs" / "catalog"
    directory.mkdir(parents=True)
    source = directory / "index.ndjson"
    source.write_bytes(catalog.verify_provenance())
    inputs["catalog"] = {"directory": str(directory), "tree": tree_identity(directory),
                         "metadata": json.loads((catalog.PACKAGE / "sample/provenance.json").read_text())}
    base = catalog.Case("catalog-real", {"catalog.json": SENTINEL}, (str(source), "catalog.json"))
    for rounds in (None, 0, args.compute_rounds):
        value = base if rounds is None else catalog.compute_case(base, rounds)
        rows.append({"label": value.label, "tool": "catalog", "input_set": "catalog", "rounds": rounds,
                     "arguments": list(value.arguments), "output": "catalog.json", "case": catalog.case_record(value)})
    for role in ("unchanged", "changed"):
        directory = artifact / "inputs" / ("conformance-" + role)
        base = conformance.materialize_real(directory, getattr(args, "conformance_" + role))
        metadata = conformance.workload_metadata(base)
        assert (metadata["old_sha256"] == metadata["new_sha256"]) == (role == "unchanged")
        key = "conformance-" + role
        inputs[key] = {"directory": str(directory), "tree": tree_identity(directory), "metadata": metadata}
        for format in (("github",) if role == "unchanged" else ("github", "diff")):
            workload = conformance.Workload(base.manifest, base.old, base.new, format, True)
            for rounds in (None, 0, args.compute_rounds):
                label = key + "-" + format + ("" if rounds is None else "-compute-" + str(rounds))
                arguments = workload.arguments() + ["--output", "report.md"]
                if rounds is not None:
                    arguments += ["--rounds", str(rounds)]
                rows.append({"label": label, "tool": "conformance", "input_set": key,
                             "rounds": rounds, "format": format, "arguments": arguments, "output": "report.md",
                             "workload": {name: str(getattr(workload, name)) for name in ("manifest", "old", "new")}})
    return inputs, rows


def expected_output(row):
    compute = row["rounds"] is not None
    if row["tool"] == "catalog":
        value = catalog.case_from_record(row["case"])
        expected = (catalog.compute_oracle if compute else catalog.oracle)(value)
        assert expected["error"] is None
        report = expected["files"][row["output"]]
        stdout = expected.get("stdout", "").encode()
        return report, stdout
    workload = conformance.Workload(*(Path(row["workload"][name]) for name in ("manifest", "old", "new")),
                                   row["format"], True)
    expected = conformance.compute_oracle(workload, row["rounds"]) if compute else conformance.oracle(workload)
    assert expected.error is None and expected.report is not None
    return expected.report.encode(), expected.stdout if compute else b""


def execute(row, command, directory, expected, input_set, timeout):
    report, stdout = expected
    output = directory / row["output"]
    output.write_bytes(SENTINEL)
    command = [*command, *row["arguments"]]
    started = time.perf_counter_ns()
    process = subprocess.run(command, input=b"", capture_output=True, cwd=directory,
                             env=environment(directory), timeout=timeout, check=False)
    elapsed = time.perf_counter_ns() - started
    assert process.returncode == 0 and not process.stderr, (row["label"], command, process.returncode, process.stderr)
    assert process.stdout == stdout, (row["label"], command, "stdout differs")
    assert output.read_bytes() == report, (row["label"], command, "complete output differs")
    expected_tree = {"files": {row["output"]: {"sha256": catalog.digest(report), "bytes": len(report)}}, "directories": []}
    assert tree_identity(directory) == expected_tree, (row["label"], command, "output filesystem differs")
    assert tree_identity(Path(input_set["directory"])) == input_set["tree"], "input filesystem changed"
    return {"command": command, "cwd": str(directory), "elapsed_ns": elapsed, "returncode": 0,
            "stdout": process.stdout.decode(), "stderr": "", **expected_tree}


def prepare(args, paths, records, target, hashes, revision, dirty):
    artifact = args.artifact_dir.resolve()
    prepared = {"schema": 1, "kind": "native-tools-preparation", "revision": revision, "dirty": dirty,
                "thinlto": bool(args.lto_linker), "effective_target": target, "compute_rounds": args.compute_rounds,
                "source_and_tool_paths": {name: str(path) for name, path in paths.items()},
                "source_and_tool_sha256": hashes, "build_records": records, "applications": {},
                "input_guards_passed": False}
    for tool, package in PACKAGES.items():
        app = {}
        for compute in (False, True):
            name = tool + ("-compute" if compute else "")
            binary, llvm = artifact / name, artifact / (name + ".ll")
            entry = ROOT / "examples" / package / ("compute.py" if compute else "__main__.py")
            command = [str(paths["compiler"]), "build", str(entry), "--module-root", str(ROOT / "examples"),
                       "-o", str(binary), "--runtime", str(paths["runtime"]), "--clang", str(paths["clang"]),
                       "--opt-level", "3", "--emit-llvm", str(llvm), "--target", target]
            if args.lto_linker:
                command += ["--lto-linker", str(paths["lto_linker"])]
            stdout = checked(command, args.timeout)
            assert elf_target(binary) == elf_target(paths[tool + "_rust"]) == elf_target(paths[tool + "_codon"])
            prefix = "compute_" if compute else ""
            app.update({prefix + "binary": str(binary), prefix + "llvm": str(llvm), prefix + "compile_command": command,
                        prefix + "compile_stdout": stdout,
                        prefix + "dynamic_libraries": linked_libraries(binary)})
        prepared["applications"][tool] = app
    validations = {}
    for compute in (False, True):
        name = "catalog" + ("-compute" if compute else "")
        value = catalog.validate_commands(commands_for(prepared, paths, "catalog", compute), args.timeout, compute=compute)
        output = artifact / (name + "-validation.json")
        output.write_text(json.dumps(value, indent=2) + "\n")
        validations[name] = {"path": str(output), "cases": value["cases"], "engine_executions": value["engine_executions"]}
    app = prepared["applications"]["conformance"]
    value = conformance.validate(argparse.Namespace(
        python=paths["python"], binary=Path(app["binary"]), compute_binary=Path(app["compute_binary"]),
        rust_binary=paths["conformance_rust"], codon_binary=paths["conformance_codon"],
        codon_compute_binary=paths["conformance_codon-compute"],
        prepared=[args.conformance_unchanged, args.conformance_changed], source_root=None,
        retain_dir=artifact / "conformance-validation", asan=False))
    assert value["status"] == "passed" and value["guards_passed"]
    validations["conformance"] = {"path": str(artifact / "conformance-validation/record.json"), "counts": value["counts"],
                                  "parser_differentials": value["parser_differentials"], "report_differentials": value["report_differentials"]}
    prepared["validations"] = validations
    if args.workload_preparation:
        previous = json.loads(args.workload_preparation.read_text())
        assert previous["revision"] == revision and previous["dirty"] == dirty
        for name in paths:
            if (name.startswith(("source:", "helper:", "compiler_source:", "conformance_unchanged", "conformance_changed"))
                    or "_rust_source:" in name or "_codon_source:" in name or name.endswith("_rust_builder")):
                assert previous["source_and_tool_sha256"][name] == hashes[name], (name, "shared workload source differs")
        inputs = previous["inputs"]
        rows = [{key: value for key, value in row.items() if key not in ("gates", "expected_output", "expected_stdout")}
                for row in previous["workloads"]]
        prepared["workload_preparation"] = str(args.workload_preparation.resolve())
    else:
        inputs, rows = prepare_workloads(args, artifact)
    prepared["inputs"], prepared["workloads"] = inputs, rows
    for row in rows:
        expected = expected_output(row)
        row["expected_output"] = {"sha256": catalog.digest(expected[0]), "bytes": len(expected[0])}
        row["expected_stdout"] = expected[1].decode()
        row["gates"] = {}
        for engine, command in commands_for(prepared, paths, row["tool"], row["rounds"] is not None).items():
            directory = artifact / "workload-gates" / row["label"] / engine
            directory.mkdir(parents=True)
            row["gates"][engine] = execute(row, command, directory, expected, inputs[row["input_set"]], args.timeout)
    prepared["retained_sha256"] = {str(path): shared.sha256(path) for path in sorted(artifact.rglob("*")) if path.is_file()}
    return prepared


def verify(prepared, paths, hashes, args, target, revision, dirty):
    assert prepared["kind"] == "native-tools-preparation"
    assert prepared["revision"] == revision and prepared["dirty"] == dirty
    assert prepared["thinlto"] == bool(args.lto_linker) and prepared["effective_target"] == target
    assert prepared["compute_rounds"] == args.compute_rounds
    assert prepared["source_and_tool_paths"] == {name: str(path) for name, path in paths.items()}
    assert prepared["source_and_tool_sha256"] == hashes
    assert all(shared.sha256(Path(path)) == expected for path, expected in prepared["retained_sha256"].items())
    for item in prepared["inputs"].values():
        assert tree_identity(Path(item["directory"])) == item["tree"]
    for app in prepared["applications"].values():
        for prefix in ("", "compute_"):
            assert linked_libraries(Path(app[prefix + "binary"])) == app[prefix + "dynamic_libraries"]


def measure(row, prepared, paths, directory, args):
    expected = expected_output(row)
    assert row["expected_output"] == {"sha256": catalog.digest(expected[0]), "bytes": len(expected[0])}
    assert row["expected_stdout"] == expected[1].decode()
    commands = commands_for(prepared, paths, row["tool"], row["rounds"] is not None)
    directories = {engine: directory / engine for engine in ENGINES}
    for path in directories.values():
        path.mkdir(parents=True)
    def run(engine):
        return execute(row, commands[engine], directories[engine], expected,
                       prepared["inputs"][row["input_set"]], args.timeout)
    gates = {engine: run(engine) for engine in ENGINES}
    orders, phases = engine_orders(), {}
    for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
        phases[phase] = []
        for index in range(count):
            order = orders[index % len(orders)]
            sample = {"index": index, "order": order}
            for engine in order:
                sample[engine] = run(engine)
                assert {key: value for key, value in sample[engine].items() if key != "elapsed_ns"} == {
                    key: value for key, value in gates[engine].items() if key != "elapsed_ns"}
            phases[phase].append(sample)
    summaries = {engine: shared.summary(phases["samples"], engine) for engine in ENGINES}
    result = {**{key: value for key, value in row.items() if key != "gates"}, "gates": gates,
              "engine_orders": orders, **phases, "summary": summaries,
              "ratios": {left + "_over_" + right: summaries[left]["median_seconds"] / summaries[right]["median_seconds"]
                         for left, right in (("imre", "rust"), ("codon", "rust"), ("original", "imre"),
                                             ("port", "imre"), ("codon", "imre"), ("original", "rust"))}}
    if row["rounds"]:
        result["amortized_seconds"] = {engine: value["median_seconds"] / row["rounds"] for engine, value in summaries.items()}
    return result


def main():
    if sys.version_info < (3, 14):
        raise SystemExit("run this benchmark with CPython 3.14 or later for its Python-semantics oracle")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "catalog-rust", "conformance-rust"):
        parser.add_argument("--" + name, required=True)
    for name in ("runtime", "imre-build-record", "catalog-codon-record", "conformance-codon-record",
                 "conformance-unchanged", "conformance-changed", "artifact-dir", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--workload-preparation", type=Path,
                        help="reuse another mode's frozen input paths so rendered paths remain identical")
    phases = parser.add_mutually_exclusive_group(required=True)
    phases.add_argument("--prepare-only", action="store_true")
    phases.add_argument("--prepared-record", type=Path)
    parser.add_argument("--allow-dirty", action="store_true")
    parser.add_argument("--samples", type=shared.positive_int, default=20)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--compute-rounds", type=shared.positive_int, default=4)
    parser.add_argument("--timeout", type=shared.positive_float, default=120)
    parser.add_argument("--cpu", type=int)
    args = parser.parse_args()
    assert args.samples % 10 == 0, "samples must cover complete five-engine Williams cycles"
    paths, records, target = source_paths(args)
    python = json.loads(checked([paths["python"], "-P", "-s", "-B", "-c", shared.PYTHON_INFO]))
    assert python["implementation"] == "cpython" and tuple(map(int, python["version_number"].split(".")[:2])) >= (3, 14)
    assert not python["jit_enabled"], "the CPython comparison pins PYTHON_JIT=0"
    hashes = {name: shared.sha256(path) for name, path in paths.items()}
    revision = checked(["git", "-C", ROOT, "rev-parse", "HEAD"]).strip()
    dirty = bool(checked(["git", "-C", ROOT, "status", "--porcelain"]).strip())
    assert args.allow_dirty or not dirty, "commit measured sources first; --allow-dirty is exploratory only"
    artifact = args.artifact_dir.resolve()
    assert not artifact.exists() or not any(artifact.iterdir()), "use a fresh artifact directory"
    prepared = json.loads(args.prepared_record.read_text()) if args.prepared_record else None
    preparation_hash = shared.sha256(args.prepared_record) if args.prepared_record else None
    protected = [*paths.values(), *([args.prepared_record.resolve(strict=True)] if args.prepared_record else [])]
    if prepared:
        assert prepared["input_guards_passed"]
        protected += [Path(path) for path in prepared["retained_sha256"]]
    shared_workloads = json.loads(args.workload_preparation.read_text()) if args.workload_preparation else prepared
    input_roots = [] if shared_workloads is None else [Path(item["directory"]) for item in shared_workloads["inputs"].values()]
    assert all(not artifact.is_relative_to(path) and not path.is_relative_to(artifact) for path in input_roots)
    assert all(not args.output.resolve().is_relative_to(path) for path in input_roots)
    assert all(not path.is_relative_to(artifact) and not artifact.is_relative_to(path) for path in protected)
    assert not args.output.resolve().is_relative_to(artifact)
    assert all(args.output.resolve() != path.resolve() and not (args.output.exists() and args.output.samefile(path)) for path in protected)
    artifact.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if prepared is None:
        prepared = prepare(args, paths, records, target, hashes, revision, dirty)
        prepared["python"] = python
    assert prepared["python"] == python
    verify(prepared, paths, hashes, args, target, revision, dirty)
    if args.prepare_only:
        result = prepared
    else:
        cpu = shared.pin_cpu(args.cpu)
        result = {"schema": 1, "kind": "native-tools-benchmark", "started_at_utc": utc(),
                  "revision": revision, "dirty": dirty, "preparation": prepared,
                  "preparation_record_sha256": preparation_hash, "cpu": cpu, "python": python,
                  "samples": args.samples, "warmups": args.warmups, "compute_rounds": args.compute_rounds,
                  "engine_orders": engine_orders(), "environment": environment(Path("<engine-working-directory>")),
                  "method": "Fresh whole processes on one pinned CPU with warm filesystem caches. Each workload includes complete input reads, parsing, transformation, rendering, and one output publication. Full-byte and input/output filesystem checks are outside timing. Ten Williams orders balance measured positions and directed within-round neighbors; warmups and cross-round transitions are not balanced.",
                  "compute_method": "The catalog preloads raw NDJSON once and repeats its entire parse/select/sort/render pipeline. Conformance preloads its file manifest and identity map plus raw source/diagnostic text; every annotation scan, diagnostic parse, grouping, classification, sorting, and rendering is repeated. Complete last output and summed Unicode lengths are checked. Amortized time includes startup, preload, and final publication; zero-round controls are reported without subtraction. Rust black-boxes inputs and output contributions.",
                  "comparison": "Imre and Rust use generic target settings with matched normal/ThinLTO modes. Codon uses its pinned native release configuration with -disable-native and -numerics=py in both rows; it is not a matched ThinLTO build. Typed and upstream CPython are separate baselines. Codon has no Python interop; its fixed-width integers and JSON duplicate-key boundary are recorded explicitly in validation.",
                  "workloads": [], "input_guards_passed": False}
        controls = {}
        for row in prepared["workloads"]:
            measured = measure(row, prepared, paths, artifact / row["label"], args)
            group = (row["tool"], row["input_set"], row.get("format"))
            if row["rounds"] == 0:
                controls[group] = measured
            elif row["rounds"]:
                control = controls[group]
                measured["preload_control"] = control["label"]
                measured["preload_control_fraction"] = {engine: control["summary"][engine]["median_seconds"] / measured["summary"][engine]["median_seconds"] for engine in ENGINES}
            result["workloads"].append(measured)
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            print(f"{row['label']}: Imre/Rust={measured['ratios']['imre_over_rust']:.3f}; Codon/Rust={measured['ratios']['codon_over_rust']:.3f}", flush=True)
    assert hashes == {name: shared.sha256(path) for name, path in paths.items()}
    verify(prepared, paths, hashes, args, target, revision, dirty)
    assert revision == checked(["git", "-C", ROOT, "rev-parse", "HEAD"]).strip()
    assert dirty == bool(checked(["git", "-C", ROOT, "status", "--porcelain"]).strip())
    if args.prepared_record:
        assert preparation_hash == shared.sha256(args.prepared_record)
    result["input_guards_passed"], result["completed_at_utc"] = True, utc()
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()

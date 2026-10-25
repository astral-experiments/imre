"""Profile exact layout-comparison executables and an optional storage probe.

This driver never measures uninstrumented performance. It reuses the existing
Callgrind/DHAT runner, complete-output checks, raw traces and summary parsers.
"""

import argparse
import json
import runpy
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
layout = SimpleNamespace(**runpy.run_path(str(HERE / "layout_benchmark.py")))
driver = SimpleNamespace(**runpy.run_path(str(HERE / "dependency_profile.py")))
shared = layout.shared
PROBE = HERE / "fixtures/layout_storage.py"
DEFAULT_LABELS = ("layered-16384-all", "wide-16384-all")


def save(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def profile(args: argparse.Namespace) -> None:
    cpu = shared.pin_cpu(args.cpu)
    before = layout.runtime_context()
    record_path = args.record.resolve(strict=True)
    record = json.loads(record_path.read_text())
    if record.get("example") != "concrete_layouts":
        raise ValueError("--record must identify a layout comparison")
    thin_lto = record["options"]["lto"] == "thin"
    if thin_lto != (args.lto_linker is not None):
        raise ValueError("supply --lto-linker exactly for a ThinLTO comparison")
    build_path = args.build_record.resolve(strict=True)
    tool_root = build_path.parent
    inputs = {
        "comparison": record_path,
        "imre_build_record": build_path,
        "compiler": tool_root / "imre",
        "runtime": tool_root
        / ("libimre_runtime_lto.a" if thin_lto else "libimre_runtime.a"),
        "clang": Path(shared.executable(args.clang)),
        "harness": Path(__file__).resolve(),
        "comparison_harness": HERE / "layout_benchmark.py",
        "profiler_harness": HERE / "dependency_profile.py",
    }
    if thin_lto:
        inputs["lto_linker"] = args.lto_linker.absolute()
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    for name in ("imre_build_record", "compiler", "runtime", "clang", "lto_linker"):
        if name in inputs and hashes[name] != record["sha256"].get(name):
            raise ValueError(f"profile {name} differs from the comparison")
    baseline = layout.load_baseline(args.baseline_record, thin_lto, inputs, hashes)
    if hashes["baseline_index"] != record["sha256"]["baseline_index"]:
        raise ValueError("profile baseline differs from the comparison")
    build = json.loads(build_path.read_text())
    for name, digest in build["source_sha256"].items():
        path = ROOT / name
        if shared.sha256(path) != digest:
            raise ValueError(f"frozen compiler/runtime source changed: {name}")
        inputs["source:" + name], hashes["source:" + name] = path, digest
    for name, digest in record["sources"].items():
        path = ROOT / "examples" / name
        if shared.sha256(path) != digest:
            raise ValueError(f"application source changed: {name}")
        inputs["application:" + name], hashes["application:" + name] = path, digest
    for kind, suffix in (("binary", ""), ("ir", ".ll")):
        path = args.artifact_dir.resolve(strict=True) / ("incremental_planner" + suffix)
        digest = shared.sha256(path)
        if digest != record["sha256"]["incremental_" + kind]:
            raise ValueError(f"profiled incremental {kind} differs from the comparison")
        inputs["incremental_" + kind], hashes["incremental_" + kind] = path, digest
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    metadata = {
        "schema_version": 1,
        "method": driver.METHOD,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "comparison_sha256": hashes["comparison"],
        "comparison_git": record["git"],
        "comparison_was_validation_only": record["options"]["validation_only"],
        "comparison_options": record["options"],
        "comparison_tools": record["tools"],
        "baseline_index_sha256": hashes["baseline_index"],
        "imre_build": layout.build_provenance(build),
        "baseline_build": layout.build_provenance(baseline["tool_record"]),
        "rust_build": {
            key: value
            for key, value in baseline["reference_records"]["incremental"].items()
            if key not in {"source_contents", "build_script"}
        },
        "execution_before": before,
        "cpu": cpu,
        "profiler_environment": "all engines inherit the same environment; dependency_profile clears VALGRIND_OPTS and sets only the selected VALGRIND_LIB",
        "hardware_counters": "none; Callgrind instruction counts are not CPU cycles or elapsed-time percentages",
        "runs": {},
    }
    save(output / "provenance.json", metadata)

    def run(name: str, workloads: list[dict], binaries: dict, tools: list[str]) -> None:
        adapter = {
            "schema_version": 1,
            "git": record["git"],
            "program": name,
            "sources": (
                {str(PROBE.relative_to(ROOT)): hashes["probe_source"]}
                if name == "storage"
                else record["sources"]
            ),
            "source_comparison_sha256": hashes["comparison"],
            "binaries": {
                engine: {"sha256": shared.sha256(path), "bytes": path.stat().st_size}
                for engine, path in binaries.items()
            },
            "workloads": workloads,
        }
        adapter_path = output / (name + "-inputs.json")
        save(adapter_path, adapter)
        driver.profile(
            argparse.Namespace(
                record=adapter_path,
                binary=list(binaries.items()),
                build_record=[],
                output=output / name,
                sizes=None,
                shapes=None,
                tools=tools,
                valgrind=args.valgrind,
                valgrind_lib=args.valgrind_lib,
                cpu=args.cpu,
                timeout=args.timeout,
            )
        )
        metadata["runs"][name] = {
            "input_record_sha256": shared.sha256(adapter_path),
            "profile_provenance_sha256": shared.sha256(
                output / name / "provenance.json"
            ),
            "summary_sha256": shared.sha256(output / name / "summary.json"),
            "manifest_sha256": shared.sha256(output / name / "manifest.json"),
        }
        save(output / "provenance.json", metadata)

    if not args.skip_applications:
        selected = layout.manifest.select_workloads(
            {"workloads": record["workloads"]["incremental"]},
            args.workloads or list(DEFAULT_LABELS),
        )
        workloads = []
        for case in selected:
            checked = layout.manifest.captured_workload(
                {
                    **case,
                    "input_sha256": case.get(
                        "input_sha256", layout.manifest.text_hash(case["input"])
                    ),
                    "expected_stdout_sha256": case.get(
                        "expected_stdout_sha256",
                        layout.manifest.text_hash(case["expected_stdout"]),
                    ),
                    "expected_stdout_bytes": case.get(
                        "expected_stdout_bytes", len(case["expected_stdout"].encode())
                    ),
                }
            )
            shape, _, changes = checked["label"].rsplit("-", 2)
            workloads.append(
                {
                    "label": checked["label"],
                    "shape": shape + "-" + changes,
                    "tasks": checked["targets"],
                    "input": checked["input"],
                    "input_sha256": layout.manifest.text_hash(checked["input"]),
                    "expected_stdout": checked["expected_stdout"],
                    "expected_stdout_sha256": layout.manifest.text_hash(
                        checked["expected_stdout"]
                    ),
                    "expected_stdout_bytes": len(checked["expected_stdout"].encode()),
                }
            )
        # The shared driver expands shapes × sizes. Partition by size so an
        # arbitrary captured-label selection does not invent missing pairs.
        sizes = sorted({case["tasks"] for case in workloads})
        for size in sizes:
            run(
                "applications" if len(sizes) == 1 else f"applications-{size}",
                [case for case in workloads if case["tasks"] == size],
                {
                    "baseline": baseline["binaries"]["incremental"],
                    "imre": inputs["incremental_binary"],
                    "rust_bigint": baseline["references"]["incremental"],
                },
                args.tools,
            )

    if args.probe:
        inputs["probe_source"], hashes["probe_source"] = PROBE, shared.sha256(PROBE)
        python_info = json.loads(
            shared.run(
                [shared.executable(args.python), "-I", "-c", shared.PYTHON_INFO],
                b"",
                args.timeout,
            )["stdout"]
        )
        python = Path(python_info["executable"]).resolve(strict=True)
        if shared.sha256(python) != record["sha256"]["python"]:
            raise ValueError("probe CPython differs from the comparison")
        inputs["python"], hashes["python"] = python, shared.sha256(python)
        artifacts = output / "probe-tools"
        artifacts.mkdir()
        binaries, compilation = {}, {}
        mode_runtime = "libimre_runtime_lto.a" if thin_lto else "libimre_runtime.a"
        flags = ["--target", record["options"]["target"]]
        if thin_lto:
            flags += ["--lto-linker", str(inputs["lto_linker"])]
        for engine, compiler, runtime in (
            ("baseline", inputs["baseline:imre"], inputs["baseline:" + mode_runtime]),
            ("imre", inputs["compiler"], inputs["runtime"]),
        ):
            binary, llvm = artifacts / engine, artifacts / (engine + ".ll")
            command = [
                str(compiler),
                "build",
                str(PROBE),
                "-o",
                str(binary),
                "--runtime",
                str(runtime),
                "--clang",
                str(inputs["clang"]),
                "--opt-level",
                "3",
                "--emit-llvm",
                str(llvm),
                *flags,
            ]
            result = shared.run(command, b"", args.timeout)
            binaries[engine] = binary
            compilation[engine] = {
                "command": command,
                "compiler_sha256": shared.sha256(compiler),
                "runtime_sha256": shared.sha256(runtime),
                "binary_sha256": shared.sha256(binary),
                "ir_sha256": shared.sha256(llvm),
                "compilation": result,
            }
            for kind, path in (("binary", binary), ("ir", llvm)):
                inputs["probe_" + engine + "_" + kind] = path
                hashes["probe_" + engine + "_" + kind] = shared.sha256(path)
        workloads, checks = [], []
        commands = {
            "cpython": [str(python), "-I", str(PROBE)],
            **{name: [str(path)] for name, path in binaries.items()},
        }
        for size in args.counts:
            expected_values = (
                size * (size - 1),
                size * (size - 1) + (size + 1) // 2 + 5 * size,
                size * (size - 1) // 2 + (size + 1) // 2 + 9 * size,
            )
            for mode, shape in enumerate(("optionals", "records", "sequences")):
                stdin, expected = f"{mode}\n{size}\n", f"{expected_values[mode]}\n"
                results = {}
                for engine, command in commands.items():
                    actual = shared.run(command, stdin.encode(), args.timeout)
                    if actual["stdout"] != expected:
                        raise AssertionError(
                            f"storage probe output differs: {shape}/{size}/{engine}"
                        )
                    results[engine] = {
                        key: value
                        for key, value in layout.helpers.dependency.compact_output(
                            actual
                        ).items()
                        if key != "elapsed_ns"
                    }
                checks.append({"shape": shape, "count": size, "results": results})
                workloads.append(
                    {
                        "shape": shape,
                        "tasks": size,
                        "input": stdin,
                        "input_sha256": layout.manifest.text_hash(stdin),
                        "expected_stdout": expected,
                        "expected_stdout_sha256": layout.manifest.text_hash(expected),
                        "expected_stdout_bytes": len(expected.encode()),
                    }
                )
        metadata["probe"] = {
            "source": PROBE.read_text(),
            "source_sha256": hashes["probe_source"],
            "compilation": compilation,
            "validation": checks,
            "oracle": "closed-form integer checksums; every retained value is read; present optional values, class fields and typed sequence storage are exercised separately",
        }
        save(output / "provenance.json", metadata)
        run("storage", workloads, binaries, ["dhat"])
    for name, path in inputs.items():
        if shared.sha256(path) != hashes[name]:
            raise RuntimeError(f"profile input changed: {name}")
    after = layout.runtime_context()
    if before != after:
        raise RuntimeError("runtime environment changed during profiling")
    metadata.update(
        completed_at_utc=datetime.now(timezone.utc).isoformat(),
        execution_after=after,
        input_sha256=hashes,
    )
    save(output / "provenance.json", metadata)
    save(
        output / "manifest.json",
        {
            str(path.relative_to(output)): {
                "sha256": shared.sha256(path),
                "bytes": path.stat().st_size,
            }
            for path in sorted(output.rglob("*"))
            if path.is_file()
        },
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("record", "artifact-dir", "baseline-record", "build-record", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=shared.positive_float, default=300)
    parser.add_argument("--valgrind")
    parser.add_argument("--valgrind-lib", type=Path)
    parser.add_argument("--workload", action="append", dest="workloads")
    parser.add_argument(
        "--tools",
        nargs="+",
        choices=("callgrind", "dhat"),
        default=["callgrind", "dhat"],
    )
    parser.add_argument(
        "--probe",
        action="store_true",
        help="also compile and DHAT-profile the focused storage fixture",
    )
    parser.add_argument("--skip-applications", action="store_true")
    parser.add_argument(
        "--counts", nargs="+", type=shared.positive_int, default=[1000, 10000]
    )
    args = parser.parse_args()
    if args.skip_applications and not args.probe:
        parser.error("select applications or --probe")
    if len(set(args.counts)) != len(args.counts):
        parser.error("--counts must be distinct")
    profile(args)


if __name__ == "__main__":
    main()

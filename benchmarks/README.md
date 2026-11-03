# Benchmarks

The [application benchmarks](applications/README.md) compare Imre, Rust and CPython on developer tools and road routing. The [Flask benchmark](applications/README.md#flask) compares HTTP throughput and validates every response. Both publish the raw samples behind the project README charts.

The [original runtime suite](runtime-suite.md) retains the inventory, Base45, Luhn, telemetry and conformance workloads for compiler diagnostics.

Run the runtime suite from the repository root on Linux.

```sh
python3.14 benchmarks/runtime_suite.py --output benchmarks/local/runtime-suite.json
```

## Other workloads

| Workload | What to run |
| --- | --- |
| Route planner | [Benchmark](benchmark.py), [compare two native builds](compare.py) |
| Dependency graphs | [Dependency planner](../examples/dependency_planner/README.md), [incremental planner](../examples/incremental_planner/README.md), [batch planner](../examples/batch_planner/README.md) |
| Records and file I/O | [Manifest planner](../examples/manifest_planner/README.md), [inventory reconciliation](../examples/inventory_reconciliation/README.md), [callback pipeline](../examples/record_pipeline/README.md) |
| Struct layout | [Packed points](results/2026-10-25-struct-points.md), [sparse edits](results/2026-10-25-struct-edit-density.md) |
| Developer tools | [ty memory reports](../examples/memory_report/README.md), [typing conformance](../examples/conformance_report/README.md), [uv’s Python catalog](../examples/python_catalog/README.md), [Ruff](../examples/ruff_ecosystem/README.md) |
| Codecs and numeric workloads | [Base45](../examples/base45_codec/README.md), [Luhn](../examples/luhn_checksum/README.md), [telemetry](../examples/telemetry_comparison/README.md) |

See the [profiling guide](PROFILING.md) for native profiles. The [archived benchmark guide](https://github.com/astral-experiments/imre/blob/8234cc525f4991c9cd61cd9a486def8f064964dc/benchmarks/README.md) links the original optimization studies, samples and replay instructions.

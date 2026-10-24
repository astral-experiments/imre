# Splitlines on Relar 0.1.2

The independent confirmation compared 120 balanced blocks per primary workload against main c9a33681. Baseline, owned-line iteration and borrowed slices were cold-compiled with the same split-module settings and the same 464 external dependency objects. Each timing is a separate, default-system-heap native process on an AMD EPYC Milan Linux VM. The ratio columns are paired speedups with 95% intervals.

| Workload | Baseline ms | Owned ms | Borrowed ms | Baseline / owned | Baseline / borrowed | Owned / borrowed |
| --- | ---: | ---: | ---: | --- | --- | --- |
| catalog-10-plain | 686.39 | 685.52 | 678.62 | 1.0059× [1.0018, 1.0091] | 1.0096× [1.0073, 1.0128] | 1.0057× [1.0025, 1.0083] |
| ruff-10-diff | 1015.58 | 997.51 | 1002.48 | 1.0153× [1.0127, 1.0201] | 1.0161× [1.0092, 1.0211] | 0.9968× [0.9924, 1.0012] |

The six secondary workloads ran 36 balanced blocks each.

| Workload | Baseline / owned | Baseline / borrowed | Owned / borrowed |
| --- | --- | --- | --- |
| catalog-1-gzip | 1.0050× [0.9997, 1.0118] | 1.0068× [1.0011, 1.0126] | 1.0001× [0.9923, 1.0085] |
| catalog-1-plain | 1.0058× [1.0005, 1.0103] | 1.0025× [0.9961, 1.0126] | 0.9959× [0.9914, 1.0052] |
| catalog-10-gzip | 1.0057× [1.0002, 1.0137] | 1.0112× [1.0032, 1.0186] | 1.0034× [0.9988, 1.0089] |
| ruff-1-diff | 1.0113× [1.0052, 1.0198] | 1.0037× [0.9969, 1.0095] | 0.9923× [0.9864, 0.9987] |
| ruff-1-github | 1.0102× [1.0049, 1.0146] | 1.0100× [1.0065, 1.0119] | 1.0000× [0.9958, 1.0069] |
| ruff-10-github | 1.0201× [1.0082, 1.0453] | 1.0145× [1.0001, 1.0209] | 0.9920× [0.9841, 1.0005] |

Total heap requests and cumulative decimal MB for separate Valgrind DHAT processes. Each binary matched the full CPython oracle.

| Workload | Baseline MB / requests | Owned MB / requests | Borrowed MB / requests |
| --- | ---: | ---: | ---: |
| catalog-1-gzip | 75.553 / 232,015 | 75.553 / 232,015 | 55.683 / 231,448 |
| catalog-1-plain | 75.374 / 232,012 | 75.374 / 232,012 | 55.504 / 231,445 |
| catalog-10-gzip | 549.887 / 1,360,952 | 549.887 / 1,360,952 | 351.189 / 1,355,282 |
| catalog-10-plain | 549.709 / 1,360,949 | 549.709 / 1,360,949 | 351.010 / 1,355,279 |
| ruff-1-diff | 21.014 / 171,707 | 20.675 / 170,855 | 20.491 / 167,160 |
| ruff-1-github | 21.240 / 174,429 | 20.901 / 173,577 | 20.717 / 169,882 |
| ruff-10-diff | 197.466 / 1,722,514 | 194.074 / 1,713,994 | 192.232 / 1,677,044 |
| ruff-10-github | 199.496 / 1,750,229 | 196.104 / 1,741,709 | 194.261 / 1,704,759 |

List removal and borrowed slices qualified. The admission criterion required a 95% lower speedup bound above 1.0 on one primary and at least 1/1.01 on the other.

The unaffected Base45 control exercises repeated string append with byte-identical application LLVM in all three arms. Each cell uses 36 balanced blocks.

| Control | Baseline / owned | Baseline / borrowed | Owned / borrowed |
| --- | --- | --- | --- |
| encode | 1.0269× [1.0145, 1.0463] | 0.9892× [0.9826, 1.0088] | 0.9686× [0.9430, 0.9805] |
| decode-text | 1.0011× [0.9987, 1.0059] | 1.0190× [1.0134, 1.0232] | 1.0194× [1.0132, 1.0214] |

The borrowed encode interval includes a slowdown greater than 1%. The earlier borrowed design took roughly 43% longer on that workload before the copy-on-write append path was revised.

List removal saved 8,520 requests and 3.39 MB for the 10× Ruff diff, and its 1× diff was measurably faster than borrowing. Borrowing removed 5,670 long-line buffers totaling 198.70 MB from the 10× uv catalog and gained another 0.57% on its plain input. The maximum live heap size was the same for all arms in each of the eight cases.

The current catalog validation covered 66 cases across 325 engine executions, and the original Ruff CLI matched in 13 cases per native arm. All 180 splitline differentials agreed with CPython 3.14.5. The existing file-read UTF-8 error has the same class and occurs before loop entry in each native build, with wording that differs from CPython. The owned-line candidate passed 363 Rust tests and the workspace formatting check. A correction to one Rust test assertion preserved every allocated ELF section byte and address in both measured owned-line programs except the build ID.

The benchmark runs Relar's unchanged uv streaming catalog port on its checked-in release index and Ruff's unchanged upstream conformance reporter on the 146-file typing suite. The catalog oracle runs the upstream finder and renderer on CPython. Ruff runs both ty captures through its real subprocess calls. One 10× input repeats the release index and the other replays 1,460 source files. Plain/gzip and diff/GitHub paths have independent byte-for-byte output checks.

The measurement driver accepts `prepare`, `oracles`, `timing` and `allocations`. Set `--work` to an empty absolute output directory, and use python/typing revision `835b70f3f795f8e061b1f7621178515b0a77a0ad` with `--typing-conformance` during preparation. Build each catalog executable with `--module-root examples` and the reporter at `examples/conformance_report/upstream/conformance.py`. Use `--opt-level 3` and a matching single-package runtime archive for each compiler. Place the binaries in `<work>/apps/{baseline,owned,borrowed}/{catalog,ruff}`. For the two primary cases, run `timing --case catalog-10-plain --cpu 12 --blocks 120 --seed 604397` and `timing --case ruff-10-diff --cpu 18 --blocks 120 --seed 604397` sequentially with `--work`. Run `allocations` separately with Valgrind DHAT.

Both complete source patches apply independently to clean main `c9a33681db661ab4fc48249de6c913b9645178e3`. The borrowed prototype uses bounded, parent-rooted ranges and detaches on mutation. [Reproducible source patches](2026-10-24-splitlines-iteration-source.tar.xz) · [Raw measurements from all three trials](2026-10-24-splitlines-iteration.json.gz) · [Measurement driver](../splitlines_benchmark.py)

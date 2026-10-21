# File-manifest dependency planner

`manifest_planner MANIFEST REPORT` reads a UTF-8 list of payload paths and writes a complete dependency/rebuild report. It reuses the unchanged batch payload parser and incremental graph algorithm. Each payload is a target count, that many `name duration dependency-name ...` rows, a changed-name count, and that many changed-name rows. Durations and schedule times are arbitrary-precision integers.

From the repository root, run the checked-in sample:

```sh
PYTHONPATH=examples python3.14 -m manifest_planner \
  examples/manifest_planner/sample/manifest.txt /tmp/manifest-report.txt
cat /tmp/manifest-report.txt
```

The sample has a valid graph, a malformed duration, and another valid graph with Unicode names and a large integer. Its summary is two completed requests, one invalid request, and three cleaned requests. Payload paths resolve against the **process working directory**, not the manifest's directory.

Compile and run the same package:

```sh
cargo build --release --workspace --locked
target/release/relar build examples/manifest_planner/__main__.py \
  --module-root examples --runtime target/release/librelar_runtime.a \
  --clang clang-18 --opt-level 3 --target x86_64-unknown-linux-gnu \
  -o /tmp/manifest-planner
/tmp/manifest-planner examples/manifest_planner/sample/manifest.txt \
  /tmp/manifest-report.txt
```

On a local machine configured for Ohm, use `cargo +ohm -Zohm-defaults=no build --release --workspace --locked` and the compiler/runtime paths from its target directory. CI uses the repository's normal pinned Cargo toolchain.

## File and error contract

The program reads and closes the entire manifest before opening the report in write mode. `splitlines()` recognizes LF, CR, CRLF, VT, FF, U+001C–U+001E, NEL, line separator, and paragraph separator; U+001F is not a line boundary. Each resulting line is a path verbatim, including spaces. Empty lines are requests for the empty path; an empty manifest has no requests, and a final line ending adds no extra request. Repeated paths are processed repeatedly.

Each request emits `REQUEST`, its zero-based ID, the result, then `END` and the ID, each on its own line. Valid payloads use exactly the existing batch report fields, including graph-validation errors as data. Validation order remains duplicate names, negative durations, unknown dependencies, unknown changed names, then cycles; the whole graph is validated even with no changes. Affected scheduling treats unaffected prerequisites as built and selects ready targets by minimum input ID.

Input failures emit `INVALID`, a stable category, and a stable message:

| Failure | Category | Message |
|---|---|---|
| Invalid UTF-8 payload | `UnicodeDecodeError` | `invalid UTF-8` |
| Missing/directory/unreadable payload or input close error | `OSError` | `cannot read payload` |
| Embedded NUL in a payload path | `ValueError` | `invalid payload path` |
| Invalid integer or negative count | `ValueError` | Existing bounded-parser message |
| Missing payload row | `IndexError` | `truncated payload` |
| Malformed target/change row | `AssertionError` | Existing bounded-parser message |

Decode errors are caught before their `ValueError` superclass. The payload helper returns parsed data from inside `with`, so files close on success, parser failure, and early return. Report writes, flushes, graph scheduling, and rendering occur outside the input-recovery block: report failures remain terminal. The final `SUMMARY` reports `COMPLETED`, `INVALID`, and `CLEANED`; graph-error results count as completed parses. Normal stdout is empty.

Wrong argument count raises `ValueError: usage: manifest_planner MANIFEST REPORT`. Manifest open/read/decode/close failures and report open/write/flush/close failures are terminal. Fatal comparisons use status and final error category; CPython traceback text is not part of the contract.

The report is truncated when opened. If it is the manifest, paths have already been captured. Each request header is explicitly flushed before opening its payload, so a payload that aliases the report (including a hard link) observes the written prefix and yields the same normalized parse failure in every implementation. No `fsync` or atomic replacement is promised.

## Correctness and comparison

The [recorded comparison](../../benchmarks/results/2026-10-18-manifest-planner.md) includes ordinary and matched ThinLTO results, unchanged-program controls, complete raw samples, and sanitizer evidence. Large ThinLTO file workloads run 1.86–3.27× faster than CPython and take 1.72–1.79× Rust BigInt's time; these are warm-cache whole-program measurements.

The independent [filesystem oracle](../../benchmarks/manifest_validate.py) does not import the compiled packages. It models sequential opens and hard-link ownership, uses an independent bounded parser and graph oracle, and checks all file bytes and directory entries as well as stdout, stderr, and status. Its 196 cases cover the prior graph/parser corpus, line boundaries, Unicode, malformed UTF-8, missing/directory/NUL paths, path components, repeated recovery, output aliases, and terminal failures. A separately recorded Linux `/dev/full` check requires report-write failure to terminate with `OSError`, empty stdout, and unchanged input files; it is explicitly skipped when that device is unavailable.

```sh
python3 benchmarks/reference/incremental_planner/build.py --manifest \
  --artifact-dir benchmarks/local/manifest-reference \
  --target-dir target/manifest-reference
python3 benchmarks/manifest_validate.py --binary /tmp/manifest-planner \
  --rust-bigint benchmarks/local/manifest-reference/planner-manifest-bigint \
  --python python3.14
```

The [Rust reference](../../benchmarks/reference/incremental_planner/README.md) shares the existing algorithm/parser and uses BigInt. Its manifest entrypoint checks file close errors explicitly on Linux; it flushes and closes reports after success or failure without adding `fsync`.

`manifest_benchmark.py` has separate three-engine groups. The unchanged incremental control compares CPython, the preserved optimized pre-file Relar binary, and the new Relar binary. The file workload compares CPython, new Relar, and Rust BigInt. Defaults retain all five prior control inputs and make six file workloads from layered/wide 16,384-target and boundary-128 all-changed inputs: two valid payload reads, or those reads interspersed with invalid-integer, missing-file, and invalid-UTF8 requests.

Each group uses 30 measured rounds covering all six orders five times, two warmups, one pinned CPU, and fresh processes. Every file execution has a fresh isolated working directory. Fixture creation and verification are outside the timer; startup, file operations, computation, complete report writes, flush/close, and exit are inside. Inputs are created immediately before timing and are page-cache warm. This does not measure cold-disk throughput or durable writes. The shared devbox can still introduce scheduling/frequency noise.

The harness requires frozen compiler/runtime and Rust build records, the preserved pre-file control binary, and its `exception_benchmark.py` comparison record. It verifies source/tool/binary hashes, target and LTO mode, performs complete oracles before timing, and checks complete directory snapshots after every run. Input files and changed output files are captured once as base64 plus SHA256; samples retain status, stderr, durations, and the entire file inventory's hashes/lengths. Existing captured control inputs remain unchanged.

```sh
python3 benchmarks/manifest_benchmark.py \
  --compiler "$TOOLS/relar" --runtime "$TOOLS/librelar_runtime.a" \
  --relar-build-record "$TOOLS/build-record.json" \
  --baseline "$PRE_FILE/incremental_planner" --baseline-record "$PRE_FILE_RECORD" \
  --rust-bigint "$RUST/planner-manifest-bigint" \
  --rust-build-record "$RUST/build-record.json" \
  --python python3.14 --clang clang-18 --target x86_64-unknown-linux-gnu \
  --artifact-dir benchmarks/local/manifest-normal \
  --output benchmarks/local/manifest-normal.json
```

The variables identify local frozen artifact directories and the corresponding baseline record. Decompress a published `.json.gz` baseline before passing it. For ThinLTO, select matching ThinLTO baseline/Rust artifacts, use the linker-plugin-LTO runtime archive, and add `--lto-linker /path/to/ld.lld`; the Rust helper's `--llvm-budget 1000` matches Relar's import/inlining budgets. Keep the lexical `ld.lld` symlink name. The [reference guide](../../benchmarks/reference/incremental_planner/README.md) documents ordinary/ThinLTO builds and provenance. Run comparisons only after other builds/profilers stop and outside ptrace.

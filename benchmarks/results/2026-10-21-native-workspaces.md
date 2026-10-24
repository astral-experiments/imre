> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Temporary workspaces and atomic memory-report output

The memory-report tool now retains optional subprocess transcripts and publishes completed reports atomically. Complete process runs take **1.00–1.57× Rust’s time**; file-only runs take **1.13–1.26×**, and repeated report computation takes **1.42–1.57×** across ordinary and ThinLTO builds. Every measured row, including controls, meets the ≤2× Rust target.

Imre now supports direct `with tempfile.TemporaryDirectory(...)` scopes and native `Path.mkdir`, `unlink`, `rmdir`, and `replace`. Temporary directories use private permissions and clean up across normal exits, exceptions, returns, and loop transfers. The compiler keeps the original owner independently of rebinding the visible name. See the [language contract](../../docs/subset.md) and [example guide](../../examples/memory_report/README.md).

## Complete process runs

Median milliseconds over 48 measured samples per engine after two warmups. Pinned ty 0.0.89 and 0.0.90 run over retained inventory and Ruff Python sources. The synthetic layouts each contain 16,256 report items per side: 64 small projects launch 128 children; two larger projects launch four. All engines receive identical synthetic child bytes. Rows labeled ‘retain transcripts’ additionally save the manifest and both decoded streams from every invocation.

| Build | Workload | CPython adapter ms | Imre ms | Rust ms | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: |
| Ordinary | Real ty: inventory + Ruff scripts | 1071.744 | 969.030 | 973.203 | 0.996× |
| Ordinary | 64 projects × 254 items | 433.343 | 340.121 | 262.280 | 1.297× |
| Ordinary | 64 projects × 254 items; retain transcripts | 515.133 | 399.701 | 317.258 | 1.260× |
| Ordinary | 2 projects × 8,128 items | 198.761 | 86.036 | 54.914 | 1.567× |
| Ordinary | 2 projects × 8,128 items; retain transcripts | 216.480 | 95.227 | 63.938 | 1.489× |
| ThinLTO | Real ty: inventory + Ruff scripts | 1062.500 | 964.546 | 964.896 | 1.000× |
| ThinLTO | 64 projects × 254 items | 433.244 | 335.075 | 259.341 | 1.292× |
| ThinLTO | 64 projects × 254 items; retain transcripts | 508.741 | 395.970 | 311.708 | 1.270× |
| ThinLTO | 2 projects × 8,128 items | 204.789 | 84.908 | 54.282 | 1.564× |
| ThinLTO | 2 projects × 8,128 items; retain transcripts | 217.923 | 93.698 | 61.779 | 1.517× |

These complete runs are **1.10–2.41× faster than CPython**. The CPython baseline is a prepared-project adapter using upstream’s unchanged models and renderer, with matching capture and atomic-output behavior. The typed CPython port is separately validated.

Each sample includes startup, manifest validation, child execution, strict UTF-8 capture with universal-newline normalization, decoding, comparison, ranking, rendering, sibling staging, write/close, atomic replacement, workspace cleanup, and exit. Persisted transcripts are staged and replaced individually. Removal of the previous sample’s capture tree occurs outside the clock. The real workload includes ty’s own checking time; separate file and compute workloads expose report-processing costs.

## File-only report generation

These rows read the same reports from directories, perform the full comparison, and publish the result atomically without launching children. Startup, file reading, report processing, temporary-directory creation, publication, and cleanup are included.

| Build | Workload | CPython adapter ms | Imre ms | Rust ms | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: |
| Ordinary | 64 projects × 254 items | 168.091 | 44.269 | 35.218 | 1.257× |
| Ordinary | 2 projects × 8,128 items | 169.843 | 39.460 | 32.861 | 1.201× |
| ThinLTO | 64 projects × 254 items | 168.956 | 40.027 | 33.716 | 1.187× |
| ThinLTO | 2 projects × 8,128 items | 168.571 | 36.384 | 32.160 | 1.131× |

File-only runs are **3.80–4.63× faster than CPython**. This CPython baseline wraps the upstream comparison CLI’s output with the same atomic-publication contract and retains its report echo to stderr.

## Repeated parse, compare, and render

Separate entrypoints preload report text and repeat decoding, comparison, and rendering eight times. The table divides whole-process time by eight, retaining startup, preload, checksum, and final output costs. Every pass contributes the complete report’s Unicode character count to a checked checksum; Rust black-boxes inputs and checksum contributions.

| Build | Workload | CPython adapter ms/pass | Imre ms/pass | Rust ms/pass | Imre/Rust |
| --- | --- | ---: | ---: | ---: | ---: |
| Ordinary | 64 projects × 254 items | 60.959 | 31.400 | 20.541 | 1.529× |
| Ordinary | 2 projects × 8,128 items | 61.543 | 29.219 | 20.631 | 1.416× |
| ThinLTO | 64 projects × 254 items | 59.937 | 27.551 | 17.508 | 1.574× |
| ThinLTO | 2 projects × 8,128 items | 60.881 | 26.036 | 18.084 | 1.440× |

Repeated computation is **1.94–2.34× faster than CPython**. The raw records also retain two-child empty-report controls with and without transcript persistence, plus zero-repetition preload controls for both layouts. Nothing is subtracted from the reported times.

## Semantics and validation

The entire manifest’s structure and project names are validated before workspace creation or child execution. Capture directories must be new. Successfully decoded stdout and stderr are saved before checking a child’s exit status or parsing its report, so partial diagnostics survive failures. Launch or decoding failures do not publish that invocation’s captures. Reports are written and closed in a sibling temporary directory, then atomically replace the destination. A failed write leaves the prior report intact. Replacement changes the destination entry, including symlink and hard-link behavior; it does not promise fsync durability. Cleanup errors can occur after a successful replacement.

Both ordinary and ThinLTO preparation passed 194 application/compute/process cases with 741 comparison executions, plus 52 benchmark-workload executions and four exact actual-ty capture checks: 797 executions per mode. Full generated-code/runtime AddressSanitizer validation repeated the 741-execution matrix and added 13 native workload checks and one actual-ty capture check, for 208 native invocations and 755 total comparison executions. Leak detection was enabled, and expected-error paths were checked for sanitizer diagnostics. Cases cover failed writes and closes, replacement failures, CRLF normalization, partial captures, restrictive permissions, hard links, symlinks, and cleanup errors.

All 269 Rust unit tests, formatting, Clippy, and 34 ordinary differential suites passed, including the upstream Base45 and Luhn tests. The six affected suites passed 408 checks in each validation matrix: ThinLTO at O0/O3 and full ASan/LSan at O0/O1. The new workspace suite contributes 73 checks in every mode. The ordinary aggregate preserves 33 successful suites from the original build revision and records a fresh 77-check process-suite pass after removing its obsolete rejection of the newly supported `mkdir`. The original attempt and exact test change remain in the evidence; compiler/runtime sources are unchanged.

A separate correctness-only tee retains raw streams from the same actual ty invocation before forwarding them. It checks exact persisted text and derives the expected report independently, including under AddressSanitizer. The tee is never timed; measured real-ty rows invoke the pinned binaries directly.

Measurements run serially on CPU 31 after all build and validation jobs finish. CPython 3.14.5 uses PGO/LTO with its JIT disabled. Native code uses O3, Clang 18, and Ohm 1.98.1-1, with matching ordinary/ThinLTO modes for Rust and Imre. All six engine permutations occur eight times per workload, balancing positions and directed within-round neighbors. Warmups and transitions between rounds are not balanced. Inputs are page-cache warm, output is not fsynced, and validation/file hashing occurs outside the clock. These shared-host medians do not establish a confidence interval.

## Evidence and reproduction

Application, reference, harness, and validation inputs were frozen at clean commit `7c5c426f9a890b8a805ffd1155ffd3371673ed04`. The compiler/runtime artifacts retain their original build identity at `2e0eac85ad0eea462be9c4feeffa76497773822c`; all 74 compiler/runtime inputs have identical Git blobs, modes, and SHA-256 values at the measurement commit. Their original build and test records are retained without relabeling. The records contain all 3,744 measured executions.

| Record | SHA256 of uncompressed JSON |
| --- | --- |
| [normal](2026-10-21-native-workspaces-normal.json.gz) | `a1a70bd630c9fd40f997b1906cd5a5d566df42702fece2a309934b0b4d72b905` |
| [thinlto](2026-10-21-native-workspaces-thinlto.json.gz) | `57c295b4d81f75ec2dd58ad3c7abb1a0c53adc06dae2fb94fd52de92516b08aa` |

The [independent audit](2026-10-21-native-workspaces-audit.json), [manifest](2026-10-21-native-workspaces-evidence-manifest.json), [logical index](2026-10-21-native-workspaces-evidence-index.json), and [archive](2026-10-21-native-workspaces-evidence.tar.gz) retain source identities, build and validation records, exact checked inputs/outputs, raw ty witnesses, and timing context. Historical benchmark archives and native executables/libraries are omitted; their identities remain recorded.

Download the archive, index, manifest, and [replay script](2026-10-21-native-workspaces-replay.py) together:

```sh
python3 2026-10-21-native-workspaces-replay.py --prefix 2026-10-21-native-workspaces --output replay.json
```

Replay checks source identities, output and filesystem oracles, sample statistics, and order balance using retained bytes without Git, the original checkout, or native engines. It does not remeasure speed or prove historical host isolation. See the [recorded replay](2026-10-21-native-workspaces-replay.json), [isolation check](2026-10-21-native-workspaces-portable-check.json), and [benchmark guide](../reference/memory_report/README.md).

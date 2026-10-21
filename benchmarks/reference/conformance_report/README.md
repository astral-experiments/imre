# Independent conformance-report reference

This Rust program independently implements the complete offline report in
[`examples/conformance_report`](../../../examples/conformance_report). It reads
the same manifest, selected Python source files, and two unmodified ty GitLab
JSON captures, then scans annotations, groups and classifies diagnostics,
calculates all summary and per-file metrics, sorts entries, and renders the
same `github` or `diff` report. It does not run Python or consume a precomputed
report representation.

```sh
python3 benchmarks/reference/conformance_report/build.py \
  --toolchain ohm --artifact-dir /tmp/conformance-rust \
  --target-dir /path/to/private/conformance-target \
  --build-cache /path/to/shared/ohm-build-cache

/tmp/conformance-rust/conformance-report-reference \
  --manifest /tmp/conformance-input/manifest.json \
  --old /tmp/conformance-input/old.json --new /tmp/conformance-input/new.json \
  --format github --force-summary-table --output /tmp/conformance.md
```

Use a distinct target directory and artifact directory for `--thinlto`. The
builder records exact source contents and hashes, tool identities, flags,
compiler output, and the freshly compiled executable. Normal builds use
optimization level 3, 16 codegen units, and no LTO. ThinLTO uses the same
optimization level and codegen units, with import and inline budgets of 1,000.
The build helper disables Ohm's automatic experimental defaults.
It explicitly selects the pinned compiler's host target and compiler executable,
overrides inherited compiler wrappers, and copies Cargo's reported executable.

The manifest supplies already resolved canonical path identities, matching the
Python and Relar offline adapters. The scanner preserves Python whitespace,
Unicode scalar column offsets, line splitting, malformed-tag fallback, and
universal-newline input handling. Ordinary position integers use native `i64`
storage, with an arbitrary-precision fallback for larger JSON integers. Shared
canonical paths and moved record vectors avoid repeated path allocations and
deep copies. JSON records are decoded directly into typed structures.

Add `--compute --rounds N` to preload the same raw inputs once and repeat the
complete scan/decode/group/classify/sort/render computation. Inputs and results
pass through `std::hint::black_box`; stdout contains the sum of report Unicode
character lengths. `--output` writes the complete final report once after the
loop. Zero rounds write an empty report and print `0`, providing the same
preload/startup control as the Python and Relar entrypoints.

Ordinary file output is quiet and opens the destination only after successful
rendering. Write, flush, and close failures propagate. The shared validator
checks complete output bytes, prior-report preservation on input errors,
filesystem failures, and the same full-output compute contract against the
pinned unchanged upstream implementation and the typed port.

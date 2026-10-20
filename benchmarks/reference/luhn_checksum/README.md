# Independent Rust Luhn reference

This dependency-free implementation uses the same UTF-8 file protocol and
repetition checksum as `examples/luhn_checksum/driver.py`. It validates digits
from left to right while applying Luhn weights from the right. Check-digit
generation uses a virtual final zero. Bounded ASCII partial sums avoid integer
overflow without allocating Python-style intermediate lists.

Each iteration passes the input and completed result through `black_box`, so
LLVM cannot replace repeated work with a single call. The final complete result
is written once. Errors preserve the output file. Positive repetition counts fit
`u64`; accumulated checksums fit `u128` for addressable inputs and those counts.
Invalid CLI arguments and operating-system diagnostics are outside the shared
library corpus.

`src/unicode.rs` lists all 76 decimal-zero code points in Unicode 16.0, generated
independently from CPython 3.14.5's `unicodedata` database. Each following group of
ten contains digits 0–9. The reference does not share Relar's ICU conversion code.
The validator checks every decimal character supported by the selected CPython.
This is an explicit Unicode 16 comparison domain, not a claim of universal
compatibility across newer Unicode databases.

The same generated file records Python's Unicode16 printability boundaries,
including combining marks and variation selectors. Diagnostic quoting therefore
uses the declared Python Unicode domain. Every validation case requires exact
messages, category, cause, suppression, and unchanged output, with no diagnostic
exclusions. Timed inputs contain only valid ASCII decimal digits.

```sh
python3 benchmarks/reference/luhn_checksum/build.py \
  --artifact-dir benchmarks/local/luhn-reference \
  --target-dir benchmarks/local/luhn-reference-target
python3.14 benchmarks/luhn_validate.py \
  --rust benchmarks/local/luhn-reference/luhn-reference \
  --output benchmarks/local/luhn-reference/validation.json
```

For local Ohm builds add `--toolchain ohm`, and use `--build-cache` for a shared
build directory distinct from the target directory. The build helper records
complete sources, tool identities and versions, effective flags, build output,
and the copied executable hash. It clears inherited Rust flags and explicitly
selects release optimization level 3 and 16 codegen units. `--thinlto` selects
ThinLTO with LLVM import/inlining budgets of 1,000, matching Relar's expanded
ThinLTO budgets. Use separate artifact directories for normal and ThinLTO builds.

Pass the executable to `benchmarks/luhn_benchmark.py --rust`; its build record
defaults to the adjacent `build-record.json`. The harness requires matching
normal/ThinLTO modes, validates the exact executables before timing, balances
engine order, and records an empty-input startup/protocol control.

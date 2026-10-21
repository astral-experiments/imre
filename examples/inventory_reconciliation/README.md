# Inventory reconciliation

`inventory_reconciliation STOCK ORDERS REPORT` fulfills positive requests in batch/input order, reports shortages, and preserves the remaining stock. The same typed Python package runs unchanged on CPython and Relar. Quantities and totals are arbitrary-precision integers.

From the repository root:

```sh
PYTHONPATH=examples python3.14 -m inventory_reconciliation \
  examples/inventory_reconciliation/sample/stock.txt \
  examples/inventory_reconciliation/sample/orders.txt /tmp/inventory-report.txt
cat /tmp/inventory-report.txt

cargo build --release --workspace --locked
target/release/relar build examples/inventory_reconciliation/__main__.py \
  --module-root examples --runtime target/release/librelar_runtime.a \
  --clang clang-18 --target x86_64-unknown-linux-gnu --opt-level 3 \
  -o /tmp/inventory-reconciliation
/tmp/inventory-reconciliation \
  examples/inventory_reconciliation/sample/stock.txt \
  examples/inventory_reconciliation/sample/orders.txt /tmp/inventory-report.txt
```

Locally, use `cargo +ohm -Zohm-defaults=no` with the configured target/cache paths instead of plain Cargo. CI uses the repository's normal pinned toolchain.

## Input and ordering

The UTF-8 stock file begins with a nonnegative row count, followed by exactly that many `SKU quantity` lines. Stock quantities must be nonnegative. Duplicate stock entries use the **last quantity**, while their position in the report follows the SKU's **first appearance**. An earlier negative stock quantity is still invalid even if a later duplicate would replace it.

The orders file begins with a nonnegative batch count. Each batch begins with a nonnegative request count, followed by exactly that many `SKU quantity` lines. Empty batches are valid. Zero and negative requests are parsed and counted but ignored during fulfillment; they do not reserve stock or mark a SKU as requested. Positive duplicate requests remain separate and compete for the same remaining stock. Unknown SKUs have zero availability and never create stock rows.

Batch and row indices are **zero-based**, with row indices referring to the original batch before ignored requests are filtered. Requested-SKU totals follow first positive-request order through an ordered dictionary. Remaining and untouched stock follow first stock appearance. Set traversal never determines report order. The requested-SKU set selects the `UNTOUCHED` section, including stock referenced only by ignored requests.

SKUs are nonempty whitespace-free Unicode tokens. Numbers use the prototype's signed ASCII decimal format, with optional underscores between digits. Unicode decimal digits and CPython's configurable integer conversion digit limit are outside the shared input domain. Each count line has one token and each item line two; trailing lines, including extra blank lines, are rejected. `splitlines()` accepts the existing complete Python line-boundary set; a final line ending does not add an empty row. Paths resolve against the process working directory and may contain spaces. OS argument strings containing NUL or invalid UTF-8 are outside the common CLI domain.

The sample includes Unicode names, a duplicate stock entry, ignored requests, an empty batch, an unknown SKU, shortages, and a quantity beyond fixed-width integers. It fulfills five positive requests and reports total shortage five.

## Report and failures

Normal stdout is empty. The report begins with counts and requested/filled/shortage totals, followed by:

- `FULFILLMENTS`: every positive request as `FULFILL batch row SKU requested filled shortage`.
- `SHORTAGES`: each unfilled remainder as `SHORTAGE batch row SKU shortage`.
- `REQUESTED`: total positive demand per SKU as `REQUEST SKU quantity`.
- `REMAINING`: every stock SKU as `STOCK SKU quantity`.
- `UNTOUCHED`: the same row format for stock with no positive request.
- `END`.

Validation order is argument count, stock open/decode/close and parse, then orders open/decode/close and parse. Both inputs are fully read and closed before opening the report. Wrong argument count raises `ValueError: usage: inventory_reconciliation STOCK ORDERS REPORT`. Input I/O failures normalize to `OSError: stock: cannot read` or `orders: cannot read`; invalid UTF-8 normalizes to `ValueError: stock: invalid UTF-8` or its orders equivalent. Parser failures use stable `ValueError` messages: `truncated`, `expected count`, `negative count`, `expected SKU and quantity`, `invalid integer`, `trailing data`, and `stock: negative quantity`, with the relevant input prefix. A huge frame count still parses available rows before reporting truncation or the first row error.

Malformed input is terminal and leaves an existing report untouched. Report open/write/flush/close failure is terminal `OSError: report: cannot write`; invalid report paths use `ValueError: report: invalid path`. These are status-one failures; CPython traceback text is excluded from comparison, but the final normalized category/message is checked exactly. Output is truncated on open, buffered, explicitly flushed, and closed. It is not atomic or fsynced: partial writes remain visible after failure. A report that aliases either input, including a hard link, is safe to parse because both input contents have already been captured; the aliased file is then overwritten as normal.

## Language features and validation

Nested eager list comprehensions read batches with effectful inner bounds, flatten/filter positive requests, and fulfill requests through shared mutable inventory. Dict comprehensions implement last-wins stock and ordered requested totals. Set expansion copies the requested dictionary keys into the membership set. Filtered comprehensions derive shortages and untouched `(SKU, quantity)` tuples. These are whole-program uses of the new syntax, not replacement benchmark kernels.

The renderer uses scalar f-strings for report fields and the newline passed to `write`. Fields retain their existing left-to-right evaluation and conversion order, and the report format is unchanged. Adjacent f-strings keep longer rows readable without adding intermediate strings. Relar lowers these fields through the same typed conversions and ordered string construction used by explicit concatenation.

Parser rows use sequence unpacking while retaining the existing count/SKU diagnostics and conversion order. Report totals share one chained initialization.

Named arguments make batch/row coordinates, record fields, shared inventory, and the output stream explicit. These parameters are keyword-only where positional calls would be unclear. File loading and the reader constructor default their diagnostic label to the scalar string `"stock"`; stock parsing omits that argument, while orders pass `label="orders"`. Supplied expressions retain their original evaluation order, and the default has the same value as the previous explicit argument. The input contract, algorithm, and report bytes are unchanged.

Truth tests and operand-returning `and`/`or` select the fulfilled quantity; conditional expressions handle signed frame counts. Starred unpacking drops the executable name from `sys.argv`, and fixed-length list unpacking validates the three paths. For count tokens of at most 18 characters, the reader uses string indexing to inspect an optional sign, a string slice to remove it, and character iteration to accumulate decimal digits while checking underscore placement. Eighteen decimal digits fit the native small-integer range. Longer tokens go directly through the existing `int()` path, preserving arbitrary-precision counts and CPython's configured decimal-conversion limit, whose minimum nonzero setting exceeds this short-token bound. Quantity parsing retains the existing bulk conversion. The scanner preserves the shared ASCII input grammar and the previous count/error ordering; Unicode decimal digits remain outside the shared input domain.

Truthiness handles empty count tokens, remaining stock-row counts, and nonzero shortages. The count scanner's `for … else` returns a number only after normal exhaustion; an invalid underscore breaks to the same integer-error boundary. Stock parsing uses `while … else` to check trailing input and return only after consuming all declared rows; a negative quantity breaks to the existing error. Neither loop runs its `else` after a raised exception. Fulfillment uses the value-returning expression `(available and bounded_quantity) or 0`: missing or zero stock yields zero, while positive stock uses the existing bounded quantity. A dictionary `get` replaces membership followed by indexing, and absent SKUs still never become stock rows. Reconciliation order, eager intermediates, report bytes, and terminal errors are preserved; performance comparisons distinguish the application rewrite from compiler changes.

The independent [oracle](../../benchmarks/inventory_validate.py) never imports this package. It computes each fulfillment from initial stock minus cumulative earlier demand, checks conservation, and models file contents/hard-link aliases. It compares the entire filesystem, report bytes, stdout, stderr category/message, and status for every implementation. The corpus includes deterministic random inventories, empty/duplicate/ignored/unknown requests, Unicode/NUL SKUs, line boundaries, malformed counts/rows, huge quantities/counts, input/output aliases, and terminal errors. A separate Linux `/dev/full` check verifies the application's output-error boundary.

```sh
python3 benchmarks/reference/inventory_reconciliation/build.py \
  --artifact-dir benchmarks/local/inventory-reference \
  --target-dir target/inventory-reference
python3 benchmarks/inventory_validate.py --python python3.14 \
  --binary /tmp/inventory-reconciliation \
  --rust-bigint benchmarks/local/inventory-reference/inventory-bigint \
  --output benchmarks/local/inventory-validation.json
```

The [Rust reference](../../benchmarks/reference/inventory_reconciliation/README.md) uses the same eager algorithm, BigInt quantities, ordered dictionary traversal, and explicit checked file close. Native Rust containers and allocation representations differ from Relar.

## Benchmark

`benchmarks/inventory_benchmark.py` defaults to empty and 128-request boundary cases, plus dense-repeat and sparse/unknown-SKU cases with 2,048 and 16,384 positive requests. Nonpositive rows and empty batches are retained. Each workload uses 30 measured rounds covering all six CPython/Relar/Rust orders five times, two warmups, a pinned CPU, and fresh processes/directories. Positions and within-round predecessors are balanced; cross-round predecessors are not. Complete output and directory snapshots are checked for every run, with no discarded samples.

The timer includes startup/imports, file operations, parsing, eager allocations, reconciliation, complete rendering, flush/close, and exit. Build/oracle work, fixture creation, hashing/comparison, and fixture deletion are excluded. Inputs are created before timing, OS caches are not deliberately flushed, and writes are not fsynced. These are whole-program results on a shared machine, not isolated comprehension costs or cold-storage throughput.

```sh
python3 benchmarks/inventory_benchmark.py \
  --compiler "$TOOLS/relar" --runtime "$TOOLS/librelar_runtime.a" \
  --relar-build-record "$TOOLS/build-record.json" \
  --rust-bigint "$RUST/inventory-bigint" --rust-build-record "$RUST/build-record.json" \
  --python python3.14 --clang clang-18 --target x86_64-unknown-linux-gnu \
  --cpu 0 --samples 30 --warmups 2 \
  --artifact-dir benchmarks/local/inventory-normal \
  --output benchmarks/local/inventory-normal.json
```

`TOOLS` and `RUST` identify separately frozen tool/reference directories. Add `--validate-only` to compile and validate all oracle and captured workload cases without timing. `--baseline BINARY --baseline-record JSON` optionally adds the five unchanged incremental-planner controls from the concrete-layout comparison: CPython, preserved Relar, and new Relar. The harness verifies exact old source/input/binary identity; the old programs are unchanged. Decompress published JSON before passing it.

For ThinLTO, select matching compiler/runtime/reference records, use `librelar_runtime_lto.a`, and pass `--lto-linker /path/to/ld.lld`; the Rust helper must use ThinLTO and `--llvm-budget 1000`. The lexical linker filename is preserved. The harness records hashes, exact versions, source/build provenance, all inputs/outputs, all raw samples, and relevant inherited environment. Run outside ptrace only after other builds, tests, and profilers stop. No timing result is implied by functional validation.

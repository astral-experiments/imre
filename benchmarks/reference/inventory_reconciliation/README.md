# Rust BigInt inventory reference

This is a direct Rust port of `examples/inventory_reconciliation`, using `num-bigint 0.4.8` and `num-traits 0.2.19`. Quantities and sums have arbitrary precision. Parsed frame counts larger than addressable memory still consume available rows and report the same first parse error; they do not become a fixed-width quantity mode.

The reference builds the same ordered last-wins stock index, eager positive-request vector, requested set/totals, sequential mutable fulfillment results, shortages, and untouched-stock selection. A hash map plus a vector preserves dictionary insertion order. Borrowed Rust strings and native containers have different allocation costs from the Python representations. It is a semantic/algorithm control, not identical generated code.

Strict UTF-8 and full Python `splitlines`/whitespace behavior match the shared input format. File descriptors have explicit checked Linux close ownership. Input close failures override earlier read/decode results; reports flush and close after success or failure without adding fsync. Both inputs close before output opens. Errors normalize to the same final category/message as the Python package. The program uses Linux file-close semantics; invalid UTF-8/NUL OS argument strings are outside the common CLI domain.

Normal CI/public build:

```sh
python3 benchmarks/reference/inventory_reconciliation/build.py \
  --artifact-dir benchmarks/local/inventory-reference-normal \
  --target-dir target/inventory-reference-normal
```

Local Ohm build with the task's configured wrapper and a distinct stable target:

```sh
python3 benchmarks/reference/inventory_reconciliation/build.py \
  --toolchain ohm --cargo-wrapper /tmp/imre-cargo.sh \
  --artifact-dir benchmarks/local/inventory-reference-normal \
  --target-dir "$INVENTORY_TARGET" --build-cache "$SHARED_BUILD_CACHE"
```

The optional wrapper is invoked through `sh` and recorded verbatim. An explicit Cargo `--target-dir` overrides any wrapper target setting; shared-cache and target directories must differ. Without a wrapper the helper invokes normal Cargo, or `cargo +ohm -Zohm-defaults=no` when selected. Direct version inspection uses the selected `rustc` toolchain. CI does not opt into Ohm trust settings.

Default release builds have no LTO override. For matched ThinLTO:

```sh
CARGO_PROFILE_RELEASE_LTO=thin \
python3 benchmarks/reference/inventory_reconciliation/build.py \
  --llvm-budget 1000 \
  --artifact-dir benchmarks/local/inventory-reference-lto \
  --target-dir target/inventory-reference-lto
```

Add the same local toolchain/wrapper/cache options on an Ohm machine. Inherited `RUSTFLAGS` and encoded flags are cleared. Only `--llvm-budget N` sets `-Cllvm-args=-import-instr-limit=N -Cllvm-args=-inline-threshold=N`. Recorded `CARGO_PROFILE_RELEASE_*` settings can override Cargo's default profile. Ordinary and ThinLTO modes use separate target/artifact directories.

Outputs are `inventory-bigint` and `build-record.json`. The record retains exact Rust source/lockfile/build-helper contents and hashes, compiler/Cargo versions and hashes, wrapper contents, profile/environment overrides, command, build duration, and binary hash/size. Sources/tools are rehashed after building; output guards reject source/tool aliases including hardlinks. Rebuilding produces a new record, rather than relabeling an old measured executable.

```sh
python3 benchmarks/inventory_validate.py --python python3.14 \
  --rust-bigint benchmarks/local/inventory-reference-normal/inventory-bigint \
  --output benchmarks/local/inventory-reference-validation.json
```

Add `--binary /path/to/imre-inventory` to check all three engines. The independent oracle validates complete files/status/output, including an explicit Linux `/dev/full` failure; it does not import the compiled package or copy the mutable fulfillment algorithm.

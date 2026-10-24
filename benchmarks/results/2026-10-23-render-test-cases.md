> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff’s diagnostic detail renderer

Imre compiles Ruff’s `render_test_cases` unchanged, including its call to `itertools.groupby` and the immediate `list(group)`. The changed typing-suite comparison produced 249 diagnostic entries across seven consecutive title groups at 1× and 2,490 entries at 10×. [Pinned Ruff source](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L699-L753)

## Complete reports

The frozen cases read the manifest, source files, and both ty captures, compute the full report, and write its Markdown to disk. The 1× case uses the 146 files in the pinned typing checkout. The 10× case repeats those files ten times with distinct paths and corresponding diagnostic locations. The live case discovers the original checkout, runs ty 0.0.60 and 0.0.90, and publishes the report. The timings include process startup and exclude compilation.

All values are milliseconds, reported as medians of ten fresh processes per engine and case after one warmup. The order balances engine position and neighboring engines. The frozen runs used one CPU and the live runs used two CPUs on the same AMD EPYC-Milan Linux host. Both Imre versions used ordinary optimized module-by-module builds with the shipped 0.1.1 system runtime. The independent Rust reference used its default system allocator and ordinary release optimization.

| Files and format | Ruff on CPython | Codon | Imre 0.1.1 | Imre | Rust | Imre / Rust | Report bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 146, GitHub | 170.3 | 74.1 | 38.3 | 38.7 | 27.1 | 1.43× | 109,258 |
| 146, diff | 175.2 | 73.2 | 37.9 | 38.8 | 27.6 | 1.41× | 96,947 |
| 1,460, GitHub | 992.8 | 692.2 | 360.4 | 364.3 | 265.1 | 1.37× | 1,101,967 |
| 1,460, diff | 981.5 | 689.8 | 355.1 | 357.9 | 259.6 | 1.38× | 979,040 |
| 146, live GitHub | 730.8 | 783.2 | 663.8 | 666.9 | 647.0 | 1.03× | 109,258 |

Each warmup and timed process produced exactly the same bytes as pinned Ruff under CPython. A separate whole-program build also passed, at 1.36–1.44× Rust on the four frozen cases and 1.02× on the live checkout.

CPython 3.14.5 used the original Ruff functions through the existing input adapter. Codon 0.20.4 used its native `itertools.groupby`. Its build strips the keyword-only separator, maps the format’s string `Literal` to `str`, and passes the checkout URL directly to the same HTML row helper at its two call sites. The existing native adapters handle JSON, regex, and OS operations. [Codon build](../codon_native/build.py)

## Reproduce

Prepare both fixture scales with the example instructions. Build the native Codon and Rust comparators with their repository drivers, and compile the previous and new reporters with their respective Imre compilers. Then run the comparison below with paths to those inputs. [Fixture preparation](../../examples/conformance_report/README.md#replay-a-captured-comparison) · [Rust reference builder](../reference/conformance_report/build.py)

```sh
python3 benchmarks/conformance_render_benchmark.py \
  --fixtures "$FIXTURES" --python "$CPYTHON" --previous "$PREVIOUS" \
  --imre "$IMRE_REPORT" --codon "$CODON_REPORT" --rust "$RUST_REPORT" --cpu 8 \
  --live "$TYPING/conformance" "$OLD_TY" "$NEW_TY" --live-cpus 20 21 \
  --output measurements.json.gz
```

The measurement record includes the arguments, raw samples, execution order, output sizes and SHA-256 hashes, compiler and executable identities, and native comparator build records. [Measurement record](2026-10-23-render-test-cases.json.gz)

<details>
<summary>Executable SHA-256 hashes</summary>

| Executable | SHA-256 |
| --- | --- |
| Imre compiler | `ac9d6e227f880af112ea4deb3c69d02c695f7fba9e82728dc0364c67f1dc9dd8` |
| Ruff’s CPython interpreter | `c27397da5541f67655be462e128ca3d3eb6e0a8eb0d39036896c9042529f4304` |
| Codon reporter | `5c9fddddb412b32dccb047ac4f80752143453adf74822cfe08fcf71f02850843` |
| Imre 0.1.1 reporter | `01a463abdecfe16f33014ba83121ab154495c2fdf224c822bc81d8ccc8c16eeb` |
| Imre reporter (default) | `02a353505ee938807240e4fa5c6b2b268908ac0dee20fc5bae9e6c683b4f4251` |
| Rust reporter | `de79dd3daf3939ff2a615fa8fedede44069ebbc8968df226277878141d7ea016` |

An independent build with the final Imre compiler reproduced the entire timed default executable byte-for-byte. The whole-program build reproduced its LLVM IR, machine code, and loadable contents after excluding the ELF build ID.

</details>

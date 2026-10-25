# `string` compatibility

The vendored `Lib/test/test_string` is CPython 3.14.5's entire package. The upstream discovery hook runs **54 tests**; all 54 passed on reference Linux. `UPSTREAM.json` pins the Git blob of every original file. The companion implementations are `Lib/string/__init__.py` and `Lib/string/templatelib.py` at the same commit.

| Owner | Upstream cases | Native evidence |
|---|---:|---|
| ASCII constants and `capwords` | 2 | Complete, byte-identical method bodies; native typed assertion adapter |
| `string.Formatter` | 14 | No native result; field lookup, format specs, subclass overrides and recursion |
| `string.Template` | 24 | No native result; mapping/keyword substitution, invalid patterns and subclass customization |
| `string.templatelib` | 13 | No native result; the Python 3.14 t-string syntax and interpolation objects are separate from `string.Template` |
| `LazyImportTest.test_lazy_import` | 1 | No native result; CPython's import-isolation fixture inspects `base64`'s imports |

At both O0 and O3, `native_core.py` compiles the two named methods without modifying their source or expected operands. Its native case implements `assertEqual(str, str)`, counts the nine comparisons in each, and must raise on deliberately unequal input. `core_differential.py` separately compares 51 output lines with CPython, including full titlecase expansions, contextual final sigma, Unicode whitespace, positional/default/explicit separators, and the exact error for an empty separator. The other 52 reference occurrences remain in each evidence ledger as unexecuted. This is method-level coverage, not native unittest discovery: importing the intact package reaches `test.support` line 6 and stops at `annotationlib`.

Reproduce the reference and native ledgers from the repository root:

```sh
python3.14 benchmarks/cpython_stdlib/suite_inventory.py benchmarks/cpython_stdlib/full/string/UPSTREAM.json --result benchmarks/cpython_stdlib/full/string/reference-linux.json
python3 benchmarks/cpython_stdlib/full/string/native_core.py --imre target/release/imre --runtime target/release/libimre_runtime.a --evidence benchmarks/cpython_stdlib/full/string/evidence-linux.json
```

Optional Linux Unicode audit (runtime-level evidence, not additional suite IDs):

```sh
python3.14 benchmarks/cpython_stdlib/full/string/audit_case_mapping.py --runtime target/release/libimre_runtime.a
```

CPython supplies the expected `capitalize` and `lower` outputs for all 1,112,064 Unicode scalar values as isolated characters and in two Greek sigma contexts. The linked native runtime matched all 3,336,192 pairs.

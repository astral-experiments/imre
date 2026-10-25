# `string` compatibility

The vendored `Lib/test/test_string` is CPython 3.14.5's entire package. The upstream discovery hook runs **54 tests**; all 54 passed on reference Linux. `UPSTREAM.json` pins the Git blob of every original file. The companion implementations are `Lib/string/__init__.py` and `Lib/string/templatelib.py` at the same commit.

| Owner | Upstream cases | Native evidence |
|---|---:|---|
| ASCII constants and `capwords` | 2 | Complete, byte-identical method bodies; native typed assertion adapter |
| `string.Formatter` | 14 | No native result; field lookup, format specs, subclass overrides and recursion |
| `string.Template` | 24 | 9 complete original default-pattern methods; 4 callable assertion methods and 11 custom subclass methods remain unexecuted |
| `string.templatelib` | 13 | No native result; the Python 3.14 t-string syntax and interpolation objects are separate from `string.Template` |
| `LazyImportTest.test_lazy_import` | 1 | No native result; CPython's import-isolation fixture inspects `base64`'s imports |

At both O0 and O3, `native_core.py` compiles **11/54** original method bodies, byte-for-byte: two core methods and nine `Template` methods. The native fixture counts the 41 original string comparisons and must raise for a deliberately unequal pair in each case. CPython executes the same counted fixture as a control. The other 43 original rows remain unexecuted. This is method-level evidence: importing the intact package reaches `test.support` and stops at the unchanged `import warnings` on line 21.

[`template-cases.json`](template-cases.json) accounts for all 24 `TestTemplate` IDs. Four default-pattern methods pass bound or unbound variadic methods to `assertRaises`, sometimes with deliberately invalid call shapes. Eleven more define a custom `Template` class (including the full `is_valid` and `get_identifiers` tests). Focused native checks exercise default-pattern errors and identifiers; they do not count as those original methods. `Formatter` and the 13 t-string/`templatelib` cases are separate.

Supplemental comparisons cover Unicode and nullable mapping values, keyword order/overrides, exact error locations, independent match cursors, oversized decimal integers and a direct read-only mapping. A separate 118-observation cross-module control passes dynamic errno attributes, tuple defaults and mutable dictionary aliases through the same public `object` annotation. Another 44 comparisons pass optional booleans and strings through native exception arguments, nested tuples, and Template; they also distinguish unused from selected oversized integers. The driver also compiles the **unchanged** `StringTemplateStyle.validate` method body from CPython's `Lib/logging/__init__.py:524` and compares nine formats with CPython's real style class. Its native fixture supplies only `_fmt`; this is not a claim of native logging-module discovery. [The stored method](logging_validate.method) is checked by SHA-256. Giving `--cpython-source` additionally verifies the entire original logging Git blob and its byte-identical method span.

On Linux, `template_scaling.py` counts instructions with Callgrind for 0, 256 and 2,048 repeated Unicode placeholders and compares the entire result with CPython. After subtracting startup, an eightfold input increase may use at most sixteen times the work. On the development runtime, the original character-index/slice loop used 53.51 times the work and the token scan used 7.66 times. This is a separate scaling guard, outside the 54 upstream test IDs.

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

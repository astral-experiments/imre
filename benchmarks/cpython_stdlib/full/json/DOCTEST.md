# JSON's upstream doctest fixtures

On Imre `76d74f9`, both import entry points and both runner entry points for the unchanged package stop at `Lib/test/test_json/__init__.py:3` (`import doctest`). The package uses one shared doctest API: `doctest.DocTestSuite(module)`. Its `load_tests` adds each returned suite to a `unittest.TestSuite` before calling `test.support.load_package_tests`. It does not invoke `doctest.testmod`, a finder, or a runner directly. Shared doctest support is owned by the itertools work.

The [pinned release sources](reference.py) and CPython 3.14.5 `DocTestSuite` give these results:

| Module argument | Discovered case ID | Examples per case | Occurrences in `test_json` |
| --- | --- | ---: | ---: |
| `json` | `json` | 32 | 2 |
| `json.encoder` | `json.encoder.JSONEncoder.encode` | 2 | 2 |
| `json.decoder` | none | 0 | 0 |

The examples in a case share a namespace. The `json` case uses:

| Examples (one-based) | Fixture or operation |
| --- | --- |
| 1–6 | Import `json`; `dumps` with mixed lists, dictionaries and tuples, escapes, non-ASCII, and `sort_keys=True` |
| 7–10 | Import and construct `io.StringIO`; `dump` to the stream and inspect `getvalue()` |
| 11–15 | Build a dictionary; `dumps` with compact `separators`, then with `sort_keys=True, indent=4` |
| 16–22 | Compare `loads` with a nested value and an escaped string; `load` from `StringIO` and index the result |
| 23–27 | Define `as_complex`; `loads(object_hook=as_complex)` builds a complex number; `loads(parse_float=decimal.Decimal)` produces a decimal |
| 28–32 | Define `encode_complex`; call `dumps(default=…)`, `JSONEncoder(default=…).encode`, and join `iterencode` chunks |

The `json.encoder.JSONEncoder.encode` case imports `JSONEncoder` from `json.encoder` and calls `JSONEncoder().encode({"foo": ["bar", "baz"]})`.

These are two source cases and four occurrences of the 216, including when the reference runs all 68 example occurrences. The host reference supplies discovery, shared example state, interactive display, and expected output checks. Running any of the same JSON calls in a different test does not credit either doctest case. The Python/C backend identity methods are separate unittest cases; neither is a doctest fixture.

The [shared doctest implementation](https://github.com/astral-sh/imre/pull/646) discovers the same cases from the original docstrings. Its direct native runs reached the missing `io.StringIO` export in the root case and the missing `json.encoder.JSONEncoder` class in the encoder case. Neither direct run completed the corresponding package fixture.

After native `io.StringIO` landed, the [direct suite rerun](doctest-direct-linux.json) reached example 26 of the root case at O0 and O3: `import decimal requires a runtime module initializer`. The encoder case still stops at example 1 on `json.encoder.JSONEncoder`. The decoder suite has zero examples. These are compile diagnostics; reaching an example does not establish that the preceding examples executed. To reproduce, run [the shared doctest driver](../doctest/native.py) with the compiler, runtime, Clang and output paths, and inspect the three JSON module entries.

The [earlier hosted decoder check](https://github.com/astral-sh/imre/actions/runs/40179030429) stopped on `import warnings` in ordinary mode and on `import _imp` in reachable mode. After native warnings support landed, [all four intact-package checks](doctest-startup-linux.json) stop at `Lib/test/support/import_helper.py:2`, `import _imp`. The vendored fixture tree and full pinned Git tree agree. No test starts through either entry point.

The JSON fixture calls `import_fresh_module('json', fresh=['_json'])` and `import_fresh_module('json', blocked=['_json'])`. The pinned helper saves and restores `sys.modules`, imports each fresh dependency, and calls `_imp._override_frozen_modules_for_tests` on entry and exit. JSON uses the default `deprecated=False`, so the helper does not enter its warnings-suppression context for either call. `support.load_package_tests` later calls `loader.discover` and adds the result to the supplied suite. These are shared fixture dependencies; CPython's C and Python imports describe the two original fixtures for the same Imre operations.

Reproduce using the shared checker on both sources:

```sh
python -B benchmarks/cpython_stdlib/full/json/startup.py --compiler /path/to/imre \
    --cpython-git /path/to/cpython.git --result /tmp/json-startup.json
```

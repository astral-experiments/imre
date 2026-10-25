# importlib.machinery.ModuleSpec

The [pinned CPython importlib suite](UPSTREAM.json) has **1,346 test occurrences** on Linux. On a disposable CPython 3.14.5 install with bytecode writing enabled, 1,328 passed and 18 were skipped by upstream. All 108 test_spec cases passed. The frozen and source fixtures are separate occurrences; the four IDs discovered twice are preserved. Using the suite inventory's normal `-B` instead skips 85 additional bytecode cases.

## Native scope

A spec is a distinct mutable object with name, loader, origin, loader state, search locations, the initialization list, and location/cache fields. The native record retains its original loader and state. Its two lists are allocated per construction. It permits reads and writes that preserve each field's inferred storage type. `is_package` invokes the ordinary truth protocol in CPython argument evaluation order. The `has_location` setter runs that protocol after evaluating its target. `parent` returns the name itself for a package; otherwise it returns the prefix before the last dot.

`cached` retains an explicitly supplied cached path; an existing bytecode origin ending in `.pyc` is returned and cached with its original identity. Unknown suffixes return None. Native source-bytecode cache names still require `sys.implementation.cache_tag`, `sys.pycache_prefix`, and optimization metadata. A direct native `.py` cache lookup that would need those fields raises a catchable `NotImplementedError`; the original source-cache and missing-tag methods remain reference-only (four fixture occurrences). CPython comparison against a peer with dynamic attributes is also outside this slice. Native representation works for a data-only loader and omits absent origin/search locations while preserving empty strings and lists.

Explicit `import importlib.machinery`, its import alias and both from-import spellings expose the factory. Reading a bare package's machinery attribute requires a proven explicit child import in the current module. Arbitrary importing, finding/executing modules, callbacks in warnings or linecache, and support for every builtin or custom loader are not implied by that import.

## Evidence

At each of O0 and O3, [the full native ledger](native-linux.json) contains:

| Execution | Occurrences |
| --- | ---: |
| Original body and typed assertions run in Imre; CPython fixture supplies name/path | 10 |
| Original CPython body and assertions call native operations on one persistent heap | 14 |
| Reference only | 1,304 |
| Upstream skip, with condition and source | 18 |

The ten native bodies are the frozen and source variants of `test_package`, `test_package_is_package`, `test_has_location_setter`, `test_cached_no_origin`, and `test_cached_with_origin_not_location`. A native fixture adapter supplies an opaque loader; none of these methods invokes it. The other fourteen run their complete original bodies and assertions under the real CPython setup. Their constructor, property and mutation operations go to a live native process; the test's loader identity can cross that boundary only after a native identity check. The full finder/loader suites do not run natively.

[Direct controls](controls.py) retain and call a native loader, mutate a shared loader state and search list, exercise alias and per-instance identities, verify descriptor and constructor effect order, and cover cache clearing, Unicode parents, and nested representation. A wrong native property result fails the original host assertion; a wrong package flag fails an original native body. Neither control adds a test ID.

[Startup diagnostics](startup-linux.json) compare six intact suite entries, each as an import and a runner in normal and reachable mode. Two ordinary warnings diagnostics advance from importlib to `io.StringIO`; no whole-suite tests execute in any of these 24 checks. In particular `test_issue31285` needs the warnings loader callback and the bad-string subclass's `splitlines` behavior. Constructing its spec alone cannot establish that case.

## Reproduce

Use test-enabled CPython 3.14.5 on Linux and a built Imre compiler/runtime:

```sh
python3.14 benchmarks/cpython_stdlib/full/reference_prefix.py /tmp/imre-importlib-prefix
/tmp/imre-importlib-prefix/bin/python3.14 -E benchmarks/cpython_stdlib/suite_inventory.py benchmarks/cpython_stdlib/full/importlib/UPSTREAM.json --write-bytecode --result /tmp/importlib-reference.json
/tmp/imre-importlib-prefix/bin/python3.14 -E benchmarks/cpython_stdlib/full/importlib/run.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang --result /tmp/importlib-native.json
```

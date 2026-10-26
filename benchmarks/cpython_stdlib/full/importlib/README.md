# importlib.machinery.ModuleSpec and importlib.util cache paths

The [pinned CPython importlib suite](UPSTREAM.json) has **1,346 test occurrences** on Linux. On a disposable CPython 3.14.5 install with bytecode writing enabled, 1,328 passed and 18 were skipped by upstream. All 108 test_spec cases passed. The frozen and source fixtures are separate occurrences; the four IDs discovered twice are preserved. Using the suite inventory's normal `-B` instead skips 85 additional bytecode cases.

## Native scope

A spec is a distinct mutable object with name, loader, origin, loader state, search locations, the initialization list, and location/cache fields. The native record retains its original loader and state. Its two lists are allocated per construction. It permits reads and writes that preserve each field's inferred storage type. `is_package` invokes the ordinary truth protocol in CPython argument evaluation order. The `has_location` setter runs that protocol after evaluating its target. `parent` returns the name itself for a package; otherwise it returns the prefix before the last dot.

`cached` retains an explicitly supplied cached path; an existing bytecode origin ending in `.pyc` is returned and cached with its original identity. Unknown suffixes return None. For source origins, the getter computes and retains the same POSIX cache filename as `importlib.util.cache_from_source`. With no tag, the getter returns `None`, as CPython's `_get_cached` does. CPython comparison against a peer with dynamic attributes is also outside this slice. Native representation works for a data-only loader and omits absent origin/search locations while preserving empty strings and lists.

Explicit imports of `importlib.machinery` and `importlib.util`, including aliases and from-imports, expose their supported operations. Reading either child on a bare package requires a proven explicit child import in that module.

The public POSIX helpers construct bytecode cache paths and recover source paths without opening files. They implement PEP 3147/488 spelling, Unicode alphanumeric optimization tags, prefixes, cwd handling, the path protocol, and checked deprecated-argument behavior. The mutable `sys.implementation.cache_tag` and `sys.pycache_prefix` fields retain string identity and accept `None`. Imre produces machine code, so its default bytecode tag is `None`: the public helpers raise the same `NotImplementedError` as CPython when its tag is `None`. Set a target tag to compute names for that interpreter. Python's language optimization is zero at both LLVM optimization levels. A direct import of the mutable `sys.pycache_prefix` binding is rejected; use the qualified attribute. Windows cache spelling, arbitrary import execution and a general `sys.implementation` object are outside this slice.

## Evidence

At each of O0 and O3, [the full native ledger](native-linux.json) contains:

| Execution | Occurrences |
| --- | ---: |
| Original body and typed assertions run in Imre; CPython fixture supplies name/path | 10 |
| Original CPython body and assertions call native spec or cache-path operations | 72 |
| Reference only | 1,246 |
| Upstream skip, with condition and source | 18 |

The ten native bodies are the frozen and source variants of `test_package`, `test_package_is_package`, `test_has_location_setter`, `test_cached_no_origin`, and `test_cached_with_origin_not_location`. A native fixture adapter supplies an opaque loader; none of these methods invokes it. Eighteen other spec occurrences run their complete original bodies and assertions on CPython with a persistent native spec heap. The test's loader crosses only after a native identity check. In `test_cached_source`, the original CPython util independently calculates the expected value of the native getter. Another 54 occurrences are the complete frozen/source PEP3147 methods: CPython runs their actual assertions, warning contexts, temporary cwd and setting changes, while a native process performs each cache operation with those inputs. The two Windows-only separator cases are genuine upstream skips on POSIX. Both wrong-result and wrong-exception controls fail those original assertions. The full finder/loader suites do not run natively.

[Direct controls](controls.py) retain and call a native loader, mutate a shared loader state and search list, exercise alias and per-instance identities, verify descriptor and constructor effect order, and cover cache clearing, Unicode parents, and nested representation. A wrong native property result fails the original host assertion; a wrong package flag fails an original native body. Neither control adds a test ID.

[Startup diagnostics](startup-linux.json) compare six intact suite entries, each as an import and a runner in normal and reachable mode. Two ordinary warnings diagnostics advance from importlib to `io.StringIO`; no whole-suite tests execute in any of these 24 checks. In particular `test_issue31285` needs the warnings loader callback and the bad-string subclass's `splitlines` behavior. Constructing its spec alone cannot establish that case.

[Cache-path controls](cache_controls.py) exercise live object identities, error precedence, side effects in `__fspath__`, `__bool__` and `__str__`, frozen-frame warning output, and the cached descriptor. [A Unicode probe](cache_unicode_probe.py) compares every Unicode scalar, validates every accepted filename, and emits acceptance interval boundaries. On pinned CPython and native O0/O3 it emitted the same 1,540 boundaries over 1,112,064 scalar values. The probe is separate from the upstream case counts.

The [whole-source cache-path comparison](cache-startup-linux.json) records five intact package/test entries as imports and runners in normal and reachable modes. No case executes during these 20 checks. The importlib util and spec entries still need the test support helper's `builtins` import; the package entry still needs more of `test.support`.

The [command-mode controls](cache_command.py) compare default and explicit optimization tags, deprecated overrides, and lazy `ModuleSpec.cached` against CPython under normal, `-O`, `-OO`, `PYTHONOPTIMIZE=4`, and isolated startup. Both LLVM optimization levels run every mode.

## Reproduce

Use test-enabled CPython 3.14.5 on Linux and a built Imre compiler/runtime:

```sh
python3.14 benchmarks/cpython_stdlib/full/reference_prefix.py /tmp/imre-importlib-prefix
/tmp/imre-importlib-prefix/bin/python3.14 -E benchmarks/cpython_stdlib/suite_inventory.py benchmarks/cpython_stdlib/full/importlib/UPSTREAM.json --write-bytecode --result /tmp/importlib-reference.json
/tmp/imre-importlib-prefix/bin/python3.14 -E benchmarks/cpython_stdlib/full/importlib/run.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang --result /tmp/importlib-native.json
```

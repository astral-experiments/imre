# errno

The CPython 3.14.5 reference runs all three methods in the unchanged
[test_errno.py](upstream/Lib/test/test_errno.py).
The compiler reads the target's native errno headers: forward names, aliases
and the reverse dictionary follow CPython's registration order. Numbers and
symbol availability can differ between Linux and macOS, and between SDKs. The official
CPython 3.14.5 macOS binary uses SDK 26.4. Darwin 25 / macOS SDK 26 adds
`ENOTCAPABLE` to the SDK 15 table. Native CI and release archives use the
same SDK generation, while a source build deliberately reflects its own target headers.

The [native runner](../../../../tests/errno.py) verifies the source's pinned
Git blob, compiles the complete module, and invokes each original method in a
fresh process. The original frozen fixture and unittest assertions execute
natively; the host schedules them for this per-method ledger. A separate
[whole-file gate](../../../../tests/unittest_main.py) verifies the same pinned
blob and matching CPython version, compiles the unchanged entry point in
ordinary and reachable-only modes, and compares its native unittest output
with CPython. Linux runs O0/O3 and Apple Silicon runs O3. Both gates exercise
the same three IDs.

Two additional runs deliberately cause original assertions to fail. Removing
`EDOM` from `errorcode` must fail reverse membership; inserting a name that is
absent from the module must fail `assertHasAttr`. Their messages are compared
with CPython. Three unused from-import controls (a target-absent symbol, a long absent name,
and a close spelling) must raise ImportError before a later child initializer.
The uncaught display suggests a target export only when CPython would; a
caught exception keeps its original arguments, string, name, and name_from. Three
additional modes pass optional module/attribute strings and verify the hint
requires both present values. A separate program compares every forward name and reverse
entry, aliases, dictionary mutation, static and dynamic lookup, metadata and
missing-name behavior with CPython on the machine under test.

Reproduce the full reference with a CPython 3.14.5 installation that includes
its standard test package, and compare with a native compiler/runtime:

```sh
python3.14 benchmarks/cpython_stdlib/suite_inventory.py \
  benchmarks/cpython_stdlib/full/errno/UPSTREAM.json --result /tmp/errno-reference.json
python3.14 tests/errno.py --compiler target/release/imre \
  --runtime target/release/libimre_runtime.a --clang clang --opt-levels 0 3 \
  --evidence benchmarks/cpython_stdlib/full/errno/evidence-linux.json
python3.14 tests/unittest_main.py --compiler target/release/imre \
  --runtime target/release/libimre_runtime.a --clang clang \
  --manifest benchmarks/cpython_stdlib/full/errno/UPSTREAM.json \
  --upstream-source benchmarks/cpython_stdlib/full/errno/upstream/Lib/test/test_errno.py
```

[Linux reference](reference-linux.json) · [native O0](native-linux-O0.json) ·
[native O3](native-linux-O3.json) · [paired per-occurrence evidence](evidence-linux.json)

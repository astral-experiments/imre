# errno

The CPython 3.14.5 reference runs all three methods in the unchanged
[test_errno.py](upstream/Lib/test/test_errno.py).
The compiler reads the target's native errno headers: forward names, aliases
and the reverse dictionary follow CPython's registration order. Numbers and
symbol availability can differ between Linux and macOS.

The [native runner](../../../../tests/errno.py) verifies the source's pinned
Git blob, compiles the complete module, and invokes each original method in a
fresh process. The original frozen fixture and unittest assertions execute
natively. The host schedules methods; native unittest discovery and its
command-line runner are outside this result.

Two additional runs deliberately cause original assertions to fail. Removing
`EDOM` from `errorcode` must fail reverse membership; inserting a name that is
absent from the module must fail `assertHasAttr`. Their messages are compared
with CPython. A separate program compares every forward name and reverse
entry, aliases, dictionary mutation, static and dynamic lookup, metadata and
missing-name behavior with CPython on the machine under test.

Reproduce the full reference with a CPython 3.14.5 installation that includes
its standard test package, and compare with a native compiler/runtime:

```sh
python3.14 benchmarks/cpython_stdlib/suite_inventory.py \
  benchmarks/cpython_stdlib/full/errno/UPSTREAM.json --result /tmp/errno-reference.json
python3.14 tests/errno.py --compiler target/release/imre \
  --runtime target/release/libimre_runtime.a --clang clang --opt-levels 0 3
```

[Linux reference](reference-linux.json) · [native O0](native-linux-O0.json) ·
[native O3](native-linux-O3.json)

# inspect static attribute lookup

The reference is CPython 3.14.5, commit `5607950ef232dad16d75c0cf53101d9649d89115`. [The manifest](UPSTREAM.json) pins the complete inspect test package, the standard library implementation, and its license by Git blob. The [Linux reference](reference-linux.json) ran all 362 discovered tests: 362 passed, none skipped.

The [native evidence](native-linux.json) counts one complete unchanged method at each of O0 and O3: `TestGetattrStatic.test_no_dict_no_slots`. Its first assertion reads an absent integer attribute with a default; its second retrieves an unbound descriptor from a string. Imre executes the byte-identical method body, its two equality assertions in a simple native assertion adapter, and both product operations. CPython runs discovery and the original module, class, and instance fixture hooks. The other 361 IDs have reference results only.

The [separate parity fixture](static_attributes.py) exercises fixed instance fields; missing attributes and exact exceptions; positional and reordered keyword evaluation; lifetimes of returned mutable fields; the identity of `object()`, `inspect._sentinel`, and method descriptors; and calls through the four unbound string descriptors. Their `__objclass__` is the same builtin class exposed by `annotationlib`. The runner compares status and complete output with CPython. It also checks three explicit compiler boundaries against CPython: a slot, a property that would raise if invoked, and an unsupported integer descriptor. None can be replaced with a default.

On the current native path, the attribute name must be a literal and its owner an exact supported record, plain object, integer, boolean, or string. The supported descriptors are `str.lower`, `str.strip`, `str.lstrip`, and `str.rstrip`. Other descriptor protocols, class dictionaries, arbitrary module objects, and user class MRO reflection are still outside this slice.

## Whole-suite startup

The [raw startup diagnostics](startup-linux.json) use the entire pinned `Lib/test`, unchanged. Each of eight entry modules was checked four ways: import and runner, including all declarations and with `--reachable-only`. On main with `annotationlib` merged, the unittest, pathlib, and inspect package and submodule entries stop at `test/support/__init__.py:9`, the import of `inspect`. With this change those six entries advance to the import of `_opcode` on line 11. Functools still stops at `builtins` and annotationlib’s own tests at a starred union subscription. These checks launch zero tests.

## Run the comparison

Use a test-enabled CPython 3.14.5 and a native compiler/runtime from the same checkout:

```sh
python3.14 benchmarks/cpython_stdlib/suite_inventory.py \
  benchmarks/cpython_stdlib/full/inspect/UPSTREAM.json --result /tmp/inspect-reference.json
python3.14 benchmarks/cpython_stdlib/full/inspect/run.py \
  --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 \
  --reference /tmp/inspect-reference.json --result /tmp/inspect-native.json
```

# inspect

The reference is CPython 3.14.5, commit `5607950ef232dad16d75c0cf53101d9649d89115`. [The manifest](UPSTREAM.json) pins the complete inspect test package, the standard library implementation, and its license by Git blob. The [Linux reference](reference-linux.json) ran all 362 discovered tests: 362 passed, none skipped.

The [native evidence](native-linux.json) covers two of the 362 original occurrences at O0 and O3. Imre runs the unchanged body and assertions of `TestGetattrStatic.test_no_dict_no_slots` under CPython’s fixtures. It reads an absent integer attribute with a default and retrieves an unbound string descriptor. In `TestRetrievingSourceCode.test_cleandoc`, CPython runs the original test and assertions with native results. The other 360 IDs have reference results only.

## Cleaning docstrings

`inspect.cleandoc(doc="  Title\n\tBody  \n")` returns `"Title\nBody  "`. It expands tabs, splits on literal LF, and removes the common ASCII-space indentation after the first line. It strips leading ASCII spaces from the first line and removes empty lines at either end. CR, NUL, Unicode spaces, and remaining trailing spaces survive.

The [comparison](cleandoc_cases.py) runs the six rows of CPython’s unchanged `cleandoc_testdata` through the compiled public function. CPython runs `GetSourceBase.setUp`, the original loop, and its `subTest` assertions. Compiling that loop directly stops at the assignment to the builtin name `input` on line 721. Truncating the native output at NUL fails row 2. Removing trailing spaces fails rows 1 and 5.

The typed frontend accepts a string positionally or by the keyword `doc`. A stored callable accepts a positional string. Imre reports binding errors and statically incompatible argument types at compile time. The controls compare those diagnostics with CPython’s runtime exceptions. CPython also accepts some duck-typed inputs. The typed frontend needs a statically known string, including a `str | None` narrowed to `str`.

Runtime-loaded modules use the same bundled Python function. The [CLI fixture](../../../../tests/fixtures/boxed_inspect_coroutine.py) imports it alongside the native coroutine functions and checks keyword calls, tabs, NUL, CRLF, nonbreaking spaces and trailing spaces.

The [wheel comparison](werkzeug_cache_docs.py) imports Werkzeug 3.1.8 from the seven hash-pinned Flask gate wheels. CPython runs the real property factory and descriptors. All 17 class property docs and seven explicit factory calls are compared with native `cleandoc` in ordinary and reachable-only O0/O3 builds.

## Static attribute lookup

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

Pass `--reachable-only` to repeat the inventory in that compiler mode. To reproduce the property doc comparison, run the following with the [pinned wheels](../../../flask/requirements.txt) in `$WHEELHOUSE`.

```sh
python3.14 benchmarks/cpython_stdlib/full/inspect/werkzeug_cache_docs.py \
  --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 \
  --wheelhouse "$WHEELHOUSE" --result /tmp/inspect-werkzeug.json
```

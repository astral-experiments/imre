# Runtime overloads

The entire pinned CPython 3.14.5 `test.test_typing` suite has **712** occurrences. All 712 passed in a disposable test-enabled CPython installation built from the vendored fixtures. This native slice exercises the five original `OverloadTests` methods and reports the other 707 as reference-only, at both O0 and O3. The same counts appear in [native-linux.json](native-linux.json).

| Original case | Body and assertions | Operations and fixtures |
| --- | --- | --- |
| `test_overload_fails`, `test_overload_succeeds` | Imre compiles the entire original method. | Native decorator bindings, dummy calls and error flow; CPython supplies the outer unittest lifecycle. |
| `test_overload_registry_repeated` | Imre compiles the original test and its original `set_up_overloads` helper. | A native assertion checks both list length and function identity at every position after each repeated definition; the outer lifecycle is CPython. |
| `test_overload_registry`, `test_overload_on_compiled_functions` | Original CPython bodies, real `unittest.mock.patch`, and CPython assertions. | A persistent Imre program performs registration, lookup, singleton comparison, private registry size and clearing. CPython supplies the original functions and gives each actual patched dictionary its own native instance. |

The original `test.test_warnings.DeprecatedTests.test_dunder_deprecated` is a separate **client**, outside these 712 IDs. Its class creation, function wrapping, warning decorator and unittest assertions run in CPython. Its two overload registrations, retrieved order and deprecation markers are verified against native objects. [The separate client receipt](warnings-client-linux.json) records these roles and calls. A deliberately reversed lookup fails the original typing order assertion and the original warnings marker assertion.

## Product behavior

A module-level `@overload` declaration registers its original function at definition time, even when the module's only executed statements are definitions. The compiler supports consecutive declarations followed by an implementation and an isolated decorated declaration with a raising dummy. Noncapturing local functions get a fresh identity every time their definition executes. Repeating a definition at the same source line replaces the old registration without disturbing its position. Different modules and qualified names remain separate, snapshots own their elements after `clear_overloads()`, and changing a function's module or qualified name affects later lookups.

The public `overload`, `get_overloads` and `clear_overloads` operate on known, plain source functions. The two original builtin-function probes cover `sum` and `print`; registering either creates a name bucket but stores no function. A retrieved source function can be called through an exact `Callable` annotation if every possible registered source has that signature. The compiler also accepts a direct no-argument call when every source is proven to return `None`. Mixed signatures retain their real identities and support metadata lookup; calling them through a guessed signature is rejected.

CPython supplies the fixture metadata to the two host tests. The bridge instantiates actual native functions at the original code's first source line, including the warning wrapper's line, and sets their public string attributes. Native lookup has to return those specific identities in order. The compiler supports string `__name__`, `__qualname__`, `__module__` and `__deprecated__` in this reflection path. Bound methods, arbitrary decorators, expanded arguments to a dummy, and nonconsecutive static module rebindings remain outside this slice.

## Reproduce

Run with the pinned test-enabled CPython and its verified warnings fixtures:

```sh
python3 benchmarks/cpython_stdlib/full/typing/run.py \
    --imre target/release/imre --runtime target/release/libimre_runtime.a \
    --clang /usr/bin/clang-18 --result /tmp/typing-overloads.json
```

The gate compares normal and reachable-only imported module effects, exercises the original body of a recovered callable after the registry is cleared, checks actual function metadata mutation and error text, and requires unsupported receivers/signatures to fail compilation.

[Raw unchanged-suite entry checks](startup-linux.json) cover ordinary and reachable-only compilation, for both importing and invoking the upstream runner. On the exact #650 parent, reachable-only warnings startup advances from the missing `typing.overload` import to `test.support.import_helper:2 import _imp`. Ordinary warnings startup still stops at the earlier `importlib` import on that parent. Both typing modes still require a starred tuple display at `test_typing.py:696`. No whole-suite entry executes a test.

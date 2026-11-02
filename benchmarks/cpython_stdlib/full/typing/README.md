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

## Literal aliases

All 15 original methods in CPython 3.14.5’s `LiteralTests` passed on public Imre `e3ff3e59` at O0 and O3 with runtime imports. The runner loaded the unchanged class and its base under the original module name. On baseline `d172f092`, 14 of the same methods passed at both levels. The one new pass is `test_dir`, suite ordinal 280. Previously `dir(Literal[1, 2, 3])` raised `NotImplementedError` before entering its subtests. On the reviewed head `aaad9b9b` and the public squash, all three original membership checks passed for `__args__`, `__parameters__`, and `__origin__`. [Method outcomes](literal-linux.tsv) · [Subtest events](literal-subtests-linux.tsv) · [Product change](https://github.com/astral-experiments/imre/pull/4018)

Stock CPython passed all 15 methods both after importing the full `test.test_typing` module and with the isolated class. Suite ordinals 277–291 come from its ordered 712-case inventory. On both public Imre revisions, full-module builds succeeded at O0 and O3, then import of `typing.reveal_type` failed before any methods registered (exit 1). On the merged revision, changing the expected `__origin__` membership to false made the independent directory control fail its assertion at both levels. [Ordered reference](reference-linux.json) · [Process outcomes](literal-terminals-linux.tsv)

Earlier work reported 13 of 15 with native assertion helpers. The later named `typing.List` implementation already passed `test_literals_inside_other_types` at the baseline (ordinal 287). The historical typing total of 107 has no per-case inventory against which to deduplicate these results. [Earlier Literal coverage](https://github.com/astral-experiments/imre/pull/2438) · [Named List support](https://github.com/astral-experiments/imre/pull/3094)

### Reproduce the class runs

From the repository root, stage the vendored CPython sources. The isolated module retains the exact class and base bytes and imports their dependencies.

```sh
suite=benchmarks/cpython_stdlib/full/typing
shared=benchmarks/cpython_stdlib/full/shared/upstream/Lib/test
stage=$(mktemp -d)
for profile in full isolated; do
    mkdir -p "$stage/$profile/test"
    cp "$shared/__init__.py" "$stage/$profile/test/"
    cp "$suite/literal_runner.py" "$stage/$profile/"
done
cp -R "$suite/upstream/Lib/test/." "$stage/full/test/"
cp -R "$shared/support" "$stage/full/test/"
{
    printf '%s\n' 'from unittest import TestCase' 'from typing import T, Literal, List' 'import typing' '' ''
    sed -n '67,71p' "$stage/full/test/test_typing.py"
    printf '\n\n'
    sed -n '2645,2765p' "$stage/full/test/test_typing.py"
} > "$stage/isolated/test/test_typing.py"
(cd "$stage/isolated" && echo '039f50716ac635a54184386294dc9025e6f7be61a74bf3b5798d3cea59ab3411  test/test_typing.py' | sha256sum -c -)
```

Use a test-enabled stock CPython 3.14.5 and set `IMRE`, `RUNTIME`, and `CLANG` to the corresponding paths for the Imre revision being tested. With `profile=isolated` or `profile=full` and `level=0` or `level=3`, run the same driver in both implementations.

```sh
python3.14 -B "$stage/$profile/literal_runner.py"
"$IMRE" build "$stage/$profile/literal_runner.py" --module-root "$stage/$profile" \
    --runtime-imports --no-cache --opt-level "$level" --runtime "$RUNTIME" \
    --clang "$CLANG" -o "$stage/$profile/suite-O$level"
"$stage/$profile/suite-O$level"
```

For the independent controls, copy `literal_dir_control.py` or `literal_repr_control.py` into an empty module root and use it as the entry point in the commands above. Run the same program again with the argument `wrong`. Stock CPython prints the observed value on both runs, exits 0 with the original expectation, and raises `AssertionError` with the altered expectation. [Directory control](literal_dir_control.py) · [Representation control](literal_repr_control.py)

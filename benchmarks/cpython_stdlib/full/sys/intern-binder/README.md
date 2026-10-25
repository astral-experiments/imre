# Native `sys.intern` argument checking

Imre knows that a supported `ModuleSpec.parent` read produces a native `str`.
The `sys.intern` binder lowers the operand before it marks the outer typeshed
overload as checked. It cannot suppress an error in a nested call. The normal
sys gate verifies the [negative source](intern-nested-invalid.py) at both levels.

The [combined source](https://github.com/astral-sh/imre/commit/72850640d4be1991794e23b99486b8baa9872bf3) includes the
[published binder](https://github.com/astral-sh/imre/commit/6573f5088d1f94e1ab01133726766f7c88f89944),
ModuleSpec, string translation, and `ord`. Its tree is `cf4abbfea660a44e1a2abadd58e29d46cf8b4407`.
The [Linux receipt](control-linux.json) compares these programs with stock
CPython 3.14.5 in whole-program and reachable-only modes, at both O0 and O3.

| Source | Observation in each of the four configurations |
| --- | --- |
| [A previously rejected `ModuleSpec.parent`](intern-parent-minimal.py) | Prints `pkg`, with empty stderr. |
| [String identity across modules and codecs](intern-raw-no-narrowing.py) | All 292 output lines, including Latin-1 singletons and longer dynamic names, match CPython without type narrowing in the source. |
| [The sys interning controls](../intern_values.py) | All 29 lines match CPython. |
| [An invalid nested keyword](intern-nested-invalid.py) | Compilation fails at `6:23` with `parameter "value" is positional-only`. Before and after the binder change, the complete stderr from the identical input path has the same SHA-256. The path prefix changes if the source is moved. |

To replay, compile each source with `imre build SOURCE --no-cache --quiet
--opt-level 0 --runtime RUNTIME --clang CLANG -o EXE`, then repeat at level 3
and with `--reachable-only`. For the first three, compare the executable's
stdout, stderr and exit code with `python -B SOURCE` from the pinned release.
For the fourth, inspect the compiler diagnostic without running an executable.
These controls add no upstream IDs or native credit to the blocked `test_intern`.

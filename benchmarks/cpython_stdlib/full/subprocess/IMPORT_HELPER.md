# What the subprocess suite needs from `import_helper`

The unchanged [subprocess suite](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_subprocess.py#L1-L8) imports `test.support.import_helper` at line 5. The [helper](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/support/import_helper.py#L1-L6) imports `_imp` at line 2. Imre therefore needs the module binding to get past this import, even though the subprocess tests did not invoke either of the helper’s private `_imp` overrides on Linux.

| Original CPython fixture | Observed calls from the fixture |
| --- | --- |
| [Subprocess `test_select_unbuffered`](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_subprocess.py#L3303-L3318) | Calls [`import_helper.import_module("select")`](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/support/import_helper.py#L69-L84) once. It delegates to `importlib.import_module` and returns the exact preexisting module object. The test reads four bytes from the child’s unbuffered stdout, then calls `select.select` on that same pipe and asserts it remains readable. |
| [JSON pure and accelerated fixtures](https://github.com/astral-sh/imre/pull/938) | Import `json` fresh and each switches the frozen module override to `-1`, then restores `0`. Neither calls the extension override. |

`import_helper_trace.py` monitors the original code with CPython’s `sys.monitoring`: no fixture, builtin, or import is replaced. It verifies the release blobs, every discovered test ID, and every original skip reason. In two fresh processes per configuration, hash seeds 17 and 909 produced identical receipts. [Default resources](import-helper-linux.json): 312 passes and 41 skips out of 353. [CPU resource enabled](import-helper-cpu-linux.json): 314 passes and 39 skips out of the same 353. There were no helper-issued `_imp` calls during suite import, discovery, fixtures, or methods. Separate controls observe the real frozen override and the extension override’s main-interpreter rejection. These runs are CPython reference evidence and add **zero native passes**. [#801](https://github.com/astral-sh/imre/pull/801) tracks the Imre parent’s pipe read and readiness separately.

## Reproduce the reference trace

Use the CPython 3.14.5 release and source tree at `5607950ef232dad16d75c0cf53101d9649d89115`. Run from a scratch directory, with the normal devbox credentials used by the original UID/GID tests. For example, with the three paths set to absolute locations:

```sh
PYTHONHASHSEED=17 "$CPYTHON" -B "$SUBPROCESS_SUITE/import_helper_trace.py" \
    --cpython-source "$CPYTHON_SOURCE" --result default.json
PYTHONHASHSEED=17 "$CPYTHON" -B "$SUBPROCESS_SUITE/import_helper_trace.py" \
    --cpython-source "$CPYTHON_SOURCE" --cpu --result cpu.json
```

## Native startup and mock restoration

[Canonical receipts](mock-merged-linux.json) compare the four original Imre module import and unittest runner checks before and after [#758](https://github.com/astral-sh/imre/pull/758). All advanced from `from unittest import mock` to the `_imp` import. On the actual merge (`06c089f`), all eight whole-suite builds at O0/O3 stopped at that same import, before any method executed.

The separate [#825](https://github.com/astral-sh/imre/pull/825) fixture ran the original `test_stopped` and `test_send_signal_race` at O0/O3. CPython runs the methods, assertions and `mock.patch` contexts. In `test_stopped`, the native parent dispatched a wait through the shared Imre `Mock`; after restoration a fresh kernel wait returned `ECHILD`. Independent native runs of both `wait` and `poll` saw SIGQUIT, retained the stopped status, and reaped exit 7 after resuming the real child. Direct compilation of the original `mock.patch` context still failed on this revision. These checks establish the parent operations without upgrading the original fixtures to native execution.

# Auditing the native default fixture

This audit asks whether the shared `NativeDefaultMethod` entry can run each discovered unittest occurrence. Its results describe that one runner path. The [primary suite report](native-linux.json) also exercises original host fixtures and host-driven native operations.

Run it from a test-enabled CPython 3.14.5 interpreter with an Imre compiler and matching runtime built from the named commit:

```sh
/path/to/python -B benchmarks/cpython_stdlib/full/unittest/default_fixture_audit.py \
    --cpython-source /path/to/cpython-3.14.5 --compiler-commit <full-imre-commit> \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/unittest-default-fixtures.json.gz
```

Before compiling, the audit checks the original unittest module blobs and reconstructs the Git tree for all 1,639 files under CPython's `Lib/test`. Each staged entry is a package beneath that tree's module root and can import the original `test` package. `Toolchain(..., module_root=path)` supplies that search root; other runner integrations keep their own configured root.

The compressed JSON holds one complete [native evidence](../../NATIVE_EVIDENCE.md) report per optimization level under each of `modes.ordinary` and `modes.reachable`. All 1,090 occurrences remain present. Native successes must match the staged CPython verbose transcript except for elapsed time, and a reviewed operation and assertion role must match the original method hash. A successful return with unreviewed operations remains a blocker. The two reference SIGINT skips retain their exact conditions and sources.

For a blocked method, `evidence.stage` distinguishes an original fixture or module binding the entry cannot reproduce, a compile failure, and a pending operation review. `evidence.upstream_diagnostic` maps the compiler's line and column back to an original method or imported fixture when available. Harness provenance and import-path errors stop the audit.

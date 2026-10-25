# CPython 3.14.5: shutil

The pinned release contains one test module, `Lib/test/test_shutil.py`. Loading it with unittest discovers **223 distinct IDs**, including inherited methods on the bytes and zero-copy classes. The [manifest](UPSTREAM.json) points to the existing byte-for-byte copies of the test and CPython license. The [Linux reference](reference-linux.json) records every ID: 173 pass and 50 skip on this host. Each skip has the reason reported by CPython. These numbers describe the oracle only.

To reproduce the reference with a test-enabled build of the pinned release, use the shared inventory runner from [#479](https://github.com/astral-sh/imre/pull/479):

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/shutil/UPSTREAM.json --result /tmp/shutil-reference.json
```

The runner verifies both the vendored and installed test blobs, discovers the full suite, and runs regrtest in a temporary directory. It requires regrtest's IDs to agree with unittest discovery.

## Native execution

The merged implementation covers [file contents and copy errors (#492)](https://github.com/astral-sh/imre/pull/492), [metadata and extended attributes (#504)](https://github.com/astral-sh/imre/pull/504), and [the missing-PATH defaults for text and bytes (#720)](https://github.com/astral-sh/imre/pull/720). [POSIX deletion (#522)](https://github.com/astral-sh/imre/pull/522) and [structured copy-error filenames (#795)](https://github.com/astral-sh/imre/pull/795) are also on main. This report adds recursive tree copying and callable copy and removal routines.

[evidence-linux.json](evidence-linux.json) contains all 223 occurrences at each optimization level:

| Execution | `-O0` | `-O3` |
| --- | ---: | ---: |
| Fully native, including fixtures | 0 | 0 |
| Native method body, assertions and operations; host fixtures | 8 | 8 |
| Host body and assertions driving native operations | 51 | 51 |
| Mixed host and native operations | 14 | 14 |
| Reference only | 0 | 0 |
| Blocked | 100 | 100 |
| Observed failures | 0 | 0 |
| Original CPython skips | 50 | 50 |
| **Total** | **223** | **223** |

Of the 14 mixed methods, 8 call original Python deletion handlers, 4 include host callbacks or path-like conversions during tree copying, and 2 use the original default-search-path mocks. In those two methods, the native child has no `PATH`: it requests `os.confstr("CS_PATH")` and, after `ValueError`, `os.defpath`, then searches for the executable.

Of the 100 blocked methods, 3 deletion methods and 1 tree-copy method need OS mocks. Each of the other 96 records the first complete-method diagnostic. The 50 skips retain their original CPython reasons.

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/full/shutil/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/shutil-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/shutil/reference-linux.json /tmp/shutil-evidence.json
```

The first command writes all outcomes and exits unsuccessfully when an assertion fails. The second checks the report against the full pinned denominator. [mock-fixtures.json](mock-fixtures.json) identifies the original `unittest.mock` call sites, decorators and affected IDs, including those skipped by CPython.

The [complete TestCopy run](copy-linux.json) also checks the real 1 KiB user extended attribute outside the upstream trusted-attribute skip. Its unchanged `test_copy_return_value` iterates over native `shutil.copy` and `shutil.copy2`, makes four copies, and compares the returned filenames in Imre. The four directories come from the original CPython fixture. The two [copyfile alias methods](copyfile-linux.json) now drive a native copy and a native recursive deletion each; the host verifies that the directory is gone after the original `finally` block.

Copy and metadata errors retain the original text or bytes filename. A failed symlink copy stores the link target in `filename` and the destination in `filename2`, including when their types differ. The [POSIX filename controls](copy_error_controls.py) compare eight real failures with CPython at both levels. For a text path that CPython decodes using surrogateescape, Imre preserves the OS exception, its printed name, and its other fields; materializing that particular `filename` or delivering it as a text callback argument remains blocked by the runtime's Unicode-scalar string representation. Bytes fields preserve arbitrary filename bytes.

The [complete TestRmTree run](rmtree-linux.json) records each handler invocation. The native walker chooses onexc over onerror, reports the failing operation and exception, and resumes after the original handler returns. The Linux test transport supplies the host OS function object and reconstructs the deprecated onerror traceback in CPython. These eight methods are mixed; passing them does not demonstrate that the compiler accepts arbitrary Python callback arguments.

The [complete TestCopyTree run](tree-linux.json) retains all 14 original methods: 9 host-driven native operations, 4 mixed, and 1 blocked. Native traversal copies file contents, permissions, timestamps, extended attributes, and symlinks. It runs supplied copy functions and ignore filters, and retains failures in the mutable list of `(source, destination, message)` tuples. In `test_copytree_winerror` the original `os.chmod` patch raises `PermissionError('ka-boom')`; it cannot replace the native syscall. The [callback](tree_callback_controls.py), [filter](tree_filter_controls.py), and [structured error](tree_error_controls.py) controls are supplemental and add no upstream IDs.

## Complete-method attempts

The [copytree merge run](https://github.com/astral-sh/imre/actions/runs/40180794658) verified tree `61d5337dec05ab707494ca4150c05159ead3950b`, exactly the tree merged in [#829](https://github.com/astral-sh/imre/pull/829). Both missing-`PATH` methods ran with the environment variable absent in two native processes per optimization level. Their fixture, environment patches and assertions ran on CPython.

## Moving files

[move_ops.py](move_ops.py) selects every original `TestMove` occurrence, including its two Linux reference skips. It attempts unchanged methods and their original helper bodies when the compiler can represent the fixture. For other cases, it calls the native operation from the untouched CPython method. A test-only interposer sends actual rename operands to the original mock; native code receives its errno and performs the fallback. A Python copy callback keeps the result mixed.

[move_cross_device.py](move_cross_device.py) also compares file contents, directory trees, symlink targets, callbacks, and the source left after a failure on two real Linux mounts. It first proves the kernel returned `EXDEV` on each operand pair. These supplemental controls add no upstream IDs.

## Reproducing complete-method attempts

[method-attempts-linux.json](method-attempts-linux.json) records the compiler and runtime hashes, original method and decorator hashes, and the first diagnostic for each blocked build. The [hosted run](https://github.com/astral-sh/imre/actions/runs/40177629056) tested tree `d6aa3eccebad9fc011341575e3745c7d063ca482` (main `71ffc22`, copytree `b88820e`). All 173 reference passes were attempted under their original CPython fixtures at both levels. Eight native bodies completed; 165 could not compile. After [native _opcode (#800)](https://github.com/astral-sh/imre/pull/800), 83 reach `import warnings` in the stock `test/support/__init__.py:21`; 40 reach the unsupported `unittest` import used by the original mocks and 10 stop at `errno`. The first diagnostic for each other method is in its record. The host-driven and mixed results retain their measured execution roles.

Reproduce the method attempts with the pinned interpreter and [vendored shared fixtures](../shared/UPSTREAM.json):

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/full/shutil/attempt_methods.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --compiler-tree d6aa3eccebad9fc011341575e3745c7d063ca482 --result /tmp/method-attempts-linux.json
```

Pass `--method-attempts /tmp/method-attempts-linux.json` to the evidence runner to use this result.

# CPython 3.14.5: shutil

The pinned release contains one test module, `Lib/test/test_shutil.py`. Loading it with unittest discovers **223 distinct IDs**, including inherited methods on the bytes and zero-copy classes. The [manifest](UPSTREAM.json) points to the existing byte-for-byte copies of the test and CPython license. The [Linux reference](reference-linux.json) records every ID: 173 pass and 50 skip on this host. Each skip has the reason reported by CPython. These numbers describe the oracle only.

To reproduce the reference with a test-enabled build of the pinned release, use the shared inventory runner from [#479](https://github.com/astral-sh/imre/pull/479):

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/shutil/UPSTREAM.json --result /tmp/shutil-reference.json
```

The runner verifies both the vendored and installed test blobs, discovers the full suite, and runs regrtest in a temporary directory. It requires regrtest's IDs to agree with unittest discovery.

## Native execution

The implementation covers [file contents and copy errors (#492)](https://github.com/astral-sh/imre/pull/492), [metadata and extended attributes (#504)](https://github.com/astral-sh/imre/pull/504), [the missing-PATH defaults for text and bytes (#720)](https://github.com/astral-sh/imre/pull/720), [POSIX deletion (#522)](https://github.com/astral-sh/imre/pull/522), [structured copy-error filenames (#795)](https://github.com/astral-sh/imre/pull/795), and [recursive tree copying (#829)](https://github.com/astral-sh/imre/pull/829). This report includes [moves and their cross-device fallbacks (#902)](https://github.com/astral-sh/imre/pull/902).

[evidence-linux.json](evidence-linux.json) contains all 223 occurrences at each optimization level:

| Execution | `-O0` | `-O3` |
| --- | ---: | ---: |
| Fully native, including fixtures | 0 | 0 |
| Native method body, assertions and operations; host fixtures | 15 | 15 |
| Host body and assertions driving native operations | 73 | 73 |
| Mixed host and native operations | 38 | 38 |
| Reference only | 0 | 0 |
| Blocked | 47 | 47 |
| Observed failures | 0 | 0 |
| Original CPython skips | 50 | 50 |
| **Total** | **223** | **223** |

Of the 38 mixed methods, 14 use the original rename mock, copy callback or path conversion during moves; 10 use host operands or callbacks for [archive dispatch](archives.md); 8 call original Python deletion handlers, 4 use host callbacks or path conversions during tree copying, and 2 use the original default-search-path mocks. In those last two methods, the native child has no `PATH`: it requests `os.confstr("CS_PATH")` and, after `ValueError`, `os.defpath`, then searches for the executable.

The 47 blocked methods include all 29 Linux zero-copy methods, 4 stream open/close methods, 3 `copyfileobj` methods, 4 terminal-size methods, 2 miscellaneous methods, 3 deletion methods, 1 tree-copy method and 1 public API inventory method. The 50 skips retain their original CPython reasons.

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

The [paired run](https://github.com/astral-sh/imre/actions/runs/40184043145) observed 4 complete native move bodies with original host fixtures, 8 host-driven native operations, 14 mixed, and 2 reference skips at each level. Both file-move bodies read and compare the actual bytes, and both return-value bodies compare the resulting path. The complete 223-occurrence report passed; a separate cross-device control in that run found that the type checker rejected non-callable copiers before filesystem access.

[move_cross_device.py](move_cross_device.py) also compares file contents, directory trees, symlink targets, callbacks, and the source left after a failure on two real Linux mounts. It first proves the kernel returned `EXDEV` on each operand pair. These supplemental controls add no upstream IDs.

## Reproducing complete-method attempts

[method-attempts-linux.json](method-attempts-linux.json) records the compiler and runtime hashes, original method and decorator hashes, and the first diagnostic for each blocked build. The devbox used the [verified hosted compiler and runtime](https://github.com/astral-sh/imre/actions/runs/40194394062) from archive and random tree `aa9cdee42fa02f88f14dcebbdbe06f972bdeb660`. All 173 reference passes were attempted under their original CPython fixtures at both levels. Fifteen native bodies completed; 158 could not compile. Of these, 105 reached the starred tuple in `test/support/os_helper.py:778`, tracked in [#958](https://github.com/astral-sh/imre/pull/958). Each remaining diagnostic is recorded per method.

Reproduce the method attempts with the pinned interpreter and [vendored shared fixtures](../shared/UPSTREAM.json):

```sh
/path/to/cpython-3.14.5/python -B benchmarks/cpython_stdlib/full/shutil/attempt_methods.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --compiler-tree aa9cdee42fa02f88f14dcebbdbe06f972bdeb660 --result /tmp/method-attempts-linux.json
```

Pass `--method-attempts /tmp/method-attempts-linux.json` to the evidence runner to use this result.

## Shared unittest.mock and whole-suite startup

[This run](https://github.com/astral-sh/imre/actions/runs/40187233993) used the published Linux compiler and runtime from [unittest.mock #758](https://github.com/astral-sh/imre/pull/758), at commit `4ffa8406bae7e321093b5a4bb6550e2d4f34e502`, tree `e0d012f12c138cd432f7a72e0dc20c42f21523d4`. It predates the move implementation. The [startup report](mock-758-startup-linux.json) records the compiler and runtime hashes and both optimization levels.

* Upstream line 4 imports `unittest.mock`. A native `Mock` records a call, checks its argument, resets, and accepts `assert_not_called`.
* The original line 53 assignment uses `patch('os.chdir', side_effect=AssertionError(...))`. The compiler stops on `unittest.mock.patch`. On CPython the [control](mock_startup.py) intercepts and records the call, then proves that the real `chdir` works after normal and exceptional context exits.
* Both the unchanged module import and a default test loader plus `TextTestRunner` stop at upstream line 11, `import errno`, in whole-module and reachable-only builds. No native suite IDs run through these entry points.

The [hosted CPython reference](mock-758-reference-linux.json) contains 174 passes and 49 original skips across all 223 IDs. Its zstd reader is available; the devbox reference skips that test. The [complete-method attempts](mock-758-method-attempts-linux.json) used the hosted reference: seven bodies and assertions ran in Imre under original host fixtures, 167 could not compile, and 49 had original skips, at both O0 and O3. All 40 methods previously blocked on the mock import reached a later diagnostic. These observations are specific to the shared mock head and do not replace the primary module evidence above.

To reproduce startup, use a test-enabled CPython 3.14.5 and the pinned CPython checkout:

```sh
/path/to/python -B benchmarks/cpython_stdlib/full/shutil/mock_startup.py \
    --cpython-source /path/to/cpython --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
    --clang /path/to/clang --compiler-tree e0d012f12c138cd432f7a72e0dc20c42f21523d4 \
    --result /tmp/shutil-mock-startup.json
```

For complete methods, generate the reference with `suite_inventory.py` on the same host and pass it to `attempt_methods.py --reference`.

### On canonical main

The [four original startup modes](canonical-startup-transitions-linux.json) all stopped at line 4 before the shared mock landed. On [#758](https://github.com/astral-sh/imre/pull/758) (`06c089f`), they reached `import errno` at line 11. On [#652](https://github.com/astral-sh/imre/pull/652) (`5ca29ab`), they reached `import random` at line 14. The import and unittest-runner entries were each checked with and without reachability pruning. None executed a test. The independent errno rerun reproduced the coordinator's four checks byte for byte.

The [merged mock](mock-758-landed-startup-linux.json) and [merged errno](errno-652-landed-startup-linux.json) receipts also ran at O0 and O3 with the compiler and runtime from the indicated Git trees. Both recorded actual native Mock calls and resets. The original `no_chdir` patch still failed to compile at both levels; CPython trapped the attempted directory change inside the patch and successfully changed directories after both a normal and an exceptional exit. These startup and fixture checks add no passes to the 223 occurrences.

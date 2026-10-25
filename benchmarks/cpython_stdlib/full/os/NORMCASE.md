# POSIX normcase

Both `os.path.normcase(s)` and `posixpath.normcase(s)` return `os.fspath(s)`. There is no case conversion or separator replacement. Text and bytes retain their identity; a pathlike object supplies its spelling through its type’s `__fspath__` method. A failing protocol call propagates, and a non-string, non-bytes return raises `TypeError`.

[normcase.py](normcase.py) verifies the original CPython 3.14.5 test blobs, discovers both matching POSIX path methods, and runs them under their original fixtures in a temporary directory. Its [Linux results](normcase-linux.json) cover two supplemental `test_posixpath` occurrences at each optimization level. Neither occurrence belongs to the `test_genericpath` suite discovered on its own.

| Original method | Body and assertions | Operations and fixture |
| --- | --- | --- |
| `PathLikeTests.test_path_normcase` | Imre compiles the unchanged test and its unchanged assertion helper. | The real CPython fixture creates and cleans up its test file. Its two strings are transferred independently; the native fixture reconstructs the string-valued FakePath, invokes its protocol, and calls the public operation twice. |
| `PosixCommonTest.test_normcase` | CPython runs the unchanged method and assertions. | Twelve native calls cover idempotence, empty strings and bytes, and six rejected types. The thirteenth call uses CPython: Imre cannot construct the invalid `bytearray` input. The full method’s compile diagnostic is recorded separately. |

The runner also compiles the two platform assignments from the pinned `test_fnmatch.py` byte for byte. On POSIX both `IGNORECASE` and `NORMSEP` are false. These are import-time checks, not test occurrences. The [direct regression](../../../../tests/fixtures/paths/posix_normcase.py) additionally checks identity, exceptional protocol results, evaluation order, and unchanged raw spellings.

Run the script with a test-enabled CPython 3.14.5 on POSIX, passing `--cpython-source`, `--imre`, `--runtime`, `--clang`, `--opt-level 0|3` and `--result`.

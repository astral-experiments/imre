# What the four `copyfile` open and close tests require

The [pinned Linux trace](open-fixtures-reference-linux.json) runs all four unchanged `TestCopyFile` methods under CPython’s original fixtures and assertions. It records **reference execution only**. The four cases remain in the native denominator.

| Original method | Observable behavior |
| --- | --- |
| `test_w_source_open_fails` | The replacement `open` raises for the source. No destination is opened and no context is entered. |
| `test_w_dest_open_fails` | The source context receives the exact `OSError` raised when opening the destination, with its traceback, and suppresses it. |
| `test_w_dest_close_fails` | Both `Faux.__enter__` calls return `None`. After the fast paths give up, the buffered read raises `AttributeError`. The destination’s `__exit__` raises a new `OSError('Cannot close')`; the source receives that same new exception and suppresses it. |
| `test_w_source_close_fails` | The destination suppresses the buffered read error. The source therefore receives three `None` values, then raises `OSError('Cannot close')` itself. |

In the JSON, an object number identifies the same live exception throughout one method. `support.swap_attr` installs an ordinary Python function as `shutil.open`, and cleanup restores the previous attribute or deletes the new one. Running a patched `Mock` object stored in an imported source module ([#961](https://github.com/astral-sh/imre/pull/961)) is a different fixture requirement.

To reproduce, use a test-enabled CPython 3.14.5 build at the manifest’s commit:

```sh
/path/to/cpython/python -B benchmarks/cpython_stdlib/full/shutil/open_fixture_trace.py \
    --result /tmp/shutil-open-fixtures.json
```

The tracer verifies the test, license, and the interpreter’s `Lib/shutil.py` against their pinned Git blobs. The unittest case, its context managers, and the high-level operation execute unmodified in a temporary directory.

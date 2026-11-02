# Intact typing discovery on Linux

The input is the complete CPython 3.14.5 `test.test_typing` module at
`5607950ef232dad16d75c0cf53101d9649d89115`. A stock interpreter built from
that revision passed all 712 original IDs with zero skips in each of the three
stock runs below. These include 11 doctests added by `load_tests`. The
[original start and outcome events](stock-linux.events) follow the order of the
[reference ledger](../reference-linux.json).

## Imre 06c9fd7d

| Entry point | O0 | O3 |
| --- | --- | --- |
| Ordinary `imre -B -m discover` | ImportError at line 33 | ImportError at line 33 |
| Boxed direct import (`--runtime-imports`) | ImportError at line 33 | ImportError at line 33 |
| Filesystem discovery (`TestLoader.discover`) | Three synthetic loader errors | Three synthetic loader errors |
| Direct import, static build | Compile error at line 715 | Compile error at line 715 |

The ordinary module invocation and the boxed entry both reach
`from typing import reveal_type` at line 33 and exit before module import
completes. Filesystem discovery reports `DISCOVERED 3` and `RUNNER_COUNT 3`.
The three `unittest.loader._FailedTest` entries record distinct import failures.
Loading `test.test_typing` raises `NotImplementedError: runtime source: StmtClassDef`.
Loading `test.support` fails at `@types.coroutine`, and loading
`test.typinganndata.partialexecution` raises `NotImplementedError: runtime source: StmtAnnAssign`.

The static build rejects the starred tuple display at line 715, column 29.
Across the four entry points, zero of the 712 original IDs reached a native
callback. The historical count of 107/712 and the selected-class results
have separate ledgers. [Commands, process streams and exit codes](06c9fd7d-linux.json)

## Reproduce

From the repository root, stage the unmodified suite, test data and support
package. The drivers import `typing` from the implementation under test.

```sh
suite=benchmarks/cpython_stdlib/full/typing
shared=benchmarks/cpython_stdlib/full/shared/upstream/Lib/test
stage=$(mktemp -d)
cp -R "$suite/upstream/Lib/test" "$stage/"
cp "$shared/__init__.py" "$stage/test/"
cp -R "$shared/support" "$stage/test/"
cp "$suite/intact-discovery/"*.py "$stage/"
cd "$stage"
echo '28d17772f523800f02a5bf1e0cc012779d7df1f616fec7cc34d922dcba7ee1cc  test/test_typing.py' | sha256sum -c -
```

Set `PYTHON` to the pinned test-enabled stock interpreter. The first command
uses `TestLoader.discover`. Both invocations of `discover.py` import the module
directly and record unittest callbacks with their original IDs and skip reasons.

```sh
"$PYTHON" -B unittest_discover.py
"$PYTHON" -B discover.py
"$PYTHON" -B -m discover
```

Set `IMRE`, `RUNTIME`, and `CLANG` to absolute paths to the matching compiler,
runtime archive, and linker. The following Bash commands reproduce each native
entry at O0 and O3. A failed build has no run result.

```bash
for level in 0 3; do
    IMREFLAGS="--no-cache --clang $CLANG --runtime $RUNTIME --opt-level $level" \
        "$IMRE" -B -m discover
    for mode in filesystem boxed static; do
        entry=discover.py
        flags=()
        case "$mode" in
            filesystem) entry=unittest_discover.py ;;
            boxed) flags+=(--runtime-imports) ;;
        esac
        exe="$stage/$mode-O$level"
        "$IMRE" build "$entry" --module-root "$stage" --no-cache \
            --clang "$CLANG" --runtime "$RUNTIME" --opt-level "$level" \
            "${flags[@]}" -o "$exe" && "$exe"
    done
done
```

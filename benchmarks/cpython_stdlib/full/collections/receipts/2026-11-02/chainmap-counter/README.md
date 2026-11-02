# ChainMap lookup and Counter copying

Each process ran one original from CPython 3.14.5's `test.test_collections` through `unittest`. `TestChainMap.test_basics` exercises lookup, mutation and parent maps. `TestCounter.test_copying` exercises shallow and deep copying, a repr round trip and pickle protocols 0 through 5. The two IDs each contribute one occurrence.

All six selected processes exited 0. An earlier O3 compilation reached its 800-second limit before running tests. The same source and compiler options built successfully with a 1,800-second limit.

The receipt gives the signed exit and output checksums for each stock, native O0 and native O3 process. Its callback lines include registration, runner counts and all subtests. They can be compared across modes independently of the absolute test-module path printed at process startup.

## Linux replay

Use a stock CPython 3.14.5 interpreter and a checkout at `5607950ef232dad16d75c0cf53101d9649d89115`. Set `CPYTHON` to the checkout and `PYTHON` to that interpreter. Set `IMRE` and `RUNTIME` to the compiler and static runtime built together from Imre `63870c091308c11fc6789a3884f67f7c24f73ac0`, and `CLANG` to Clang 18. Use absolute paths for all five variables. Run this from the directory containing this receipt.

```bash
set -eu
stage=$(mktemp -d)
git -C "$CPYTHON" archive 5607950ef232dad16d75c0cf53101d9649d89115 Lib/test |
    tar -C "$stage" --strip-components=1 -xf -
cp events.py selected.py "$stage/"
cd "$stage"
export PYTHONDONTWRITEBYTECODE=1 PYTHONHASHSEED=1
unset PYTHONPATH PYTHONHOME IMREFLAGS IMRE_ENCODED_FLAGS IMRE_FORCE_RUNTIME_IMPORTS IMRE_RUNTIME_IMPORTS

run_pair() {
    "$@" test_collections TestChainMap test_basics
    "$@" test_collections TestCounter test_copying
}

run_pair "$PYTHON" -B selected.py
for level in 0 3; do
    "$IMRE" build "$stage/selected.py" --module-root "$stage" --no-cache \
        --runtime-imports --opt-level "$level" --runtime "$RUNTIME" \
        --clang "$CLANG" -o "$stage/selected-O$level"
    run_pair "$stage/selected-O$level"
done
```

# Compiling the original test.support fixture

CPython 3.14.5 discovers 1,090 occurrences in `test.test_unittest`. Twenty of its original methods load `test.support` from their default fixture. The [consumer runner](support_consumers.py) takes the same 20 IDs from the [original audit](default-fixture-linux.json.gz), verifies their method hashes, and runs each against the original fixture in ordinary and reachable mode at O0 and O3. It also runs the original logging module setup, class setup, cleanup, teardown, and body of `BasicConfigTest.test_strformatstyle` as a separate witness. All 21 original cases pass on CPython.

## Fields initialized by the constructor

CPython's [memory watchdog](../shared/upstream/Lib/test/support/__init__.py#L1149) initializes `procfile` with a literal string's `format` method and `started` with `False`. Imre now infers the two instance fields and evaluates the constructor normally. The same inference lets CPython's original [BufferedWriter initializer](upstream/Lib/test/test_unittest/support.py#L142) execute in all four configurations. Invoking the complete writer reaches its string-field `+=` at line 147.

| Original entry | Before | After | Fully native upstream cases |
| --- | --- | --- | --- |
| 20 unittest consumers and original logging fixture, ordinary O0/O3 | `test/support/__init__.py:1155:9`, undeclared `procfile` | `:1543:15`, unsupported base of `Matcher(object)` | 0 |
| Same entries, reachable O0/O3 | `test/support/__init__.py:1613:5`, platform conditional in the body of `PythonSymlink` | Same location and diagnostic | 0 |
| Original watchdog initializer in isolation | Undeclared `procfile` | `os.getpid()` at original line 1155, column 56 | Fixture operation blocked |

Importing the complete `test_logging.py` first reaches `configparser` at line 27 in ordinary mode and `pickle` at line 32 in reachable mode. The fixture projection's CPython transcript and the complete module run are included with the individual diagnostics.

The [Linux evidence](support-constructor-linux.json.gz) compares `86fde257dc4205df1ed8a06d4d6fb841cd711e0b` with `d4dbb49963c4fc32bf345215df0b3b13263f4fb0`. Both compilers use the 416 files staged from `820c40311a6a176698e34b29a25b03c8d84e97c1`. The manifest digest is `899b4e67f62cdc018e3e5900fb70e6d337673626a6d892ed10fdb2ac958284c1`. It covers the path and CPython Git blob of each file. The `before` and `after` reports preserve each occurrence, optimization level, body, assertions, operation, fixture, and diagnostic. The [constructor control](../../../../tests/fixtures/classes/constructor_fields.py) compares independent instances, argument effects, and both a failing lookup and a missing format key with CPython.

To reproduce the fixture from this checkout, use a CPython 3.14.5 interpreter with its pinned test package. The fixture export keeps the measured denominator independent of later manifest additions.

```sh
mkdir -p /tmp/imre-support-fixture-source
git archive 820c40311a6a176698e34b29a25b03c8d84e97c1 -- benchmarks/cpython_stdlib |
    tar -x -C /tmp/imre-support-fixture-source
"$PYTHON" -E -B benchmarks/cpython_stdlib/full/unittest/support_consumers.py \
    --stage-script /tmp/imre-support-fixture-source/benchmarks/cpython_stdlib/full/stage_tests.py \
    --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" --result /tmp/unittest-support.json
```

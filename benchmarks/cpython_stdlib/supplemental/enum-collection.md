# Supplemental enum collection probe

This is a historical collection measurement at Imre revision [`95908fb0`](https://github.com/astral-experiments/imre/commit/95908fb0acadc2a67304a8080e584deaef395746), on Linux x86-64 with CPython 3.14.5. It is outside the frozen 45-family totals and supplies no native test credit. The [compact receipt](enum-959-collection.json) records the inputs, exits, counts, and raw stream hashes.

The unchanged CPython [`test_enum.py`](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_enum.py#L14) came from commit `5607950ef232dad16d75c0cf53101d9649d89115` (SHA256 `19b6bf1eb6c46023ba6d8575480728a36c0e719d6df312725d8abacef42b17d5`). The fixture staged `Lib/test` without CPython's `Doc` tree, so the suite's optional documentation-file hooks did not add cases. Stock collected 1,081 unique IDs: 1,077 unittest methods and four `enum` doctests.

At native O0, the ordinary runner first exited 1 because its dynamic import of `test.test_enum` had no linked body. With `--include-module test.test_enum`, `--list-cases` exited 0 and printed zero IDs. This was a failed collection: the upstream runner [silently omits `_FailedTest` placeholders](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/libregrtest/findtests.py#L80-L88). The unittest loader returned one placeholder and one error:

```text
Failed to import test module: test_enum
  File ".../Lib/test/test_enum.py", line 14, in <module>
    from enum import Enum, EnumMeta, IntEnum, StrEnum, EnumType, Flag, IntFlag, unique, auto
ImportError: cannot import name 'EnumMeta' from 'enum' (<imre stdlib>/enum.py)
```

The stock, native, and loader probes had 60-, 150-, and 120-second bounds respectively; none timed out. O3 and upstream test bodies were not run because O0 collection did not succeed.

To reproduce with the pinned CPython source, a separate fixture directory, the 959 CLI and its matching runtime, and Clang:

```sh
mkdir -p "$fixture/Lib"
cp -a "$cpython_src/Lib/test" "$fixture/Lib/test"
export PYTHONPATH="$fixture/Lib" PYTHONDONTWRITEBYTECODE=1 LC_ALL=C.UTF-8 TZ=UTC
"$python3145" -B -m test --list-cases test_enum > stock.ids
export IMREFLAGS="--opt-level 0 --no-cache --runtime $runtime --clang $clang"
"$imre959" -B -m test --list-cases test_enum
export IMREFLAGS="$IMREFLAGS --include-module test.test_enum"
"$imre959" -B -m test --list-cases test_enum
"$imre959" -B -c 'import unittest; l=unittest.TestLoader(); s=l.loadTestsFromName("test.test_enum"); print(s.countTestCases(), len(l.errors)); print(*l.errors, sep="\n")'
```

The support-table boundary at public main [`e912096a`](https://github.com/astral-experiments/imre/commit/e912096aeb1a04dadda058c000e7c5cc1df90ac1) was checked in source, not by running that head: the [enum provider](https://github.com/astral-experiments/imre/blob/e912096aeb1a04dadda058c000e7c5cc1df90ac1/crates/imre-compiler/stdlib/enum.py) and [native enum bindings](https://github.com/astral-experiments/imre/blob/e912096aeb1a04dadda058c000e7c5cc1df90ac1/crates/imre-runtime/src/dynamic/stdlib.rs#L581-L586) export neither `EnumMeta` nor `EnumType`. The existing [ordinary Enum fixture](../../../tests/fixtures/classes/ordinary_enums.py) still documents the narrower named-member behavior.

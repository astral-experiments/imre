# dict.fromkeys

This suite runs CPython 3.14.5’s [DictTest.test_fromkeys](upstream/Lib/test/test_dict.py#L335) at both optimization levels. It records one method from test_dict separately from the [collections suite](../README.md). The [reference inventory](reference-linux.json) and [native report](evidence-linux.json) record the results.

Run `reference.py` with test-enabled CPython 3.14.5 to reproduce the reference inventory. Run `run.py --help` for the compiler, runtime, and output arguments for the native run.

The [Imre fixture](../../../../../tests/fixtures/containers/dict_fromkeys.py) also checks shared values, argument evaluation order, and bound methods. The [TextWrapper probe](textwrapper.py) exercises the call in stock CPython source. Their results are separate from the upstream method count. The [earlier branch reports](https://github.com/astral-experiments/imre/blob/6d837c0f0937ff640d28d21f3d0773afb137b515/benchmarks/cpython_stdlib/full/collections/fromkeys/README.md) contain the individual measurements.

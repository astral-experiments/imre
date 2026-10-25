# CPython 3.14.5 functools

The pinned CPython tree contains one suite, `Lib/test/test_functools.py`, and
no `test_functools/` package. The test and PSF license are byte-for-byte
copies of the Git blobs recorded in `UPSTREAM.json`.

| Result on Linux | IDs |
| --- | ---: |
| Discovered (19 classes, including pure Python and accelerator variants) | 325 |
| Passed on the reference CPython build | 325 |
| Reference skips, errors or failures | 0 |

`reference-linux.json` records the outcome of every ID. These are reference
results only; the native denominator remains 325. Neither standalone API
examples nor successful CPython execution are native suite passes.

To reproduce using the shared inventory runner and a test-enabled CPython
3.14.5 build:

```sh
python3.14 benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/functools/UPSTREAM.json \
    --result /tmp/functools-reference.json
```

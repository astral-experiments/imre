# Argparse FileType reference behavior

The [FileType reference trace](filetype-reference-linux.json) records calls during
all 1,893 unchanged tests. The 93 FileType occurrences comprise 84 generated
and nine source-defined routines. No other occurrence called the traced FileType
methods or file comparers.
All 93 passed on CPython. The [per-occurrence Imre report](evidence-linux.json) at
`8c29f1ac2d63f4040bd7cca2ab92d7d03337f08a` classifies all 93 as blocked at
O0 and O3. This reference trace contributes zero native occurrences.

| Observed in the 93 occurrences | Count |
| --- | ---: |
| `FileType.__call__` conversions | 282 |
| Ordinary returned file objects: text / buffered reader / buffered writer | 90 / 30 / 42 |
| Identity matches with stdin / stdin.buffer / stdout / stdout.buffer | 12 / 12 / 24 / 24 |
| Mocked successful opens | 5 |
| Conversions that raised `ArgumentTypeError` / invalid-mode `ValueError` | 42 / 1 |
| Reads / writes / closes in the original file comparers | 72 / 90 / 162 |

The rows cover each test's lifecycle, including any setup and cleanup. The trace
also records 13 `FileType` constructions during module import. The read, write
and close counts come from returns of the actual bound methods on the object
passed to the upstream comparers, rather than just entry to their equality methods.
For `-`, the original assertions compare the live standard streams and their binary
buffers by identity. Returning an equivalent filename or reopening a descriptor
does not satisfy that operation.

```sh
python3.14 -B benchmarks/cpython_stdlib/full/argparse/trace_filetype.py \
  --result /tmp/argparse-filetype-reference.json
```

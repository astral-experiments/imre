# CPython tempfile tests

## Whole suite on public #8087

CPython 3.14.5 passed 112 of the 118 original methods and skipped six. Public #8087 passed 110 and skipped eight in both FILE and named entry at O0 and O3. Each of those 110 also passed on stock. The independently tested [shared mkdir operation](https://github.com/astral-experiments/imre/pull/8120) preserved the same per-method results.

After withdrawing the source, support files and caches, every saved native executable passed 109, skipped eight and failed one. `TestMkstempInner.test_noinherit` starts the original `tf_inherit_check.py` helper to examine a descriptor in a new process. The helper file was absent in these runs. [All 118 methods and their original source lines](paired-public-8087.tsv)

# Public JSON options in `test_speedups`

Three methods in the [pinned `test_speedups.py`](../../upstream/Lib/test/test_json/test_speedups.py) exercise public JSON APIs. Each occurs once, under `CTest`. All three pass on stock CPython 3.14.5 and remain ordinary blockers in the [216-occurrence Imre report](evidence-linux.json). The pure CPython observations below explain the fixture differences; they add no test IDs or native passes.

| Original method | Accelerated CPython | Pure CPython comparison |
| --- | --- | --- |
| `TestDecode.test_bad_bool_args` | `JSONDecoder(strict=BadBool())` raises `ZeroDivisionError` at construction, even for `""` or `{}`. | Construction succeeds. Decoding a literal control character tests `strict` and raises; decoding `""`, `{}` or `1` succeeds. |
| `TestEncode.test_bad_bool_args` | All five options raise `ZeroDivisionError` during `encode({'a': 1})`. | `ensure_ascii`, `check_circular` and `sort_keys` raise during encoding. The pure encoder finishes without testing the truth of `skipkeys` or `allow_nan` for this input. |
| `TestEncode.test_unsortable_keys` | Sorting `{'a': 1, 1: 'a'}` raises `TypeError` when comparing `int` and `str` keys. | Same exception and message. |

At O0 and O3 on [main `f259c3d`](https://github.com/astral-sh/imre/commit/f259c3d26d7ab15f75a1e4db07620305f5099a45), the two complete `BadBool` methods stop at their unannotated local helpers. The complete mixed-key method stops at `self.json.encoder`. Separate probes reproduce the native default `loads` and decoder results, and distinguish three outstanding problems: the compiler rejects upstream `BadBool.__bool__`, `JSONDecoder(strict=False)` rejects its keyword, and the encoder call for mixed keys does not compile. The [Linux receipt](speedup-options-linux.json) records the full-method and operation diagnostics separately, with hashes of the source and binaries. Neither of the successful controls completes an upstream method.

To reproduce with the [canonical Linux archive](https://github.com/astral-sh/imre/actions/runs/40210416327) and the pinned CPython source:

```sh
python3.14 -B benchmarks/cpython_stdlib/full/json/speedup_options.py \
  --cpython-source "$CPYTHON_SOURCE" --imre "$IMRE" --runtime "$IMRE_RUNTIME" \
  --clang /usr/bin/clang-18 --imre-revision f259c3d26d7ab15f75a1e4db07620305f5099a45 \
  --result /tmp/json-speedup-options.json
```

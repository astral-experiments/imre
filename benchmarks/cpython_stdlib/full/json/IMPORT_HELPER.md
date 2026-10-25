# How the JSON fixtures use `_imp`

The [reference trace](import-helper-linux.json) observed the original [JSON package](../../upstream/Lib/test/test_json/__init__.py) using the pinned [import helper](../shared/upstream/Lib/test/support/import_helper.py). CPython 3.14.5 ran 216 occurrences: 215 passed and the original large-memory test skipped. Four IDs occurred twice, including both backend sanity tests. This is reference evidence; it adds no Imre pass or exclusion.

| Original fixture | Frozen override at line 103 | At line 107 | Scanner module |
| --- | --- | --- | --- |
| Accelerator, `fresh=['_json']` (package line 11) | `-1`; `_json` absent | `0`; `_json` present | `_json` |
| Pure, `blocked=['_json']` (package line 12) | `-1`; `_json` bound to `None` | `0`; still blocked | `json.scanner` |

Both fixtures used `usefrozen=False` and `deprecated=False`. Each restored the exact prior `_json`, `json`, `json.decoder`, `json.encoder`, and `json.scanner` objects. The [tracer](import_helper_trace.py) uses CPython monitoring events to observe the real C callable, argument and return/raise. It does not replace a builtin, patch the fixtures, or instrument CLI child processes.

Neither fixture entered the extension-check helper at lines 123/127. Separate controls on this Linux GIL build showed:

- `_imp.is_frozen('__hello__')` was false under the off override and true under the on override. Each exit reset the override to zero.
- The extension override at line 123 raised `RuntimeError: _imp._override_multi_interp_extensions_check() cannot be used in the main interpreter`. It did not reach line 127 there.
- In a real subinterpreter, the outer line 123 set `-1` and returned `0`; the inner line 123 set `1` and returned `-1`. After an exception in the inner body, line 127 restored `-1`; the outer exit restored `0`.

The [importlib handoff](https://github.com/astral-sh/imre/pull/903#issuecomment-6396740327) identifies the frozen override and import-cache semantics needed for JSON. The [pinned C implementation](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Python/import.c#L4687-L4725) also distinguishes the main-interpreter and free-threaded restrictions.

```sh
PYTHONPATH=/path/to/cpython/Lib /path/to/cpython-3.14.5/python -B \
  benchmarks/cpython_stdlib/full/json/import_helper_trace.py \
  --cpython-source /path/to/cpython --result /tmp/json-import-helper.json
```

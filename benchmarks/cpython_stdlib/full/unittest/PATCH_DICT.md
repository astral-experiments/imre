# Native `unittest.mock.patch.dict`

`from unittest.mock import patch` exposes `patch.dict` for a native `dict[str, str]`.
In `with patch.dict(mapping, updates, clear=True) as active`, `active` is the
original mapping. The patch saves entries when it starts and restores them when
the context exits, including after a return or an exception. A patcher also
supports explicit `start()` and `stop()`; an unstarted patcher's `stop()`
returns `None`. Passing `None` as the updates argument raises `TypeError`.

The [primary report](native-linux.json) attempts all 18 unchanged methods that directly call `patch.dict`. In the [ordinary and reachable O0/O3 run](https://github.com/astral-sh/imre/actions/runs/40226227778), three passed with native fixtures, bodies and assertions: `test_patch_dict_as_context_manager`, `test_patch_dict_start_stop` and `test_patch_dict_stop_without_start`. Nine stopped in native compilation; six stopped while staging their original fixtures. The report retains their individual diagnostics.

The [earlier standalone receipt](patch-dict-default-linux.json.gz) records two native passes, ten compiler blockers and six fixture blockers for the same 18 occurrences on compiler `f446325`.

The original `test_patch_dict_decorator_resolution` now gets past the custom exception layout during the verified import of `test.support`. The [per-mode receipt](patch-dict-rechecked-linux.json) keeps this occurrence blocked: ordinary compilation stops at `_MemoryWatchdog.__init__` assigning `procfile` (line 1155), while reachable compilation reaches a platform conditional in the class body (line 1613). Both diagnostics were independently reproduced at O0 and O3.

`test_patch_dict` and `test_patch_dict_with_clear` start with a dictionary containing both `object()` and a string (upstream lines 513 and 573). With the [verified compiler and runtime](patch-dict-mixed-values-linux.json), both stop compiling this literal before reaching the earlier repeated-local-function diagnostic. Their bodies, assertions, operations and fixtures remain unobserved in both import modes and optimization levels.

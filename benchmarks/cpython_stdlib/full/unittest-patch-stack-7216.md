# Two original unittest.mock patch-stack occurrences on public #7216

CPython 3.14.5 and public Imre `5053404ebfac0d8c532807bd5ee64efae486f209` passed these two methods of the upstream `PatchTest` class. Each is occurrence 1. The frozen two-row selector (SHA-256 `7e14bfa631c7bef61776ca93982bc90db1d4b041f4a7de8140e1edfa365584e7`) records `addSuccess` and zero prior credit on physical lines 2 and 3. The positions below refer to the one-based ordered unittest reference. [Per-occurrence ledger](unittest-patch-stack-7216.tsv)

| Method | Reference position | What the original asserts |
| --- | ---: | --- |
| `test_patch_propagates_exc_on_exit` | 978 | The decorated function raises `RuntimeError`. Its custom patcher's `__exit__` receives that exact exception class, a value and a traceback. |
| `test_stopall_lifo` | 1,014 | After three patchers start, `patch.stopall()` calls their overridden `stop` methods in the order `three`, `two`, `one`. |

## Source and fixture

The entry was the complete 65,244-byte `testpatch.py` from CPython `5607950ef232dad16d75c0cf53101d9649d89115` (Git blob `bd85fdcfc472a615e3cb15c678ddeea499962113`). It retained the module's imports, setup, class and original methods. The maintained fixture supplied `test.test_importlib.util` from that revision's `contextlib` and `sys` imports and its verbatim `uncache` context manager. The 626-byte helper has SHA-256 `9a8ee05f255bf6b45b46d361ceecc275cbc7f41b7d6163f55054429771937ae8`. The native runs declared the upstream `package3` and `package4` packages and their submodules. [Exception test](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_unittest/testmock/testpatch.py#L1747-L1778) · [LIFO test](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_unittest/testmock/testpatch.py#L1947-L1962) · [Upstream helper](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_importlib/util.py#L143-L165)

## Recorded Linux runs

Stock reported `addSuccess` for each fully qualified ID. At each of O0 and O3, each method exited 0 and printed one test and `OK` in ordinary FILE, the saved ELF and its relocated copy with the entry source withdrawn. The separate declared `-m unittest -v` selection printed both fully qualified names with `... ok`, then `Ran 2 tests` and `OK`. Each tier exited 0 with empty stdout and 316 bytes of stderr. The ledger records the stock and FILE PIDs and the two saved executable hashes. [Measured Imre source](https://github.com/astral-experiments/imre/commit/5053404ebfac0d8c532807bd5ee64efae486f209)

With the complete upstream utility restored, native check and build exited 0 at both tiers. At startup, `testpatch.py:15` imported the utility, whose line 18 requests the CPython extension `_testmultiphase`. Both cold FILE runs and the eight single-method saved and withdrawn launches exited 1 with `unittest.SkipTest: No module named '_testmultiphase'` before test selection. The #7216 measurement selected only these two originals. The earlier #7192 fixture inlined the helper alongside the two method bodies and contributes no additional pair. [Upstream extension import](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_importlib/util.py#L18)

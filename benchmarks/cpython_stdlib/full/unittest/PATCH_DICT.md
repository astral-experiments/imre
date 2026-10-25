# Native `unittest.mock.patch.dict`

`from unittest.mock import patch` exposes `patch.dict` for a native `dict[str, str]`.
In `with patch.dict(mapping, updates, clear=True) as active`, `active` is the
original mapping. The patch saves entries when it starts and restores them when
the context exits, including after a return or an exception. A patcher also
supports explicit `start()` and `stop()`; an unstarted patcher's `stop()`
returns `None`. Passing `None` as the updates argument raises `TypeError`.

The [default-fixture receipt](patch-dict-default-linux.json.gz) covers all 18
unchanged methods in the pinned `testmock/testpatch.py` that directly use
`patch.dict`. For ordinary and reachable compilation at both O0 and O3:

| Original fixture and body | Occurrences |
| --- | ---: |
| Passed, body, assertions, patch and fixture all native | 2 |
| Native compiler stopped with the recorded diagnostic | 10 |
| Original fixture or module binding could not be staged | 6 |

The two passes are `PatchTest.test_patch_dict_as_context_manager` and
`PatchTest.test_patch_dict_start_stop`. The primary [1,090-occurrence
report](native-linux.json) runs those same bodies and assertions with a host
fixture. These are two ways to run the same occurrences. The remaining
`test_patch_dict_stop_without_start` reaches its original line 827,
`self.assertFalse(patcher.stop())`: the assertion parameter on this compiler
requires `bool`, while `stop()` may return `None`. It remains a reported
compiler failure. The supplemental receipt records the diagnostic or fixture
binding for every other method, along with the upstream and compiler hashes.

```sh
gzip -dc patch-dict-default-linux.json.gz | jq '.modes["reachable-O3"].records'
```

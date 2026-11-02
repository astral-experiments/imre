# Regex parent ledger on Linux x86-64

The CPython 3.14.5 loader registers 165 parents in `test.test_re`. Every full unittest ID occurs once. The historical ledger matches 136 of those (ID, occurrence) pairs and leaves the 29 in the accompanying TSV. The ledger is 10,521 bytes with SHA-256 `78de83317040441e0071866fd619b4403ce4d49930d824580bedf358a982781c`. Its 136 keys, sorted lexicographically and serialized as UTF-8 `id<TAB>occurrence<LF>` without a header, have SHA-256 `69bff8474ca38a48d6c103de4642418c1e52142c720591130cb8501223b877d4`. [Historical complement in loader order](uncredited-136-linux.tsv)

Ordinals 47, 83, 101 and 111 in the capture replay all belonged to the 136. Ordinal 55, `ReTests.test_bug_527371`, also belonged to the 136. Ordinal 6, `ExternalTests.test_re_tests`, owns an external pattern table with its own row numbering. Ordinal 158, `ReTests.test_symbolic_groups`, is a separate registered parent. Both are outside this selection. [Previous named replay](capture-replay.md)

## Baseline selection

The six unchanged `ReTests` methods below passed on CPython 3.14.5. At Imre `c88c4bf3af090e7b83a7ee72402ebbd7bb9befed` all six errored in ordinary O0 and O3. Each native lane reported the same first exception for each parent.

| Ordinal | Method | Historical member | First native exception |
| ---: | --- | --- | --- |
| 55 | `test_bug_527371` | yes | `AttributeError` reading `Match.lastindex` |
| 66 | `test_bug_gh100061` | no | `PatternError` parsing an atomic group containing a negative lookahead |
| 79 | `test_empty_array` | no | `TypeError` passing an `array.array` to `Pattern.match` |
| 99 | `test_inline_flags` | no | `PatternError: expected a single scoped flag` |
| 112 | `test_match_repr` | no | `PatternError` parsing `(.+)(.*?)\1` |
| 122 | `test_pattern_compare_bytes` | no | `AttributeError` calling `re.purge()` |

`test_inline_flags` reported ten successful subtests on CPython. The native runs stopped before reporting subtests. All three lanes reported zero skips.

Ordinal 147, `ReTests.test_search_anchor_at_beginning`, ran separately with the upstream resource setting and timeout assertion. CPython reported `Ran 1 test in 0.008s`, `OK` (exit 0). Baseline O0 reported `Ran 1 test in 2.027s` and O3 reported `Ran 1 test in 2.938s`, both `FAILED (errors=1)` (exit 1). Both stopped when the stopwatch setup called the missing `time.get_clock_info`. The parent reached neither the ten timed regex operations nor the elapsed-time assertion. There were zero skips and zero subtests.

Three additional parents produced matching upstream skips on all three lanes. `test_locale_caching` (105) and `test_locale_compiled` (106) each reported `test needs en_US.iso88591 locale`. `test_memory_leaks` (113) reported `requires debug build`. CPython reported `Ran 3 tests in 0.000s`, baseline O0 `Ran 3 tests in 2.642s` and baseline O3 `Ran 3 tests in 2.260s`. Each raw footer was `OK (skipped=3)` (exit 0). These three skipped parents add zero paired passes.

## Public paired passes

**137 of 165 after ordinal 112.** `test.test_re.ReTests.test_match_repr`, occurrence 1, reached its representation assertion after variable backreferences landed in `666d9ba9cc363c4b84def416a5d89a5d12128324`. That revision returned `<object>` for the first match. On `b823210dcbf84bab24d2c2005cc367c1e0a0dec4` the unchanged parent passed in both ordinary native modes. It checks the match span and captured text for six subject types and a second alternation pattern. [Variable backreferences](https://github.com/astral-experiments/imre/pull/4074) · [Match representation](https://github.com/astral-experiments/imre/pull/4126)

**138 of 165 after ordinal 79.** `test.test_re.ReTests.test_empty_array`, occurrence 1, passed on public `eb628d6ca8f72c61224be2a45a44b17444539887`. The original checks twelve empty `array.array` typecodes. The bytes pattern `b"bla"` must return `None`, while `b""` must match and return the empty tuple from `groups()`. The native runtime now provides the array storage formats and reads those buffers when matching a bytes pattern. [Native array formats](https://github.com/astral-experiments/imre/pull/4128) · [Bytes-pattern array subjects](https://github.com/astral-experiments/imre/pull/4132)

All singleton runs reported zero skips and zero subtests. The terminal ledger lists each selected ordinal in execution order, the full revision, native optimization level, parent counts, successful subtests, raw unittest count and exit. Both native optimization levels linked the Cargo dev runtime with debug information disabled. The 138-key replay packet has SHA-256 `2d3854ed20b650c4469be39bd6922c903b156a48e2afc85403893dd10a1ca6be`. [Selected terminal records](parent-replay-terminals-linux.tsv)

**Ordinal 55 restored, union 138 of 165.** The unchanged `test.test_re.ReTests.test_bug_527371` passed on public `463b9fe28ccc91606dd5f5c402e7493df08a2d8f` after the matcher started tracking the last group to close on the winning path. The baseline raised `AttributeError` on `Match.lastindex`. The public O0 and O3 runs each reported one pass, zero skips and zero subtests. In separate controls, CPython and both native modes passed eight checks and rejected a deliberately wrong sibling-group winner with `AssertionError` and exit 1. This restores an existing historical key. The public control packet has SHA-256 `62e474f79966198db27bb6f5ad49e8f8a094920b58b734ee43567a72db5a7989`. [Capture closing order](https://github.com/astral-experiments/imre/pull/4151) · [Selected terminal records](parent-replay-terminals-linux.tsv)

## Reproduce the historical join

The upstream source is CPython commit `5607950ef232dad16d75c0cf53101d9649d89115`. The SHA-256 of `Lib/test/test_re.py` is `879c8b562a5bddb413e73ad6d026a6199785bd08fa1c2c5db1ef831b4e1c47e2`. The following command checks the complement against loader order and reconstructs the historical key digest. Run from the Imre root with the staged CPython test tree and interpreter from the capture replay instructions. [Upstream source](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py)

```bash
PYTHONPATH="$stage" "$PYTHON314" -B - <<'PYTHON'
import csv
import hashlib
import unittest
from collections import Counter
from pathlib import Path
from test import test_re

def cases(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from cases(item)
        else:
            yield item.id()

assert hashlib.sha256(Path(test_re.__file__).read_bytes()).hexdigest() == "879c8b562a5bddb413e73ad6d026a6199785bd08fa1c2c5db1ef831b4e1c47e2"
seen = Counter()
parents = []
for ordinal, name in enumerate(cases(unittest.defaultTestLoader.loadTestsFromModule(test_re)), 1):
    seen[name] += 1
    parents.append((ordinal, name, seen[name]))
with open("benchmarks/cpython_stdlib/full/re/uncredited-136-linux.tsv") as src:
    missing = {(int(r["ordinal"]), r["id"], int(r["occurrence"]))
               for r in csv.DictReader(src, delimiter="\t")}
assert len(parents) == 165 and len(missing) == 29 and missing <= set(parents)
keys = sorted((name, occurrence) for ordinal, name, occurrence in set(parents) - missing)
serialized = "".join(f"{name}\t{occurrence}\n" for name, occurrence in keys).encode()
assert len(keys) == 136 and hashlib.sha256(serialized).hexdigest() == "69bff8474ca38a48d6c103de4642418c1e52142c720591130cb8501223b877d4"
PYTHON
```

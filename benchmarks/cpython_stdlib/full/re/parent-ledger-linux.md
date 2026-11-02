# Regex parent ledger on Linux x86-64

The CPython 3.14.5 loader registers 165 parents in `test.test_re`. Every full unittest ID occurs once. The historical ledger matches 136 of those (ID, occurrence) pairs and leaves the 29 in the accompanying TSV. The ledger is 10,521 bytes with SHA-256 `78de83317040441e0071866fd619b4403ce4d49930d824580bedf358a982781c`. Its 136 keys, sorted lexicographically and serialized as UTF-8 `id<TAB>occurrence<LF>` without a header, have SHA-256 `69bff8474ca38a48d6c103de4642418c1e52142c720591130cb8501223b877d4`. [Historical complement in loader order](uncredited-136-linux.tsv)

Ordinals 47, 83, 101 and 111 in the capture replay all belonged to the 136. Ordinal 55, `ReTests.test_bug_527371`, also belonged to the 136. Ordinal 6, `ExternalTests.test_re_tests`, owns an external pattern table with its own row numbering. Ordinal 158, `ReTests.test_symbolic_groups`, is a separate registered parent. The initial 138-key selection included neither parent. [Previous named replay](capture-replay.md)

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

The ordinal 112 and 79 singleton runs reported zero skips and zero subtests. The terminal ledger lists each selected ordinal in execution order, the full revision, native optimization level, parent counts, successful subtests, raw unittest count and exit. The #112 and #79 native builds at both optimization levels linked the Cargo dev runtime with debug information disabled. The 138-key replay packet has SHA-256 `2d3854ed20b650c4469be39bd6922c903b156a48e2afc85403893dd10a1ca6be`. [Selected terminal records](parent-replay-terminals-linux.tsv)

**Ordinal 55 restored, union 138 of 165.** The unchanged `test.test_re.ReTests.test_bug_527371` passed on public `463b9fe28ccc91606dd5f5c402e7493df08a2d8f` after the matcher started tracking the last group to close on the winning path. The baseline raised `AttributeError` on `Match.lastindex`. The public O0 and O3 runs each reported one pass, zero skips and zero subtests. In separate controls, CPython and both native modes passed eight checks and rejected a deliberately wrong sibling-group winner with `AssertionError` and exit 1. This restores an existing historical key. The public control packet has SHA-256 `62e474f79966198db27bb6f5ad49e8f8a094920b58b734ee43567a72db5a7989`. [Capture closing order](https://github.com/astral-experiments/imre/pull/4151) · [Selected terminal records](parent-replay-terminals-linux.tsv)

The separate replay of ordinal 158, `test.test_re.ReTests.test_symbolic_groups`, passed on CPython. The baseline stopped at its first named backreference and conditional. Public `463b9fe28ccc91606dd5f5c402e7493df08a2d8f` compiled all six opening patterns, including the Unicode identifiers, then raised `PatternError` on the match with 200 groups at both optimization levels. Both public runs reported one error, zero skips and zero subtests. This checkpoint adds zero keys for ordinal 158. [Selected terminal records](parent-replay-terminals-linux.tsv)

**139 of 165 after ordinal 122.** `test.test_re.ReTests.test_pattern_compare_bytes`, occurrence 1, passed at both native optimization levels on public `fd73f631466c6d440fd2b84838ffa2d9fc329fa8`. The parent clears the regex cache, compares and hashes separately compiled bytes patterns, then compares text and bytes patterns with `BytesWarning` promoted to an error. The public O0 and O3 runs reported one pass in 1.728s and 2.908s, respectively, with zero skips and zero subtests. The 139-key replay packet has SHA-256 `650fe7403fcce82be592c04cd596acdf25a01d4ea9ccfa2ed30488671b339479`. [Pattern caching and comparison](https://github.com/astral-experiments/imre/pull/4174) · [Selected terminal records](parent-replay-terminals-linux.tsv)

**Ordinal 147 reached its timing assertion.** Each optimized-runtime build linked a Cargo release runtime with debug information disabled and a same-revision Cargo dev compiler. Python code generation ran separately at O0 and O3. Both `c88c4bf3` release-runtime baselines raised the same missing-clock error as the dev-runtime baselines. After `time.get_clock_info` landed at `d21a657056f1ccea4f01d5654bdf541be6e660ac`, the unmodified stopwatch measured 0.321735s at O0 and 0.275684s at O3 for the ten regex operations. Both exceeded the original 0.1s threshold. Each public lane reported one failure, zero skips and zero subtests. [Native clock information](https://github.com/astral-experiments/imre/pull/4202) · [Unittest wall times and raw results](parent-replay-terminals-linux.tsv)

The next timing change, published at `2a0ad34695006d693080ed35d3dadf8836a73427`, recognizes pending finalizers kept alive through instance fields. On the change's pre-merge head `0f221c6b4e06310c8b8496d55ae7fc28557bc718`, the release-runtime stopwatch measured 0.112961s at O0 and 0.126698s at O3. Both candidate runs still failed the original assertion. The practical union below gives ordinal 147 zero credit. [Finalizer lifetime change](https://github.com/astral-experiments/imre/pull/4229)

## Seven practical parents and six regressions

**142 of 165 on public `d7816c3e6e7379903cb09683a96b17c1420124ee`.** The six parents in the baseline table passed together at ordinary O0 and O3, and the independently selected `test_symbolic_groups` passed in both modes. Each mode freshly compiled the selected driver to an object and linked the corresponding public runtime. Three previously uncredited registrations added to the 139-key checkpoint.

| Ordinal | New `ReTests` parent, occurrence 1 | Check reached on public O0 and O3 |
| ---: | --- | --- |
| 66 | `test_bug_gh100061` | Atomic groups and possessive repeats respect the negative lookahead and the winning span. |
| 99 | `test_inline_flags` | Global flags across verbose layout, case and line matching, and all ten original subtests pass. |
| 158 | `test_symbolic_groups` | Named references and conditionals compile for text and bytes. The 200-group pattern matches `xc8yz` at span `(0, 5)`. |

The runtime changes landed separately. The six-parent run reported `Ran 6 tests in 26.860s` at O0 and `Ran 6 tests in 16.082s` at O3. The ordinal 158 runs reported `Ran 1 test in 2.463s` and `Ran 1 test in 1.351s`. All four reported `OK` and had zero skips. [Atomic and possessive matches](https://github.com/astral-experiments/imre/pull/4203) · [Global regex flags](https://github.com/astral-experiments/imre/pull/4204) · [Symbolic conditionals](https://github.com/astral-experiments/imre/pull/4208)

CPython and both public builds also passed six existing regression parents, `test_expand`, `test_keep_buffer`, `test_match_getitem`, `test_re_split`, `test_qualified_re_split` and `test_re_subn`. CPython reported `Ran 6 tests in 0.015s`, O0 `Ran 6 tests in 12.702s` and O3 `Ran 6 tests in 15.148s`, each with `OK`. Across all three selections, each public build recorded 13 distinct passing parents and 18 successful subtests, ten in `test_inline_flags` and eight in `test_re_split`. The 142-key packet has SHA-256 `6c1450df06492369b88ece10b7a6a61f2abf653944d26ad537805485e9f2cf59`. [Raw terminals for all selections](parent-replay-terminals-linux.tsv)

## Later observations

Ordinal 138, `test.test_re.ReTests.test_re_groupref_overflow`, passed on CPython with two successful subtests. Both ordinary builds of `d7816c3e` stopped at the original import on line 703 with `ModuleNotFoundError: No module named 're._constants'; 're' is not a package`. They reached neither the replacement-template error check nor the pattern error check. O0 reported `Ran 1 test in 1.813s` and O3 `Ran 1 test in 2.111s`, both `FAILED (errors=1)`, with zero skips or subtests. A separate diagnostic using the upstream numeric bound passed 14 of 14 checks on CPython and 8 of 14 at native O0. All six discrepancies involved the conditional pattern error message or position. The original parent adds zero keys. [Source](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L702-L708) · [Raw terminal records](parent-replay-terminals-linux.tsv)

On public `2a0ad34695006d693080ed35d3dadf8836a73427`, the release-runtime replay of ordinal 147 reached the unchanged 0.1s assertion. Its stopwatch measured 0.179661s at O0 and 0.125337s at O3. The corresponding unittest durations were 0.785s and 0.630s, with `FAILED (failures=1)` in both modes. On the same public revision, each optimization level passed the preceding seven practical parents and six regression parents with all 18 successful subtests and zero skips. These selected outcomes add zero new case keys to the 142-key historical ledger. [Finalizer lifetime change](https://github.com/astral-experiments/imre/pull/4229) · [Raw terminal records and runtime profiles](parent-replay-terminals-linux.tsv)

An unmerged scalar-finalizer experiment at `4c726ee70c5372527f42e13076990a99a6d3810d` also failed the prespecified ordinal 147 checks. Its O0 stopwatch measured 0.176891s and its O3 stopwatch measured 0.110110s. The comparison then alternated the public and candidate builds in both orders at each level. One of the two additional candidate O3 visits passed. The passing run reported `OK` without a stopwatch value. The other seven additional visits failed. These twelve visits add zero public keys. The counterbalanced run packet has SHA-256 `ce1097a97bcf7888397d3061f3fbf91764c7cb9c00f99e8a20c25e8cb940c2be`.

At public `a1a48176fdd1128a1eb2cf72fd340166670292aa`, which includes the independent immutable-regex-operand change, the same original stopwatch measured 0.129378s at O0, a failure. O3 passed its 0.1s assertion. At candidate `f24a8e8b6206eff85ef858ec545bf9aac428a430`, which borrowed the bounded subject in `findall`, O0 passed and O3 failed at 0.126256s. The two passing visits did not print an exact stopwatch duration. Neither revision passed at both optimization levels. The candidate stayed unmerged after all six paired measurements with many matches on a bytearray ran slower. [Immutable regex operands](https://github.com/astral-experiments/imre/pull/4185) · [Public raw terminal records](parent-replay-terminals-linux.tsv)

A second candidate, `868ce1e2f4db5c07ad13bfd0e8e5f70d461bb0a7`, deferred taking the subject snapshot until the first match. In a fresh comparison, the public `a1a48176` runtime failed at both levels, at 0.104588s and 0.106923s. Its unittest wall times were 0.524s and 0.544s, each with `FAILED (failures=1)`. The candidate passed both ordinary levels, with unittest wall times of 0.513s and 0.460s. Both successful runs omitted the stopwatch duration. The runtime change then landed separately in #4318. [Deferred subject copy](https://github.com/astral-experiments/imre/pull/4318)

A separate eight-parent review preserved the upstream decorators. CPython passed all eight. On public `a1a48176fdd1128a1eb2cf72fd340166670292aa`, ordinary O0 and O3 each reported six skips with `implementation detail specific to cpython` (ordinals 1, 2, 3, 4, 7 and 15) and two errors on missing `re._compiler` (12 and 14). The raw footer was `FAILED (errors=2, skipped=6)` at both levels. Seven proposals concerning SRE disassembly or private implementation helpers remain pending review. Ordinal 2 retains a public requirement for repeated debug output. The proposals change no recorded status or credit and keep all 165 registrations in the denominator. The observed replay packet has SHA-256 `b391129a321e7c56c222ace9575da5b6a5b1ee7fb06e8dbac22fe520d08150f9`. [Seven pending proposals and their named coverage](pending-omissions-linux.tsv) · [Raw terminal records](parent-replay-terminals-linux.tsv)

**143 of 165 after ordinal 123.** The compiled-pattern reducer landed separately in #4259, opened by `@charliermarsh`. It brought `test.test_re.ReTests.test_pickling`, occurrence 1, to a paired pass on public `44fd746fef8978963404738794102d00fb3dee79`. The original uses each pickle protocol from 0 to 5, round-trips the compiled pattern, checks equality and imports `re._compile`. CPython passed in 0.005s. At public `a1a48176fdd1128a1eb2cf72fd340166670292aa`, ordinary O0 and O3 both raised `NotImplementedError: native pickle has no reducer for re.Pattern` at line 1419 on the first `pickle.dumps` call. Their raw counts were `Ran 1 test in 5.251s` and `Ran 1 test in 5.250s`. After #4259 the same native selection passed in 16.418s and 8.461s, with zero skips and zero subtests. In separate cross-interpreter checks for text and bytes patterns, all 48 native/stock combinations decoded each other’s streams and rejected the deliberately wrong flags. The five-lane parent receipt has SHA-256 `bb45aa6593a313ba447204b7f2a73dba46bc7bce87521971ab35bf99f60776d8`. [Compiled-pattern reducer](https://github.com/astral-experiments/imre/pull/4259) · [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L1415-L1423) · [Raw terminal records](parent-replay-terminals-linux.tsv)

Ordinal 125, `test_possible_set_operations`, passed on CPython and failed at both ordinary optimization levels on `a1a48176`. The first ambiguous-set pattern at line 1282 failed to emit the expected `FutureWarning`. Later warning filename and match-result assertions were never reached. Each lane ran one parent and zero subtests. A later independent change at `1c07fbb39cc54f4145730e635ad8a2d488311ead` fixed leading doubled punctuation. Its 256 warning-free text and bytes checks leave the original warning and caller-location assertions open. [Leading punctuation](https://github.com/astral-experiments/imre/pull/4285) · [Pinned original](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L1280-L1289) · [Raw terminal records](parent-replay-terminals-linux.tsv)

Two further parents passed on CPython. On `a1a48176`, both native levels stopped at line 2064 of ordinal 108, `test_look_behind_overflow`, with `PatternError` while building the NFA for the first search. Both levels skipped ordinal 142, `test_regression_gh94675`, through the original decorator with `test requires multiprocessing`. On CPython, its real `Pattern.sub` child started, exited with status 0 and was no longer alive after the join. The nine-run packet for ordinals 108, 125 and 142 has SHA-256 `6a13c918d52a4a947183835be0b91d13fc6be7cfa56c0bf3561c5080a0e4bf48`. [Raw terminal records](parent-replay-terminals-linux.tsv)

**144 of 165 after ordinal 147.** The unchanged `test.test_re.ReTests.test_search_anchor_at_beginning`, occurrence 1, passed once at each ordinary optimization level on public `ae4220413168d248d1d319c5e511d9c76b56ada7` with a Cargo release runtime. Its ten timed operations completed below the original 0.1s threshold in each run. The public unittest counts were `Ran 1 test in 0.437s` at O0 and `Ran 1 test in 0.486s` at O3, both `OK` with exit 0. CPython reported `Ran 1 test in 0.008s`, `OK`. These are unittest wall times. None of the three successful runs printed its stopwatch duration. In the matched `a1a48176` baseline visit, both levels failed the same assertion. Earlier mixed results are recorded separately above. The full 144-key checkpoint has SHA-256 `33dfa5a3f1236a414244a1eaad9145607e4ec7913247363df37f118c45d9de7e`. The dedicated timing receipt contains all 35 visits and has SHA-256 `5b11fcdb2c1ababe000025de7c045142762b19d937ba1919cb8b8c9f96c3dad9`.

On the same public revision, the ordinary dev-runtime builds passed the three existing `findall` parents, `test_findall_atomic_grouping`, `test_findall_possessive_quantifiers` and `test_re_findall`. O0 reported `Ran 3 tests in 16.177s` and O3 reported `Ran 3 tests in 5.944s`, both `OK`. All four public selections reported zero skips and zero subtests. A separate trace on the merged release runtime at O3 observed zero full-subject copies across six 10 MB misses and one snapshot per call across eight hits. The control executable also rejected two deliberately incorrect results. The merged public control packet has SHA-256 `89300a1a5a224c8fe5a09c56eb52db98c0f9d64fb9f4829a8ce24866ba29c3a6`. [Deferred subject copy](https://github.com/astral-experiments/imre/pull/4318) · [Pinned timing assertion](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py#L2475-L2488) · [Raw terminal records](parent-replay-terminals-linux.tsv)

## Reconciliation with the coordinator’s 140 keys

The 140-key coordinator file (SHA-256 `532094fab84f29a89382f844532a902df887df835ebc8c9c973facf5480aaac1`) shares 139 exact (ID, occurrence) pairs with this campaign’s 144-key file. The coordinator alone recorded `test.test_re.ExternalTests.test_re_tests`, occurrence 1. Its 403 table inputs count as one parent. Five `test.test_re.ReTests` methods occur only in the campaign file, all at occurrence 1. The combined ledger has **145 of 165** registrations and SHA-256 `42ec0c15bf295ab19a1e46733c5ec39a509d9f286d9edf405daf589579c098c8`. [Coordinator’s published 140-key count](https://github.com/astral-experiments/imre/blob/fa6e1473700d1a8b3e49b7c28cc381e0295eed44/README.md#L129)

| Ordinal | Original and raw streams | CPython 3.14.5 lines | SHA-256 of those lines |
| ---: | --- | ---: | --- |
| 66 | [`test_bug_gh100061`](delta-140/066-test_bug_gh100061.txt) | 2623–2641 | `d2ff910f34ecb0828dfb9fd8d509c3bafaed027e62432bb9b19e61a148959671` |
| 122 | [`test_pattern_compare_bytes`](delta-140/122-test_pattern_compare_bytes.txt) | 2338–2353 | `888eb4df0696c09da1494ca346ec5d6b972bec3c7ae5755159ea8c863ea8089b` |
| 123 | [`test_pickling`](delta-140/123-test_pickling.txt) | 1415–1423 | `97500bedb672d9d9fbbdd3bc58c2d17b3b8195c158f3c572583355cbef98c8cf` |
| 147 | [`test_search_anchor_at_beginning`](delta-140/147-test_search_anchor_at_beginning.txt) | 2476–2488 | `2c9d5ca6ef33dcba7224b0330440cbbc47c5d369151a4bb60809a5c202ee3b27` |
| 158 | [`test_symbolic_groups`](delta-140/158-test_symbolic_groups.txt) | 282–293 | `c213b9ccac0a0160e61582252547aa079e41a611929cb70b7bc60cd6cba90937` |

The five files contain the complete selected stock, O0 and O3 stdout and stderr streams, with the actual process exits and product hashes. Each named parent recorded a verbose unittest success terminal and the enclosing process exited 0. The selected driver also printed successful subtests. The five-key archive has SHA-256 `1cce072dbd2e0e4e66a91590ca73c7b54ce838519e87e0f177e4de79890a4c9a`. Method hashes cover the complete definition at the pinned revision, from the first decorator or definition line through the final newline. [Pinned source](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py)

## Intact Linux run and five quiet callbacks

At public Imre `f45a52ca1ba9a4050bc2d8973a7a7afd6541a3d8`, the ordinary O0 and O3 executables used the same optimized Rust runtime. Each intact run and the CPython 3.14.5 reference at `5607950ef232dad16d75c0cf53101d9649d89115` started and stopped all 165 registered parents. [Imre revision](https://github.com/astral-experiments/imre/commit/f45a52ca1ba9a4050bc2d8973a7a7afd6541a3d8) · [CPython source](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_re.py)

| Lane | Unittest terminal | Exit | Parents (pass / fail / error / skip) | Subtests (pass / fail / error) |
| --- | --- | ---: | --- | --- |
| CPython | `Ran 165 tests in 1.083s` · `OK (skipped=3)` | 0 | 162 / 0 / 0 / 3 | 2824 / 0 / 0 |
| O0 | `Ran 165 tests in 1878.396s` · `FAILED (failures=72, errors=13, skipped=14)` | 1 | 127 / 13 / 11 / 14 | 2730 / 68 / 3 |
| O3 | `Ran 165 tests in 1673.594s` · `FAILED (failures=72, errors=13, skipped=14)` | 1 | 127 / 13 / 11 / 14 | 2730 / 68 / 3 |

The same 127 parents passed in all three runs. For each, every subtest ID, result and occurrence count matched stock. All 127 belong to the coordinator’s confirmed 145 distinct public keys. The SHA-256 of `ledger-re-145.json` is `4a3fa067f67a6626ae0a480f6ce2382ec31243244dd14d7b6a0bcb01f15dd736`.

The intact `test_search_anchor_at_beginning` assertion measured 0.5160606280041975s at O0 and 0.33790804100135574s at O3 against its strict 0.1s limit. Each executable then ran the same five original IDs once after two consecutive quiet three-second intervals. Stock, O0 and O3 each reported `Ran 5 tests` (0.082s, 4.606s and 3.663s respectively), `OK` and exit 0. The passing assertion prints no stopwatch value. The five files preserve the upstream body hashes and actual `startTest`, `addSuccess` and `stopTest` callback lines and byte offsets for each lane. [66 · atomic groups](quiet-f45a52ca/066-test_bug_gh100061.txt) · [122 · bytes pattern comparison](quiet-f45a52ca/122-test_pattern_compare_bytes.txt) · [123 · pickling](quiet-f45a52ca/123-test_pickling.txt) · [147 · anchored search](quiet-f45a52ca/147-test_search_anchor_at_beginning.txt) · [158 · symbolic groups](quiet-f45a52ca/158-test_symbolic_groups.txt)

| Receipt | SHA-256 |
| --- | --- |
| `imre-re-intact-f45a52ca-linux.tgz` | `543fa9e41100ac8495b10ca00af83059c45ce107f586c21a9505ff49f48e2299` |
| `imre-re-intact-145-join-f45a-release.tgz` | `470d79aa00b6062844cfc298cf6a66cc17a6d27d8efae881bea715268839cbac` |
| `imre-re-five-explicit-f45a-release-intact.tgz` | `382c0396f2cc7056c593c504e0c883f1554a3abaeefe374086499a84fdb426ae` |
| `imre-re-five-explicit-f45a-release-quiet.tgz` | `7e3c920cd7681c806f0ce9368d5f1bd617cc0162e7df282f5e00fb865bbfce10` |

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

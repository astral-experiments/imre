# User codec registry and the intact CTextIOWrapperTest

## Final class replay

**45 of the 82 originals passed cleanly at both native O0 and O3** on public 0c9885cf2953271a30a7a40b7ac2f5a8bf4f76db. Linux CPython 3.14.5 at 5607950ef232dad16d75c0cf53101d9649d89115 passed all 82. Both native runs registered the intact class and reached the 180-second silence bound at module ordinal 510. The other 44 positive cases were independently confirmed in clean subsets. The slow original subsequently passed in a fresh process at both levels, taking 544.780 seconds at O0 and 275.903 seconds at O3.

| Verified outcome | O0 | O3 |
|---|---:|---:|
| Stock-matched success | 45 | 45 |
| Error in the original body | 28 | 28 |
| Assertion failure in the original body | 8 | 8 |
| Original with six subtest errors | 1 | 1 |
| Skip or unattempted original | 0 | 0 |

The registry fix in #4010 and public build repair in #4033 enabled the 82 bodies. The separately verified #4034 preserves a live read-only view for direct bytearray decoder input. The custom decoder provider works, but module ordinal 511 still reaches a body error opening a real file with that registered codec. Its exact reproduction was delivered to the owner of #3956.

- [Full 82-row crosswalk](imre-codec-intact-cwrapper-0c9885cf-extended.tsv). SHA-256 `ad492efc0bb66961ebab855335d2a260a970d41a1dfb5bc001ca4dd032e8fb8f`.
- [Portable evidence bundle](imre-codec-public-0c9885cf.tar.gz), 815,770 bytes. SHA-256 `217a60b947f5c7bab8eee6b35ce24cb16efbbb05297e2529ccf80a385e4e5de7`.
- [Independent compact crosswalk and verification](imre-cwrapper-summary-0c9885cf-extended.tar.gz), 11,838 bytes. SHA-256 `22c1febee783701b930b7c1bc5ec1d0a59c061392b807d631bc4ce7763d0ef40`.

## Checkpoints

The stock reference is Linux CPython 3.14.5 at 5607950ef232dad16d75c0cf53101d9649d89115. The source-pinned native public baseline 36c4f334e96c3bb5a8270ff8f8c85808715fec6d appears in the earlier buffered-rw-pair class packet. It registered all 82, then errored on the missing codecs.register during every setUp.

Development revision 9d54510b5601917481fc2a6c228cdccd2e11eba0 adds the first codec search path on public parent a23a577bcd59a7322af266ff9ec4e8f569f2f929. Its focused archive preserves the 3.14.5 reference and both native optimization levels. It separates the accepted four-tuple path, callback lifetime and cache behavior from the enabled original lookupTestDecoder callback, which still raised AttributeError on the missing CodecInfo constructor at that revision. Supplemental controls do not count as original class cases.

O0 and O3 independently reran the first successful original, test_bufio_write_through (module ordinal 447, class ordinal 3). Its callbacks and runtime streams matched stock in a fresh one-case process at both optimization levels. The first twelve observed O0 successes also passed together in a clean, stock-matched run. The engineering cutoff archive distinguishes an original class no-progress timeout from subsequently cancelled development runs and records containment and cleanup.

On the integrated development head 5e547cf0, both native levels exercised the exact enabled lookupTestDecoder and all fifteen stock selector outcomes. A separate O0 run of the original test_seek_and_tell reaches a genuine later body error at line 3289, when the native file opener receives the user codec name. Standalone codec lookup and the file stream's encoding support are reported separately.

The corrected review head 4b4274a9 includes a cross-API cache test. Calling the public lookup API between two UTF-16-LE decodes no longer lets a later callback replace the winning default provider. Its predecessor 5eade8bd fails both call orders.

The registry merged as 83112e95c9bf505091edecfaa4ce0d73dbee52e0. Its clean compiler build succeeded, but the matching runtime build failed because the MemoryView initializer lacked the concurrently added layout and readonly fields. The failed provider contributed zero native original outcomes. Repair head f0187492f8947ef7818412cb70d440c105b07f22 compiles both products. Eight focused decoder view and lifetime comparisons at native O0 and O3 matched stock, including positive and negative strides, retained views, resize and callback exceptions.

Public #4033 (0c9885cf2953271a30a7a40b7ac2f5a8bf4f76db) restores the runtime build. A decoder that mutates its original bytearray in place sees the new byte under CPython 3.14.5 and sees the old byte at public native O0. The focused callback throws an AssertionError there. The negative-control archive preserves both raw runs.

The intact O0 class on public 0c9885cf registered all 82 originals. Its first completed test, module ordinal 445 (test_append_bom), raised LookupError at the upstream UTF-16 encode on line 3517. Ordinal 446 (test_basic_io) errored on TextIOWrapper.truncate. Ordinal 447 (test_bufio_write_through) was the first original to emit success and stop. The live prefixes record completed callbacks while the process is running. Clean independent qualification is recorded separately.

PR #4034 merged as e64c7815dbdd35d7261edfb9c78bac90df29cbad from independently reviewed eaeb30852ebc6a892972345aeaaef48231c15f8f. The integrated head matched the eight CPython callback controls at native O0 and O3. The original writable reader fixture and hash/lifetime controls passed. These view tests are separate from the 82 original-case results pinned to #4033.

Public O0 completed its first 37 original bodies with 25 success callbacks, 7 errors and 5 failures. All twelve exception traces lead into the original bodies. The live checkpoint predates the outcome of module ordinal 482.

A clean build of merged #4034 at e64c7815 passed all eight view controls at both native optimization levels. The unchanged combined registry fixture passed at ordinary O0. The O3 class on 0c9885cf independently reproduced the first #445 and #446 body errors and the #447 success callback.

Both original public class invocations registered the same 82 IDs and completed 65 in order. Both had 35 success callbacks, 21 errors, eight failures, and one original with unsuccessful subtests. All 30 first exceptions arose in the original test bodies. Both processes reached the 180-second no-callback bound in the 1,000-iteration cyclic-stream original test_rwpair_cleared_before_textio (module ordinal 510), with no adopted children or directory residue. Canonical class timeouts and subsequent disjoint selections are counted separately.

The public registry controls matched all 15 stock selector outcomes and three lifetime/error controls on each native level. The real enabled upstream lookupTestDecoder supplied a working incremental decoder. The O0 disjoint selection of positions 67–82 finished with nine success callbacks and seven errors, including the untouched test_seek_and_tell body trying to open a temporary file with test_decoder. Its key and raw traceback are retained alongside the class receipt.

Public test_bufio_write_through (module 447, global 685, occurrence 1) independently repeated as a single original in fresh O0 and O3 processes. Each matched stock's entire callback stream and normalized unittest stderr with clean process receipts. A separate O3 subset of twelve confirmed originals retains its complete outputs.

Public test_seek_and_tell (module 511, global 749, occurrence 1) was also selected alone. Stock passed. Both native builds completed fixture registration and cleanup, then reported the original body error at test_io.py:3289, called from line 3311, trying to open a real file with encoding='test_decoder'. The registry and incremental decoder controls passed independently. There is no original pass credit for this method.

The canonical runs and the disjoint tail selections account for all 82 originals at both levels on 0c9885cf. Each lane observed 44 successes, 28 errors, eight assertion failures, one method with six unsuccessful subtests, and one bounded method. Both tails included the same thread-start diagnostic. Positive-only repeats determine the independently creditable count.

With its no-progress limit extended to 600 seconds after the method starts, original test_rwpair_cleared_before_textio (module 510, global 748, occurrence 1) passed at O0 in 544.780 seconds of unittest time. Its single-original receipt matches stock except for elapsed time. The intact 180-second timeout remains visible in its own columns. Both levels also passed the same first twelve methods in stock-matched subsets.

Base qualification confirmed all 44 observed successes at both native levels in separate, clean stock-matched runs. Of these, 35 came from the intact prefix and nine from the disjoint 16-ID tail. The 82-row base crosswalk SHA-256 is 031097d2390b7130fd582aa8d21f7e0442c7d13e9072ded49e1c05f13131873d. An independent raw reparse matched every original coordinate, outcome, timeout and confirmed source.

After the base archive and TSV were closed, their outer shell wrapper attempted to execute a PID and exited 127. Its seal log and seven fresh extraction and verification commands are in the base-wrapper-verification archive. All seven returned zero with empty stderr and reproduced the published JSON, TSV, upstream oracle and false-positive controls byte for byte.

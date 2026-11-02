# User codec registry and the intact CTextIOWrapperTest

The stock reference is Linux CPython 3.14.5 at 5607950ef232dad16d75c0cf53101d9649d89115. The source-pinned native public baseline 36c4f334e96c3bb5a8270ff8f8c85808715fec6d appears in the earlier buffered-rw-pair class packet. It registered all 82, then errored on the missing codecs.register during every setUp.

Development revision 9d54510b5601917481fc2a6c228cdccd2e11eba0 adds the first codec search path on public parent a23a577bcd59a7322af266ff9ec4e8f569f2f929. Its focused archive preserves the 3.14.5 reference and both native optimization levels. It separates the accepted four-tuple path, callback lifetime and cache behavior from the enabled original lookupTestDecoder callback, which still raised AttributeError on the missing CodecInfo constructor at that revision. Supplemental controls do not count as original class cases.

O0 and O3 independently reran the first successful original, test_bufio_write_through (module ordinal 447, class ordinal 3). Its callbacks and runtime streams matched stock in a fresh one-case process at both optimization levels. The first twelve observed O0 successes also passed together in a clean, stock-matched run. The engineering cutoff archive distinguishes an original class no-progress timeout from subsequently cancelled development runs and records containment and cleanup.

On the integrated development head 5e547cf0, both native levels exercised the exact enabled lookupTestDecoder and all fifteen stock selector outcomes. A separate O0 run of the original test_seek_and_tell reaches a genuine later body error at line 3289, when the native file opener receives the user codec name. Standalone codec lookup and the file stream's encoding support are reported separately.

The corrected review head 4b4274a9 includes a cross-API cache test. Calling the public lookup API between two UTF-16-LE decodes no longer lets a later callback replace the winning default provider. Its predecessor 5eade8bd fails both call orders.

The registry merged as 83112e95c9bf505091edecfaa4ce0d73dbee52e0. Its clean compiler build succeeded, but the matching runtime build failed because the MemoryView initializer lacked the concurrently added layout and readonly fields. The failed provider contributed zero native original outcomes. Repair head f0187492f8947ef7818412cb70d440c105b07f22 compiles both products. Eight focused decoder view and lifetime comparisons at native O0 and O3 matched stock, including positive and negative strides, retained views, resize and callback exceptions.

Public #4033 (0c9885cf2953271a30a7a40b7ac2f5a8bf4f76db) restores the runtime build. A decoder that mutates its original bytearray in place sees the new byte under CPython 3.14.5 and sees the old byte at public native O0. The focused callback throws an AssertionError there. The negative-control archive preserves both raw runs.

The intact O0 class on public 0c9885cf registered all 82 originals. Its first completed test, module ordinal 445 (test_append_bom), raised LookupError at the upstream UTF-16 encode on line 3517. Ordinal 446 (test_basic_io) errored on TextIOWrapper.truncate. Ordinal 447 (test_bufio_write_through) was the first original to emit success and stop. The live prefixes record completed callbacks while the process is running. Clean independent qualification is recorded separately.

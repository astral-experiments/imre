# BufferedRWPair prerequisite

The existing native buffered pair branch at 507ffceb68a9088f00908af9dc33f2d01a417a92 was integrated with public main eb0bff9f871927ff25d6f40c9969cae70029b759 at 2df90c65d4e5a8bdabe7c8c4207a144fb803b5c1. The two-level class packet records the first unchanged CTextIOWrapperTest replay that passes the original BufferedRWPair lookup at test_io.py line 2406. The next import lookup, io.BufferedRandom at line 2668, stopped registration at both O0 and O3. Stock CPython 3.14.5 passed all 82 originals.

The TSV gives every canonical key, its occurrence and separate observed and qualified outcomes. The real-files packet separately exercises the pair implementation using two independent FileIO objects, negative controls and Linux close-order/error observations.

## Evidence for the open provider PR

At ba89fea7603f7af2ed03783a09bb8a2aae63d4ae, both native levels and pinned stock flushed pending bytes and closed both real descriptors on ordinary destruction and collection of a self-cycle. The lifetime archive also contains the raw writer callback observing whether the reader was already closed. CPython clears an exact acyclic pair's reader first. This candidate let the callback see the reader open. This is a red regression control for the next change.

The real-files archive compares the provider candidate with its exact public parent. It records independent kernel receipts for writer-before-reader explicit close and the reader's EBADF chained to the writer's ENOSPC on a full device. Parent and candidate imports are recorded separately in the intact-class packets.

## Reviewed and merged last-reference fix

PR #3960 supplied both public buffered types. Follow-up PR #3968 used the existing RWPair branch, reviewed and tested at 1902912146f5688c9fcca1bf46fdaaecb6d94b20. It merged on main as 36c4f334e96c3bb5a8270ff8f8c85808715fec6d. The stable patch ID against the respective base is cb96f1a1015f3599c6a197dfd2cacd692ccaa3b8 for both commits.

The exact-head real-file and lifetime archives distinguish explicit close, which closes the writer first, from final release of an exact acyclic pair, which releases the reader first. The real-files archive also covers the reader's EBADF with the writer's ENOSPC in its exception context. The finalizer-review archive includes nested garbage collection, weakref callbacks and resurrection, alongside failures on the earlier provider. All are supplemental controls. The reported original count comes only from the separate CTextIOWrapperTest crosswalk.

## Public class results

On the Linux public merge, the intact CPython 3.14.5 CTextIOWrapperTest registers the same 82 distinct original IDs as stock. These are module ordinals 445 through 526, each at occurrence one. Stock passed all 82. Both public boxed O0 and O3 started and stopped all 82, with 82 setup errors, zero skips, and zero test bodies reached. Every traceback reaches codecs.register(lookupTestDecoder) at upstream test_io.py line 2832, before any method assertion. The following line registers codecs.unregister for cleanup. Source and native provider hashes, the absent-selector check, isolated originals and raw per-ID callbacks are in the intact-cwrapper archive. The originals TSV gives the exact IDs and separates observation from credit.

The aggregate public packet retains the final public and reviewed-head RWPair controls together with the full class crosswalk. The public real-files packet includes a reproducible native limitation on FileIO subclasses that override close. The separate reader-dealloc-warn packet shows that implicit standalone BufferedReader destruction omits the raw warning callback on O0, even though the descriptor closes.

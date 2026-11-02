# BufferedRWPair prerequisite

The existing native buffered pair branch at 507ffceb68a9088f00908af9dc33f2d01a417a92 was integrated with public main eb0bff9f871927ff25d6f40c9969cae70029b759 at 2df90c65d4e5a8bdabe7c8c4207a144fb803b5c1. The two-level class packet records the first unchanged CTextIOWrapperTest replay that passes the original BufferedRWPair lookup at test_io.py line 2406. The next import lookup, io.BufferedRandom at line 2668, stopped registration at both O0 and O3. Stock CPython 3.14.5 passed all 82 originals.

The TSV gives every canonical key, its occurrence and separate observed and qualified outcomes. The real-files packet separately exercises the pair implementation using two independent FileIO objects, negative controls and Linux close-order/error observations.

## Evidence for the open provider PR

At ba89fea7603f7af2ed03783a09bb8a2aae63d4ae, both native levels and pinned stock flushed pending bytes and closed both real descriptors on ordinary destruction and collection of a self-cycle. The lifetime archive also contains the raw writer callback observing whether the reader was already closed. CPython clears an exact acyclic pair's reader first. This candidate let the callback see the reader open. This is a red regression control for the next change.

The real-files archive compares the provider candidate with its exact public parent. It records independent kernel receipts for writer-before-reader explicit close and the reader's EBADF chained to the writer's ENOSPC on a full device. Parent and candidate imports are recorded separately in the intact-class packets.

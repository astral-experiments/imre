# Native unittest fixtures for collections

The pinned CPython discovery found 659 occurrences. At each optimization level, 640 original unittest methods went through the native default-fixture runner. The other 19 are the 16 collections doctests and three reference skips. The raw reports retain all 640 outcomes and diagnostics.

Seven methods passed with the unchanged original body, assertions, collection operation and default unittest lifecycle in a single Imre binary: the four Counter ordering tests, Counter total, deque clear and deque maxlen zero. The receipts include the pinned source blob, method and staged-entry digests, native verbose output and exit status. CPython discovered these methods and verified their sources.

The evidence report combines these new executions with earlier observations at the same product revision and binary digests. The other 652 records are identical to the earlier report, which is preserved separately. The source receipt identifies the exact product tree and parents.

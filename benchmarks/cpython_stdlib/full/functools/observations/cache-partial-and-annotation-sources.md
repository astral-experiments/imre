# Cache, partial, dispatch, and deferred annotation verification

Every source below ran the complete pinned CPython 3.14.5 suite at O0 and O3. Counts agreed between levels. The native test bodies used original CPython fixture hooks. Historical observations describe one exact source at a time.

| Source | Verification | Native bodies with host fixtures | Host-driven native operations | Mixed | Blocked |
| --- | --- | ---: | ---: | ---: | ---: |
| [`5853ebc` primary](5853ebc/evidence-linux.json) | Deferred annotations with canonical sys and record comparisons | 4 | 13 | 15 | 293 |
| [`5853ebc` companion](5853ebc/evidence-free-threading-linux.json) | Deferred annotations with canonical sys and record comparisons | 0 | 0 | 0 | 4 |
| [`211b698` primary](211b698/evidence-linux.json) | Native dispatch registry, #871 | 4 | 13 | 15 | 293 |
| [`211b698` companion](211b698/evidence-free-threading-linux.json) | Native dispatch registry, #871 | 0 | 0 | 0 | 4 |
| [`bb18891` primary](bb18891/evidence-linux.json) | Live builtin length, #949 | 6 | 13 | 13 | 293 |
| [`bb18891` companion](bb18891/evidence-free-threading-linux.json) | Live builtin length, #949 | 0 | 0 | 0 | 4 |
| [`0c43b71` primary](0c43b71/evidence-linux.json) | Fixed argument tuple expansion in partial, #1175 | 4 | 13 | 15 | 293 |
| [`0c43b71` companion](0c43b71/evidence-free-threading-linux.json) | Fixed argument tuple expansion in partial, #1175 | 0 | 0 | 0 | 4 |
| [`9d9b4ed` primary](9d9b4ed/evidence-linux.json) | Identity of an empty partial, #1183 | 4 | 13 | 15 | 293 |
| [`9d9b4ed` companion](9d9b4ed/evidence-free-threading-linux.json) | Identity of an empty partial, #1183 | 0 | 0 | 0 | 4 |
| [`6bb9b6e` primary](6bb9b6e/evidence-linux.json) | Live builtin length with canonical groupby | 6 | 13 | 13 | 293 |
| [`6bb9b6e` companion](6bb9b6e/evidence-free-threading-linux.json) | Live builtin length with canonical groupby | 0 | 0 | 0 | 4 |
| [`78c9efc` primary](78c9efc/evidence-linux.json) | Deferred annotations with ChainMap, #650 | 4 | 13 | 15 | 293 |
| [`78c9efc` companion](78c9efc/evidence-free-threading-linux.json) | Deferred annotations with ChainMap, #650 | 0 | 0 | 0 | 4 |
| [`4a142b7` primary](4a142b7/evidence-linux.json) | Live builtin length with percent formatting and generators | 6 | 13 | 13 | 293 |
| [`4a142b7` companion](4a142b7/evidence-free-threading-linux.json) | Live builtin length with percent formatting and generators | 0 | 0 | 0 | 4 |
| [`e1dff5f` primary](e1dff5f/evidence-linux.json) | Canonical control before CacheInfo | 4 | 13 | 15 | 293 |
| [`e1dff5f` companion](e1dff5f/evidence-free-threading-linux.json) | Canonical control before CacheInfo | 0 | 0 | 0 | 4 |
| [`3b7a9fd` primary](3b7a9fd/evidence-linux.json) | Native CacheInfo, #1201 | 4 | 13 | 15 | 293 |
| [`3b7a9fd` companion](3b7a9fd/evidence-free-threading-linux.json) | Native CacheInfo, #1201 | 0 | 0 | 0 | 4 |
| [`2cd94ee` primary](2cd94ee/evidence-linux.json) | Deferred annotations with canonical generator and unittest changes | 4 | 13 | 15 | 293 |
| [`2cd94ee` companion](2cd94ee/evidence-free-threading-linux.json) | Deferred annotations with canonical generator and unittest changes | 0 | 0 | 0 | 4 |

## Native default-fixture attempt

On [the CacheInfo source](3b7a9fd/default-fixtures-linux.json), the shared runner stopped before native execution for all 325 original fixtures in ordinary and reachable-only modes at both levels. It required direct test classes for 240, rejected duplicate package bindings for 49 (45 `unittest`, four `collections`), required additional instance or class state for 31, rejected four decorated/configured fixtures, and found one original namespace dependency. The [four companion occurrences](3b7a9fd/default-fixtures-free-threading-linux.json) required their original decorated fixture. These receipts add no full native passes; the prior four native bodies retain their host-fixture classification.

The standalone cache control compared all 537 bytes with accelerated CPython. The pure variant agreed on 486 bytes; when a cache wraps a cache, the pure wrapper exposes the inner cache statistics. The original functools tests, annotation methods, ChainMap controls, and whole-package startup remain separate denominators.

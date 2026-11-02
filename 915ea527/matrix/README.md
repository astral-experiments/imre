# Complete CPython test_io on public Imre #3960

The public Imre commit is 915ea527aa75c5787f9d566c0db939f4b459b46f and the CPython 3.14.5 source is pinned at 5607950ef232dad16d75c0cf53101d9649d89115. Each lane compiles the intact module and calls its unittest loader. A completed list receipt compares native stdout byte-for-byte against all 667 CPython original IDs. Each selected receipt covers five cases drawn from that same registered suite. Whole receipts cover the unfiltered execution.

| Lane | Build exit | IDs matched in CPython order | Selected process | Whole process |
|---|---:|---:|---|---|
| ordinary0 | 0 | 667 | Pending | Pending |
| ordinary3 | 0 | Pending | Pending | Pending |
| boxed0 | 0 | 667 | Exit 0 | Pending |
| boxed3 | 0 | 667 | Pending | Pending |

Raw build and process output, arguments and ELF hashes accompany each completed phase. Unfinished phases have no final disposition. The unchanged test, Python I/O implementation, entry point and full canonical registry are in the sibling boxed-O0 receipt.

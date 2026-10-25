# Defaultdict factory on its merged source

The tested two-parent commit has the same Git tree and first parent as [the merged change](https://github.com/astral-sh/imre/pull/1025). [Source and binary hashes](source.json) identify this local build. The [per-occurrence report](evidence-linux.json) includes all 659 cases at O0 and O3.

At each level, 11 complete bodies and assertions ran in Imre under CPython fixtures, 14 used native operations driven or checked by the host, four had no collections operation, three were reference skips, and 627 were blocked. No complete fixture and body ran fully natively. For the unchanged [factory method](factory-host-O0.json), CPython drives the body and assertions and Imre raises the factory error. Two negative controls failed the original assertion.

On the first O0 run, the linker ran out of disk space on the Counter intersection doctest. The [first attempt](doctests-first-O0.json) and [repeat of all 16 doctests](doctests-retry-O0.json) retain that failure and retry. The binary hashes stayed the same. Only that occurrence changed between the full reports.

The [earlier hosted run](../431da8fbc0a8/evidence-linux.json) independently attempted all 659 cases at both levels. The separate four-mode receipts cover invalid factory arguments and the warning hierarchy, nonblocking writes, and plain dictionary copy.

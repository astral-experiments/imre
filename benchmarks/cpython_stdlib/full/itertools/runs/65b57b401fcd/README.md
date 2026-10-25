# Starred tuple merge with main `9492b48`

In [this Linux run](https://github.com/astral-sh/imre/actions/runs/40220344386/job/121250364502), the shared [suite launcher](../../../../SUITE_RUNNER.md) built the unchanged primary suite in ordinary and reachable mode on the prospective merge of [#958](https://github.com/astral-sh/imre/pull/958). The [source receipt](source.json) records the exact tree, compiler, runtime, reference and launcher. The [ordinary](ordinary.json) and [reachable](reachable.json) reports are the original files from the [workflow artifact](https://github.com/astral-sh/imre/actions/runs/40220344386/artifacts/12609263906).

Each mode attempted 115 native compilations at each of O0 and O3. All four results retain the full 137-occurrence denominator: 11 native bodies under host fixtures, 106 blocked, four reference skips and 16 reviewed exclusions. The first diagnostic differs for eight blocked `TestVariousIteratorArgs` methods. No occurrence changes classification.

The separate [free-threaded companion](../../evidence-free-threading-linux.json) contributes one more occurrence. Its original test passed on free-threaded CPython; native compilation stopped on `threading`. The four whole-file builds for each test file executed no original methods.

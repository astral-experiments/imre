# Finite unittest launcher validation

This experiment uses Imre main `4184e6ac609c77f1bf2cfa963321db596a5611cf` in Imre Cloud, with compiler and runtime built together by the offline Cargo command in `run.json`. Stock is CPython 3.14.5; the source archive is pinned to `5607950ef232dad16d75c0cf53101d9649d89115` and SHA-256 `0d838be32b52da92a4293ecd99bc91f7be8b4acc573423665bd1a1428be46e47`. The full unchanged 1,639-file `Lib/test` tree was staged, and all four original source Git blobs were checked against the family manifests. Tool, runner, image, stream and restored-source hashes are in `run.json`.

`runner.py` was generated from four explicit modules. Stock's ordinary `python -B -m unittest -v test.test_fnmatch test.test_tomllib` discovers the identical ordered original ID/outcome sequence, including the docstring-split result in its verbose text. The generated launcher uses literal imports, `TestLoader.loadTestsFromModule`, `TestSuite` and `TestResult` without editing CPython tests. Each tier's JSON and raw stderr retain every ID, occurrence and outcome.

| Frozen family | Originals | Stock | Saved native O0 | Saved native O3 | Source-withdrawn O0/O3 | Strict paired |
| --- | ---: | --- | --- | --- | --- | ---: |
| fnmatch | 24 | 24 pass | 24 observed pass | 24 observed pass | identical | 0 |
| tomllib (three leaf modules) | 17 | 17 pass | 17 observed pass | 17 observed pass | identical | 0 |
| Total | 41 | 41 pass | 41 observed pass | 41 observed pass | 41 identical at each tier | 0 |

The saved images ran with all four original `.py` files and their matching `.pyc` files absent; fixture data and other support files remained available. The source files were restored and rehashed. The finite runpy command, with the package and all leaf modules explicitly declared, still exits 1 at `runpy.py:88` before reporting an original. There were no test timeouts in these two families. Focused regression tests cover original skips/errors, fixture errors, subtest skips, repeated IDs, expected outcomes and a timeout during an identified original.

The architectural negative control **fails separately on main**. `poisoned_runner.py` adds an ordinary literal import of `literal_poison.py`, whose entire body is `status: int = True`. The O0 build exited 0 and linked an image (301 objects). As an entry and in `small_import.py`, that same source exited 1 with `expected int, got bool; annotations are enforced contracts` and no image. Source withdrawal demonstrates real collection from a saved executable, but it does not resolve the imported-body checking discrepancy. All 41 candidates therefore receive zero strict paired credit. These current-main observations are outside the scorecard's fixed Imre pin and do not change its receipts.

Run `PYTHONDONTWRITEBYTECODE=1 python3 benchmarks/compatibility/evidence/literal-runner-main/reproduce.py` on the configured Imre Cloud workspace after staging the exact source fixture and building the matched tools. The script temporarily withdraws only the four staged source copies and restores them in a `finally` block.

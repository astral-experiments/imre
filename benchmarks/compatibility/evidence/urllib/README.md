# urllib pinned replay

The frozen family consists of `test.test_urlparse` (77 occurrences) and `test.test_urllib` (101 occurrences), exactly 178 originals. Neither inventory contains repeated IDs. The other urllib companion suites are outside this frozen denominator. The receipts retain the original module-qualified IDs and occurrence numbers.

`build.json` records the clean pinned Imre source tree, offline Cargo command using the warm release cache and `/workspace/.toolchains`, compiler/runtime and tool hashes, stock Python version/hash, and exact CPython source archive hash. The complete 1,639-file CPython `Lib/test` fixture tree was copied unchanged; both test files and the license were checked against the family manifests. `suite_runner.py` imports the unchanged upstream modules and uses their ordinary unittest loader and lifecycle. Its result class emits one atomic line per original; successful originals containing skipped subtests remain passes. This avoids the canonical replay parser's inability to read unittest's split docstring/subtest lines. The same runner source produced O0 and O3 executables and was run by exact module selector. The architectural control below prevents treating the imported bodies as verified AOT passes. Stock runs have a 120-second limit; each native build and suite run has a 180-second limit.

Each suite's `run.json` records exact native argv, environment, exit status, timeout, stream hashes and the separate finite-module runpy attempts from `SUITE_RUNNER.md`. At this pin, those attempts stop before collection at `runpy.py:88` with an unlinked `exec` body, even with `unittest.__main__` and the suite declared; they receive no test credit. As an independent intact-file control, `urlparse/O0.direct*` and `urlparse/run.json` retain the successful O0 build and verbose execution of the original file.

The five non-ASCII byte skips in `test_urlparse` are subtest skips across three passing originals. `test_urllib` has two Windows-only original skips and two HTTP host-validation errors under the inherited Cloud proxy environment. The proxy configuration was left in place. Error details are retained without irrelevant tracebacks in each result log.

| Suite | Originals | Stock | Native O0 | Native O3 | Paired |
| --- | ---: | --- | --- | --- | ---: |
| `test_urlparse` | 77 | 77 pass | 77 observed pass | 77 observed pass | 0 |
| `test_urllib` | 101 | 97 pass, 2 skip, 2 error | 97 observed pass, 2 skip, 2 error | 97 observed pass, 2 skip, 2 error | 0 |
| Total | 178 | 174 pass, 2 skip, 2 error | 174 observed pass, 2 skip, 2 error | 174 observed pass, 2 skip, 2 error | 0 |

There was no denominator mismatch, timeout, or unreported original. Paired credit is withheld for all 174 candidate passes.

## Architectural controls

`architecture/control.json` records the exact commands, hashes and outcomes. With both staged original test source files and matching bytecode absent, while all 1,637 remaining fixture files stayed in place, the saved O0 and O3 executables reproduced every baseline ID and outcome for both suites. The original source copies were then restored with matching hashes. This establishes independence from those on-disk sources during execution.

The required negative control failed: adding a distinct imported `urllib_poison.py` containing `status: int = True` to the same harness graph yielded O0 build exit 0 and a linked executable (298 objects). The identical source as an entry, and in a small imported graph, each failed with exit 1, no executable, and `expected int, got bool; annotations are enforced contracts`. Source withdrawal alone does not resolve this discrepancy or establish that the imported test bodies went through the required AOT checks. The receipts preserve the observed result lines but mark all candidate pairs with an omission reason.

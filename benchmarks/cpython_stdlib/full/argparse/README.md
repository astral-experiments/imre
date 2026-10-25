# CPython argparse

The denominator is **1,893 distinct unittest IDs** from CPython 3.14.5
(`5607950ef232dad16d75c0cf53101d9649d89115`). The release has one
`test_argparse.py` module. Its metaclasses generate test methods, so counting
source definitions would miss cases. The vendored files include the translation
snapshot used by `TestTranslations.test_translations` and the upstream license.

The test-enabled release interpreter on Linux passed all 1,893 IDs with no skips.
`reference-linux.json` records one result per ID; reference success is not native
coverage. To reproduce with the shared inventory runner:

```sh
python3.14 -B benchmarks/cpython_stdlib/suite_inventory.py \
  benchmarks/cpython_stdlib/full/argparse/UPSTREAM.json \
  --result benchmarks/cpython_stdlib/full/argparse/reference-linux.json
```

Existing native CLI differential fixtures compare exit status, stdout and stderr
with CPython. They are regression checks, not executions of these unittest IDs.

[Native routine results](NATIVE.md) report which original routines drove Imre
parser operations, what ran on the host, and which IDs are still blocked.

[Source-defined methods](METHODS.md) records the unchanged native bodies, host fixtures,
per-ID blockers and the whole-module startup dependency.

## Per-occurrence evidence

[evidence-linux.json](evidence-linux.json) is the version 1 report for the same
1,893 occurrences at each optimization level. The pinned release has no
argparse companion test module. The earlier [generated parse](NATIVE.md) and
[source-method](METHODS.md) runs remain available with their respective compiler revisions.

| Unchanged routines on Linux | -O0 | -O3 |
| --- | ---: | ---: |
| Fully native, including fixtures | 0 | 0 |
| Native body and operation with host fixtures | 4 | 4 |
| Host body and assertions with native operations | 571 | 571 |
| Completed with no parser operation (empty input table) | 12 | 12 |
| Blocked | 1,306 | 1,306 |
| Failed, skipped or excluded | 0 | 0 |
| **Total** | **1,893** | **1,893** |

The compiler and runtime for this report both come from
`80aecec2d89cf4e2e13ed13309bbf1ae2ec1b3e2`. All 417 source-defined
methods and all 1,476 generated methods (1,236 parsing and 240 formatting)
have an observation; no test or fixture was excluded. Each of the four native bodies unconditionally calls
`parse_args` with no explicit assertion; CPython runs the enclosing fixtures.
[Native numeric conversion](https://github.com/astral-sh/imre/pull/647) brings the host-driven count from 463 to 571, with 2,101 native parser calls at each level.
The independent process oracle compares exit status, stdout, and stderr; the
original assertions receive the native result. A host-driven source method
retains the whole-body compiler diagnostic in its evidence. An unsupported
fixture or partial run does not earn a pass.

Run the following with the test-enabled release interpreter, the recorded
compiler, and its matching runtime library. Validate the output against the
unchanged CPython reference before combining it with other suites.

```sh
python3.14 -B benchmarks/cpython_stdlib/full/argparse/evidence.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang clang-18 \
  --imre-revision 80aecec2d89cf4e2e13ed13309bbf1ae2ec1b3e2 --result /tmp/argparse-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
  benchmarks/cpython_stdlib/full/argparse/reference-linux.json /tmp/argparse-evidence.json
```

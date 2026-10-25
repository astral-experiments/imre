# CPython argparse

The denominator is **1,893 distinct unittest IDs** from CPython 3.14.5
(`5607950ef232dad16d75c0cf53101d9649d89115`). The release has one
`test_argparse.py` module. Its metaclasses generate test methods, so counting
source definitions would miss cases. The vendored files include the translation
snapshot used by `TestTranslations.test_translations` and the upstream license.
That test runs CPython's own `Tools/i18n/pygettext.py` on `argparse.py`.
When staging the reference interpreter, include both `Lib` and `Tools/i18n`
from the pinned CPython checkout. [UPSTREAM.json](UPSTREAM.json) records the
original blobs for the tool and the two test helpers that invoke it.

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
| Native body and operation with host fixtures | 5 | 5 |
| Host body and assertions with native operations | 925 | 925 |
| Completed with no parser operation (empty input table) | 12 | 12 |
| Blocked | 951 | 951 |
| Mixed, reference-only, failed, skipped or excluded | 0 | 0 |
| **Total** | **1,893** | **1,893** |

The compiler and runtime come from merged main at
`8c29f1ac2d63f4040bd7cca2ab92d7d03337f08a`. Each report accounts for all
417 source-defined methods and all 1,476 generated methods (1,236 parsing and
240 formatting). Four native bodies each call `parse_args` and have no explicit assertion.
The fifth, `TestBooleanOptionalAction.test_invalid_name`, checks the exception
type and message natively. All five use CPython fixtures.

| Native operations driven by complete CPython routines | Routines per level | Calls per level |
| --- | ---: | ---: |
| [Numeric conversion](https://github.com/astral-sh/imre/pull/647) | 571 | 2,101 |
| [Option prefixes and abbreviations](https://github.com/astral-sh/imre/pull/688) | 739 | 2,965 |
| [Usage and help formatting](https://github.com/astral-sh/imre/pull/719) | 779 | 3,005 |
| [Optional subparsers and aliases](https://github.com/astral-sh/imre/pull/756) | 788 | 3,024 |
| [Known arguments and ordered extras](https://github.com/astral-sh/imre/pull/818) | 789 | 3,025 |
| [Flag constants and counts](https://github.com/astral-sh/imre/pull/873) | 888 | 3,605 |
| [Builtin classes as constants and defaults](https://github.com/astral-sh/imre/pull/895) | 900 | 3,623 |
| [Boolean optional actions](https://github.com/astral-sh/imre/pull/926) | 924 | 3,683 |
| [Argument registration errors](https://github.com/astral-sh/imre/pull/968) | 925 | 3,684 |

The [run on merged #968](https://github.com/astral-sh/imre/actions/runs/40213144597)
adds two previously blocked routines from `TestBooleanOptionalAction`.
The complete original `test_invalid_name` runs under Imre, including its
assertion. In `test_const`, CPython runs the original body and assertion;
Imre runs the failing `add_argument` operation. The
[PR-head run](https://github.com/astral-sh/imre/actions/runs/40196262224) has the
same classifications and call counts. Two blocked `TestExitOnError` methods
have different compiler diagnostics on merged main.

Of the 3,684 host-driven native calls at each level, 3,622 call `parse_args`,
16 call `parse_known_args`, 14 call `format_help`, 11 call `format_usage`,
10 each call `print_help` and `print_usage`, and one calls `add_argument`.
The process comparison checks exit status, stdout and stderr; the original
assertions receive the native result. Each host-driven source method retains
the diagnostic from attempting its whole body. A partial run does not earn
a pass. [Whole-file startup](METHODS.md#whole-file-startup) still stops at
the original `dataclasses.Field` import before it can execute a test.

Run the following with the test-enabled release interpreter, the recorded
compiler, and its matching runtime library. Validate the output against the
unchanged CPython reference before combining it with other suites.

```sh
python3.14 -B benchmarks/cpython_stdlib/full/argparse/evidence.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang clang-18 \
  --imre-revision 8c29f1ac2d63f4040bd7cca2ab92d7d03337f08a --result /tmp/argparse-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
  benchmarks/cpython_stdlib/full/argparse/reference-linux.json /tmp/argparse-evidence.json
```

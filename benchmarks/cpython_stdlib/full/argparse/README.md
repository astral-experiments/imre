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
| Native body and operation with host fixtures | 4 | 4 |
| Host body and assertions with native operations | 789 | 789 |
| Completed with no parser operation (empty input table) | 12 | 12 |
| Blocked | 1,088 | 1,088 |
| Mixed, reference-only, failed, skipped or excluded | 0 | 0 |
| **Total** | **1,893** | **1,893** |

The compiler and runtime come from merged main at
`71ffc22719423ad434d0389a08e6d80395aece01`. Each report accounts for all
417 source-defined methods and all 1,476 generated methods (1,236 parsing and
240 formatting). The four native bodies each call `parse_args` unconditionally
and have no explicit assertion; CPython runs their fixtures.

| Native operations driven by complete CPython routines | Routines per level | Calls per level |
| --- | ---: | ---: |
| [Numeric conversion](https://github.com/astral-sh/imre/pull/647) | 571 | 2,101 |
| [Option prefixes and abbreviations](https://github.com/astral-sh/imre/pull/688) | 739 | 2,965 |
| [Usage and help formatting](https://github.com/astral-sh/imre/pull/719) | 779 | 3,005 |
| [Optional subparsers and aliases](https://github.com/astral-sh/imre/pull/756) | 788 | 3,024 |
| [Known arguments and ordered extras](https://github.com/astral-sh/imre/pull/818) | 789 | 3,025 |

The [post-merge run](https://github.com/astral-sh/imre/actions/runs/40177794356)
adds one complete original routine, `TestParseKnownArgs.test_optionals`.
Its CPython body checks the namespace and the extras returned by the native
parser. Four other `TestParseKnownArgs` bodies already run natively with host
fixtures. Their existing classification is preserved.

Of the 3,025 host-driven native calls at each level, 2,980 call
`parse_args`, one calls `parse_known_args`, 13 call `format_help`, 11 call
`format_usage`, and 10 each call `print_help` and `print_usage`.
The process comparison checks exit status, stdout, and stderr;
the original assertions receive the native result. Each host-driven source
method retains the diagnostic from attempting its whole body. A partial run
does not earn a pass. [Whole-file startup](METHODS.md#whole-file-startup)
and per-method execution have separate results.

Run the following with the test-enabled release interpreter, the recorded
compiler, and its matching runtime library. Validate the output against the
unchanged CPython reference before combining it with other suites.

```sh
python3.14 -B benchmarks/cpython_stdlib/full/argparse/evidence.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang clang-18 \
  --imre-revision 71ffc22719423ad434d0389a08e6d80395aece01 --result /tmp/argparse-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
  benchmarks/cpython_stdlib/full/argparse/reference-linux.json /tmp/argparse-evidence.json
```

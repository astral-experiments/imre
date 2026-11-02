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

## First 14 tests with the metaclass patch

Public revision `c6b39ff06f9dde32486e78588724099953cb9a00` stopped during
import before unittest registered a test. With the local metaclass patch
`473b3648bc741c5a9008646568783cd92c6e7167`, unittest registered all 1,893
cases and passed the first 14 at O0 and O3. The full O0 registration order
matched CPython. The 14 were two `StdStreamTest` cases and twelve generated
`TestActionExtend` cases. These local runs add zero new originals to the
campaign count.

The second local build, `d0b71885cd262e4bdd80e8c13041ed62935bc0eb`, added
the finalizer change from #3935 to the metaclass patch. Each level used a
matched pair of runs with the same 14 successful callback sequences. Total
process times in seconds include import, discovery and fixture execution.

| Level | Metaclass wall | With #3935 wall | Metaclass CPU | With #3935 CPU |
| --- | ---: | ---: | ---: | ---: |
| O0 | 481.393 | 490.397 | 481.258 | 490.193 |
| O3 | 485.649 | 401.492 | 485.564 | 401.459 |

The [callback ledger](matched-first-14-linux.tsv) contains the emitted
`startTest`, `addSuccess` and `stopTest` events, including their monotonic
timestamps, for pinned CPython and all four native runs. Its SHA-256 is
`cf3c0892b1b80fd14170dbe35e94968bdf874ac6ac180876683562bd4312c3b4`. The separately retained full archive is 1,461,106 bytes with SHA-256
`b8497577bc64f2656fb9d94fddd70cae6633f7bdf50892468c23ad73fa4bdf66`.

## FileType cases 279–281 on public main

CPython 3.14.5 (`5607950ef232dad16d75c0cf53101d9649d89115`) and Imre
(`98e78f93f40801e6f08a79f0783ad44c8b6d3933`) each ran the same three selected
unittest methods on Linux. The methods check the error for passing the
`FileType` class to `add_argument`, the error for an invalid file mode, and
the arguments forwarded to a mocked `open`. Each native run used the intact
CPython test module and `argparse` source with Imre's matching compiler and
runtime. [Upstream methods](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_argparse.py#L2057-L2094)

The ledger records each occurrence's `startTest`, `addSuccess` and `stopTest`
with the Unix timestamp emitted by the result recorder. Each run completed
module cleanup and left its working directory empty. [Callback ledger](filetype-callbacks-linux.tsv)

| Run | Process exit | Raw stdout SHA-256 |
| --- | ---: | --- |
| CPython | 0 | `06674df1d5e742b6e252474ec59d8798c2e0006b9224d5d27d23a6e1da9aa45a` |
| Imre O0 | 0 | `1977df59c4c21a2519738b7fdf6245af7ee8068fbcf4d43cf84c1507e245c1d6` |
| Imre O3 | 0 | `3bad65a725e6ff1181e263565a91defa167e3121569d32820ad6c2932bdfb6b9` |

SHA-256 digests of the inputs and callback ledger

- `Lib/test/test_argparse.py` — `bd3892b42c83f276327c141a7d6b38ed08c07a56c6076e4d3aefeddf90515499`
- `Lib/argparse.py` — `052271a2a1eb1b6b60116acb8f3505c98edd52dba062b0309926e63ac4544743`
- Result recorder — `2185643dc756d7509ce073883f5b948d2a9145970c1c4417c9b3fe789338f275`
- `filetype-callbacks-linux.tsv` — `6b5f3a34fa04ce1209fcb81b7a7301e751f828772c78769690a13a7fc1d4af62`

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

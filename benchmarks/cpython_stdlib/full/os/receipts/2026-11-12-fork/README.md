# ForkTests.test_fork on Linux, 2026-11-12

A named unittest launch of CPython 3.14.5's `test.test_os.ForkTests.test_fork` passed on stock and in both Imre SDKs. Each SDK compiled both of the original helper invocations at the requested native tier. Each helper called `os.fork()` and waited for its own child to exit successfully. The ledger adds occurrence 1 once across the two measured revisions. [Occurrence TSV](occurrence.tsv) · [Process records with complete argv and SHA-256](processes.json)

## Source and invocation

The measured #7081 commit is `4f0c9a277620c5364204ad2544770389f8d31f67`. The earlier cohort used #7040 at `fd31e5b88d8bf60b1eb95b79f1e691f4fb18a8af`. Both trees contain CPython `5607950ef232dad16d75c0cf53101d9649d89115`'s `test_os.py` blob `555c85f4857c2d7e69dda2f390847c7f02123cb5`. The method at lines 5576–5590 hashes to `881b0673612d02929ea4c0cfee62e65be612663d8216f7236e665df8e6021887` including its final newline. The exact `-c` argument hashes to `7b18c8e2102a68696e497dd537d5055c14af4d9c4bbef2de05ec2c89ce8a2169`. [Upstream method](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_os.py#L5576-L5590)

The standard `stage_tests.py` root supplied 654 verified test and support files, with the isolated launcher's fixtures installed in its interpreter prefix. The ordinary packaged standard library and the test's original platform guards, setup, method and teardown ran at UID/EUID 1000. A unittest observer loaded one fully qualified name. Each run recorded one START, PASS and STOP, exit 0, empty standard input from a pipe and empty standard output. Times and stderr bytes for each run are in the process records. The 579-case corpus denominator comes from the frozen roster.

At #7081 the two Linux SDKs came from `cargo build --offline --locked --profile dist --workspace` on clean Git source, using the pinned Rust 1.98.1. The O0 build environment set `IMRE_DEFAULT_NATIVE_OPT_LEVEL=0`. The O3 build used the compiler's default of 3. Each build emitted its packaged stdlib and provider archives. Isolated Python selects the compiled-in optimization default because the CLI skips environment compiler options under `-I`. [SDK configuration](https://github.com/astral-experiments/imre/blob/4f0c9a277620c5364204ad2544770389f8d31f67/README.md#installation) · [Python CLI option handling](https://github.com/astral-experiments/imre/blob/4f0c9a277620c5364204ad2544770389f8d31f67/crates/imre-compiler/src/cli.rs#L52-L96)

For each native outer launch, `IMRE_PYTHON` and argv[0] named that tier's SDK. The ordinary Python CLI used `IMREFLAGS="--opt-level 0"` or `IMREFLAGS="--opt-level 3"` and a fresh cache. The original script helper made the first call with `-X faulthandler -I -c` and the second with `-X faulthandler -c` and `PYTHONMALLOC=malloc_debug`. Each SDK driver exec'd the generated program with argv `["-c"]`. Stock used its CPython interpreter for both paths.

```console
/workspace/os-python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14 /workspace/os-evidence/named_method.py test.test_os.ForkTests.test_fork
IMRE_PYTHON=/workspace/os-strict/7081-sdk/O0/imre IMREFLAGS="--opt-level 0" /workspace/os-strict/7081-sdk/O0/imre --include-module test.test_os -B /workspace/os-evidence/named_method.py test.test_os.ForkTests.test_fork
IMRE_PYTHON=/workspace/os-strict/7081-sdk/O3/imre IMREFLAGS="--opt-level 3" /workspace/os-strict/7081-sdk/O3/imre --include-module test.test_os -B /workspace/os-evidence/named_method.py test.test_os.ForkTests.test_fork
```

## Compiler and ELF measurements

The table gives the helper PID followed by the terminal fork PID. The observer opened the helper's live `/proc/<pid>/exe` and hashed its bytes. The fork copied that image and argv. A second live executable capture from each terminal PID confirmed the same SHA-256. Each terminal trace contained only `exit_group(0)` and its exit marker. Each helper's `wait4` and the outer runner's wait for that helper both reported exit 0. Import-time fixture probes created and removed a symlink and a temporary file. The test working directories ended empty. Compiler output and cache artifacts have separate recorded paths.

| Product | Lane | Isolated helper → child | malloc_debug helper → child | Observed code generation for both |
| --- | --- | --- | --- | --- |
| #7040 | stock | 22946 → 22970 | 22984 → 22985 | CPython 3.14.5 |
| #7040 | O0 | 26307 → 26440 | 26441 → 26510 | Clang `-O0`, `O0/providers.a` |
| #7040 | O3 | 23820 → 24369 | 24370 → 24543 | Clang `-O3`, `O3/providers.a` |
| #7081 | stock | 24143 → 24146 | 24152 → 24191 | CPython 3.14.5 |
| #7081 | O0 | 28136 → 28245 | 28246 → 28347 | Clang `-O0`, `O0/providers.a` |
| #7081 | O3 | 29006 → 29040 | 29041 → 29052 | Clang `-O3`, `O3/providers.a` |

For #7081, these are SHA-256 digests of the actual SDK compiler and the two generated helper ELFs. Both the compiler driver and LLD link argv, the linked archive hashes, the observed temporary executable path and the original child argument are in the process records.

| Lane | SDK compiler | Isolated helper ELF | malloc_debug helper ELF |
| --- | --- | --- | --- |
| O0 | `fad1fa56483cb41f316542044931868499686acc9b6bcbf7b910f6ec2e136ef4` | `1b16e3d58bdf5bd83e8991a0134e2ce38b77e93f8516cfd42faf730521155530` | `e120109e4b2bc01190ff5998690d6536306903d912fca58f8f5b9337d8b8efcb` |
| O3 | `0946375ae50cdfb8f85c4b53a9687530003c0613c43fa412cba70a64734e4863` | `a643a1c13870ba364d957d69f3a676d89b80e203737d4d57ed8e1b137ea7b057` | `ab8d359516d25bb2c0beb4605f16b13793befc9493856c6b59fad61900b6c2db` |

## Credit and previous O0 observation

The frozen key is OS-family ordinal 119, occurrence 1. Its original independent stock disposition was `addSuccess`, and its bit in the union of the previous 469 credited keys was zero. The adjacent `test_fork_at_finalization` at ordinal 120 already had credit. The named replays used UID 1000 with fork allowed. The frozen projection supplies family order and disposition and omits the historical runner UID and campaign-wide callback ordinal.

The separate Linux inventory passed 488 of 579 originals. The historical frozen run recorded 486 stock passes of 579. Adding ordinal 119 takes OS coverage from 469/579 to 470/579 and the intersection with those frozen stock passes from 467/486 to 468/486. The full-corpus and frozen-stock denominators on #7081 stay at 16,225 and 15,183. The single row raises the corresponding totals to 13,154 and 13,056. [OS reference runs](../../README.md)

The previous #7040 outer-O0 trace used the default-O3 SDK. Its isolated helper (19620 → 19702) compiled at O3. Its `malloc_debug` helper (19703 → 19745) compiled at O0. That run documents isolation behavior and contributes zero strict O0 credit.

## Retained evidence

The original frozen roster has SHA-256 `25a3cd4f76f2e990e535f6658cbff4781124b00f4962f66bd86c13e79a5b6bdb`. The published 469-key input has SHA-256 `a9b0edf599bdcf3620329bbc05fc3add3bdd84c854aaf3f03e65dfbdd180e446`. The replay archive includes both inputs, the membership check, the named unittest runner, SDK manifests, launch environments, raw streams and per-PID traces. After extracting it, run `python3 os-strict/archive-tools/verify.py .` from the extraction root.

| Artifact | Contents | SHA-256 |
| --- | --- | --- |
| `fork-strict-public-7040-7081-evidence.tar.gz` | Six runs, all 12 terminal children, frozen inputs and build records | `455624e7d0172c10066a8a5be02405dce155dd5f353cd91a2228161acd10e2db` |
| `fork-7081-helper-elfs.tar.gz` | Four live native helper ELF files at #7081 with the PID, path and GNU build ID for each | `2c988c28a465f1d904e91aa4aedd45d56968f3b18e218a35146ff86d6fa6642f` |

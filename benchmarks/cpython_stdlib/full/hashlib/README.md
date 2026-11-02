# hashlib

Imre computes MD5, SHA-1, SHA-2 (224, 256, 384 and 512) and SHA-3 (224, 256, 384 and 512) with exact native bytes. These constructors support incremental updates, independent copies, repeated digest reads, provider metadata and the `usedforsecurity` keyword. The named constructor recognizes the supported aliases and reports CPython's error for unknown names. Buffer-protocol inputs such as `bytearray` and `memoryview` remain outside the native surface.

The native SHAKE-128 and SHAKE-256 providers support variable-length output through both digest methods.

The [reference inventory](reference-linux.json) covers every default Linux test in the [pinned CPython 3.14.5 suite](UPSTREAM.json). Its 82 registered methods ran with 69 passes and 13 upstream skips. The [execution ledger](evidence-linux.json) separates the original fixtures and operations.

| Original Linux methods | Execution |
| ---: | --- |
| 2 | Method body, native assertion adapter and hash operations compiled. The runner supplies part of the fixture. |
| 9 | CPython ran the original test and its assertions. Native SHA-1 or SHA-256 handled the public constructor, digest or incremental operations. The original alternate constructors ran on CPython. |
| 58 | Reference pass with no recorded native execution. |
| 13 | Original default-resource skip. The separately enabled SHA-256 four-GiB update passed natively. |

The [four-mode results](hashlib-linux.json) include the unchanged Werkzeug `generate_etag` function, compiled from its pinned wheel and compared to the original module on seven inputs. The [native runner](../../native_hashlib.py) executes the four original SHA-1 vectors, four SHA-256 vectors and the original `test_large_update` check. That runner supplies part of the class setup.

To reproduce the ledger, run the following with test-enabled CPython 3.14.5 from the repository root. Supply the compiler, runtime and seven pinned Flask wheels in the paths below.

```sh
python3.14 -B benchmarks/cpython_stdlib/suite_inventory.py \
  benchmarks/cpython_stdlib/full/hashlib/UPSTREAM.json --result /tmp/hash-reference.json
mkdir -p /tmp/hash-results
for level in 0 3; do
  for mode in ordinary reachable; do
    flags=()
    if [ "$mode" = reachable ]; then flags+=(--reachable-only); fi
    python3.14 -B benchmarks/cpython_stdlib/native_hashlib.py \
      --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /usr/bin/clang \
      --cpython-source /path/to/cpython --wheelhouse /path/to/wheelhouse --opt-level "$level" \
      "${flags[@]}" --result "/tmp/hash-results/$mode-O$level.json"
  done
done
python3.14 -B benchmarks/cpython_stdlib/full/hashlib/evidence.py /tmp/hash-results
```

## SHAKE vector tests

`run_shake_vectors.py` runs CPython’s original `test_case_shake128_vector` and `test_case_shake256_vector` with their normal unittest assertions and constructor setup. The two text files each contain 256 messages with 512-byte expected digests. Use the `www/hashlib` directory from the [pinned upstream vector repository](https://github.com/python/pythontestdotnet/tree/a226968296ab3054903aa4d0b453714310ebf3e7/www/hashlib).

```sh
python3 benchmarks/cpython_stdlib/full/hashlib/run_shake_vectors.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --vectors /path/to/pythontestdotnet/www/hashlib --opt-level 0
```

Repeat with `--opt-level 3`. Add `--runtime-imports` to exercise runtime-loaded modules. The runner verifies both file hashes, stages them in the pinned test suite’s URL cache and builds one executable. It runs that executable three times. Both tests must pass with the original files. Changing one digit of the last expected digest in either file must fail just that test and report the actual and expected digest. Each control uses a temporary copy.

The [intact Linux ledger](intact-linux.tsv) compares all 82 registered cases at Imre revision `7c5a5717f5e21cdb0513fda3d3466995cece7078` with CPython `5607950ef232dad16d75c0cf53101d9649d89115`. Both run the module directly with `test.support.use_resources` left at `None` and the big-memory limit unset. The empty-cache and cached columns use the same native executable within each lane.

| Vector cache | CPython 3.14.5 | Imre, ordinary and runtime imports at O0 and O3 |
| --- | --- | --- |
| Empty | 72 passed, 10 skipped | 48 passed, 3 errors, 31 skipped |
| Both pinned files | 74 passed, 8 skipped | 50 passed, 1 error, 31 skipped |

With an empty cache, CPython skipped both SHAKE cases after failing to retrieve the files. Imre built the opener, but opening the vector URL raised `NotImplementedError` because the opener has no default HTTP handler. With both files present, only those two verdicts changed. The remaining native error was `test_threaded_hashing`. The ledger includes each skip reason. Six other remote vector tests and two big-memory tests still skipped on CPython with the SHAKE files present.

## BLAKE2 provider results

At Imre `8ffb436432bbfe8ac4d4d1a04971273a4049db22`, the full module ran with only the two SHAKE files cached. The comparison uses Imre `3c1e5e2e1f234a017ab56c50fb75bff053312d0c` as its baseline and CPython `5607950ef232dad16d75c0cf53101d9649d89115` as its reference. All 82 IDs, skip reasons and first exceptions appear in the [cached Linux ledger](provider-cached-linux.tsv).

| Cached run | Result |
| --- | --- |
| CPython 3.14.5 | 74 passed, 8 skipped |
| Imre baseline, ordinary and runtime imports at O0 and O3 | 50 passed, 1 error, 31 skipped |
| Imre with the BLAKE2 provider, ordinary and runtime imports at O0 and O3 | 61 passed, 1 error, 20 skipped |

On the reviewed head `457011ce6c9cc1e7d2458112b3d71abb0f1fda90`, the BLAKE2b and BLAKE2s vector tests (rows 5 and 7) raised `NotImplementedError` because the URL loader had no default HTTP handler. The merge parent supplied that handler. On the merged revision both tests skipped with the same unavailable-file reason as CPython.

The remaining error was `test_threaded_hashing` (row 75).

Nine original cases (rows 3, 4, 6 and 11–16) checked fixed digests. Changing one expected hexadecimal digit in each case made all nine fail their own digest comparisons on CPython and in both runtime-import builds of Imre. Rows 9 and 69 passed their metadata and Unicode-input checks.

In a separate run with the upstream keyed BLAKE2b and BLAKE2s vector files, rows 5 and 7 each passed 256 records. Changing one digit in the last expected digest of either file failed just that row on CPython and on runtime-import Imre at O0 and O3.

## SHA3 provider results

The four-profile Linux ledger compares the original 82 cases at Imre `928de797ff631957659474defa71dd6d746590be` with CPython 3.14.5 `5607950ef232dad16d75c0cf53101d9649d89115`. Each row gives the registered ID, the printed verdict or skip reason, and the first exception for errors and failures. The Imre columns cover ordinary and runtime imports at O0 and O3. [Four-profile Linux ledger](hashlib-82-profiles.tsv)

| Runner and vector cache | CPython 3.14.5 | Imre, each of the four modes |
| --- | --- | --- |
| Stock, empty | 69 passed, 13 skipped | 63 passed, 1 error, 18 skipped |
| Direct, empty | 72 passed, 10 skipped | 66 passed, 1 error, 15 skipped |
| Direct, two SHAKE files | 74 passed, 8 skipped | 68 passed, 1 error, 13 skipped |
| Direct, all eight files | 80 passed, 2 skipped | 74 passed, 1 error, 7 skipped |

Stock CPython ran `python3.14 -B -S -P -m test -v test_hashlib` with default resources. The two Imre stock adapters set `test.support.use_resources = {}` before importing the test module. Running both adapters on CPython reproduced all 82 stock verdicts and skip reasons. The direct unittest runners leave `use_resources` at `None`, which enables the CPU and URL-fetch checks. Each direct Imre lane used one ELF in three separate processes with the corresponding cache. All runs used an unset big-memory limit and blocked external HTTP.

The ordinary stock adapter imports its resource setup before the test module. An earlier ordinary adapter placed executable statements before a later import and failed to compile. An earlier boxed O3 adapter reported two errors, including row 55, `test_clinic_signature_errors`, at a starred call. The ledger records the complete replay using the boxed adapter with literal dynamic imports.

For an all-eight run, place the two SHAKE, two BLAKE2 and four SHA3 text files from the upstream revision in the suite’s `Lib/test/data` directory. Each file contains 256 records. To regenerate the TSV from an extracted evidence packet, run `python3 public/export_hashlib_profiles.py . /tmp/hashlib-ledger`. [Pinned vector files](https://github.com/python/pythontestdotnet/tree/a226968296ab3054903aa4d0b453714310ebf3e7/www/hashlib)

Rows 38, 40, 42 and 44 checked the four empty-message SHA3 digests. An altered expected digit made each comparison fail. With all files present, rows 39, 41, 43 and 45 exercised all 256 records for their respective algorithms. Altering each file’s last expected digest in a separate run made its own test fail on the 255-byte message while its paired control passed. These controls ran on CPython and runtime-import Imre at O0 and O3. Rows 10, 58 and 70 check metadata, eligible provider attributes and Unicode rejection.

With only the SHAKE files cached, seven additional cases passed in all four Imre modes compared with the earlier BLAKE2 ledger. All seven already belonged to the historical 68-case union. Under stock defaults, 63 IDs passed on both CPython and all four Imre modes. Supplying all eight files yielded 74 such passes. The four SHA3 vector rows add four distinct IDs to the earlier supplemental total of 70, which already included both BLAKE2 vector rows. [Earlier SHAKE-cache results](provider-cached-linux.tsv)

Row 75, `test_threaded_hashing`, reported `NotImplementedError: this thread target cannot run independently of the boxed Python heap` in the profiles marked `ERROR`.

## Built-in MD5 constructor

The Linux ledger compares CPython 3.14.5 `5607950ef232dad16d75c0cf53101d9649d89115` with Imre at the first parent `a450eefcc06211bb2f9343c713414e54014c683d` and the public squash `d46c2bb06c872c8855cc9c44823ff71dcc039743`. It records the verdict or skip reason and the first exception for each of the 82 original tests. The full Imre suites used ordinary imports at O0 and O3. The reviewed change was `64b8ff2cb50e7fd5d5cde2d45d6e4d9713941f32` on `1bc43499c6805efd1eafad2cbce6c29df1b596ee`. [Constructor ledger](hashlib-constructors-82.tsv)

| Runner | Stock, empty cache | Direct unittest, empty cache |
| --- | --- | --- |
| CPython 3.14.5 | 69 passed, 13 skipped | 72 passed, 10 skipped |
| Imre squash parent, O0 and O3 | 66 passed, 1 error, 15 skipped | 69 passed, 1 error, 12 skipped |
| Imre public squash, O0 and O3 | 67 passed, 1 error, 14 skipped | 70 passed, 1 error, 11 skipped |

Stock CPython ran `-m test` with its default resources. The stock Imre adapter disabled the same resources before loading the tests. Both direct loaders left `test.support.use_resources` unset. With the caches empty, stock skipped all eight vector files at the URL-resource guard and direct runs skipped them after retrieval failed. The three CPU-gated 4-GiB streaming cases (rows 3, 73 and 74) passed in direct runs. Both profiles skipped rows 20 and 21 at their separate 4.0-GiB memory requirement.

Row 59, `test_get_builtin_constructor`, skipped on the parent because it could not import `_md5`. On the squash it rejected a masked module with an empty cache, restored the module, returned the exact `_md5.md5` factory and populated both `MD5` and `md5` cache keys. Changing the expected uppercase key to `MD4` failed the original assertion. Row 59 was the only verdict that changed between these two commits in either profile at O0 and O3. A separate digest control updated a private MD5 copy to hash `abc`, obtaining `900150983cd24fb0d6963f7d28e17f72`. Changing the final digit to `3` failed that digest comparison.

Row 57, `test_disallow_instantiation`, passed on CPython and skipped at its CPython-only guard on Imre. A separate probe called the registered result types and their `__new__` methods directly. At the squash, 36 of 36 calls produced the expected `TypeError` and message. Row 63, `test_hash_disallow_instantiation`, already passed on the parent and continued to pass for `_hashlib.HASH`. Imre did not expose the optional `_hashlib.HASHXOF` on this run.

Row 75, `test_threaded_hashing`, passed on CPython. Imre raised `NotImplementedError: this thread target cannot run independently of the boxed Python heap` at `thread.start()` in the original test (line 1065). Five native threads using a bound `hash.update` target completed and produced the expected SHA-1 digest in a separate probe. The original target is a Python closure that repeatedly reads the shared hash and input bytes.

## Key derivation

The complete, unchanged CPython 3.14.5 `KDFTests` class passed all four tests on Linux with OpenSSL 3.5.6 and FIPS disabled. On public Imre `5472a33`, `test_file_digest` and `test_normalized_name` passed, while `test_pbkdf2_hmac_c` and `test_scrypt` skipped because Imre lacked the extension. At `6c0cab3`, all four passed in ordinary and runtime-import builds at O0 and O3 with system OpenSSL 3.5.7. Rows 81 and 82 add two distinct paired passes.

The intact PBKDF2 test exercised SHA-1, SHA-256 and SHA-512. A native call trace counted 55 successful PBKDF2 derivations, four successful scrypt derivations and two rejected scrypt preflights. Changing one expected hex digit for each KDF made both cases fail their own byte comparisons on CPython and in each native mode. In a separate native control, hiding only the scrypt symbol kept PBKDF2 passing and skipped scrypt. Making the provider unavailable reproduced the two upstream skips.

The empty-cache, ordinary O0 run of all 82 registrations went from 63 passed, 18 skipped and one error on the baseline to 66 passed, 15 skipped and one error on the candidate. All 63 prior passes held. The additional pass outside this class was `test_hash_disallow_instantiation`. Both runs errored in `test_threaded_hashing`.

The keyed ledger records the original unittest IDs, suite ordinals and verdicts, with mutated inputs and optional-provider runs in separate profiles. The terminal ledger records the raw runner summaries and exit codes. [Keyed outcomes](kdf-linux.tsv) · [Terminal counts](kdf-terminals-linux.tsv)

## Ordinary stock replay at `7f886453`

The Linux runner loaded the original 82 registrations from CPython 3.14.5 at `5607950ef232dad16d75c0cf53101d9649d89115` with the ordinary stock resources, an empty vector cache and no big-memory allowance. CPython passed 69 and skipped 13. At `7f8864530b30bd98d14661da8e15d94a5fc9ed50`, ordinary Imre O0 and O3 each passed 67, skipped 14 and errored in `test_threaded_hashing` at `thread.start()`. All 82 outcomes and first exceptions matched the previous public replay at `00b76b9e6fccde1a07d6da9ddf265c5d6a8590b4`. [Ordered stock ledger](stock-7f886453.tsv)

Both implementations skipped rows 3, 73 and 74 at the CPU resource check, rows 5, 7, 39, 41, 43, 45, 50 and 51 at the URL resource check, and rows 20 and 21 at the 4 GiB memory check. Imre also skipped the CPython-specific row 57.

Row 63, `test.test_hashlib.HashLibTestCase.test_hash_disallow_instantiation` (occurrence 1), passed on CPython and in both native runs. The original checked the real `_hashlib.HASH` type on all three. CPython also exposed `HASHXOF` and ran its optional checks. Substituting constructible `dict` for `HASH` made the original assertion fail on CPython and native O0. Joining full IDs and occurrences to the earlier 77 paired rows yields 78 distinct matches across profiles. Row 63 was already a stock pass on the previous public revision.

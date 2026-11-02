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

The [intact Linux ledger](intact-linux.tsv) compares all 82 registered cases at Imre revision `241b9fe70dc50d7d2887915e0e23ab9f2020cfd3` with CPython `5607950ef232dad16d75c0cf53101d9649d89115`. Both run the module directly with `test.support.use_resources` left at `None` and the big-memory limit unset. The empty-cache and cached columns use the same native executable within each lane.

| Vector cache | CPython 3.14.5 | Imre, ordinary and runtime imports at O0 and O3 |
| --- | --- | --- |
| Empty | 72 passed, 10 skipped | 48 passed, 3 errors, 31 skipped |
| Both pinned files | 74 passed, 8 skipped | 50 passed, 1 error, 31 skipped |

With an empty cache, CPython skipped both SHAKE cases after failing to retrieve the files. Imre reached `urllib.request.build_opener` and raised `AttributeError`. With both files present, only those two verdicts changed. The remaining native error was `test_threaded_hashing`. The ledger includes each skip reason. Six other remote vector tests and two big-memory tests still skipped on CPython with the SHAKE files present.

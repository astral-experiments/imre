# hashlib

Imre computes SHA-1 and SHA-256 with exact native bytes. Both constructors support incremental updates, independent copies, repeated digest reads, provider metadata and the `usedforsecurity` keyword. The named constructor recognizes the supported aliases and reports CPython's error for unknown names. Buffer-protocol inputs such as `bytearray` and `memoryview` remain outside the native surface.

The [reference inventory](reference-linux.json) covers every default Linux test in the [pinned CPython 3.14.5 suite](UPSTREAM.json). Its 82 registered methods ran with 69 passes and 13 upstream skips. The [execution ledger](evidence-linux.json) separates the original fixtures and operations.

| Original Linux methods | Execution |
| ---: | --- |
| 2 | Method body, native assertion adapter and hash operations compiled. The runner supplies part of the fixture. |
| 9 | CPython ran the original test and its assertions. Native SHA-1 or SHA-256 handled the public constructor, digest or incremental operations. The original alternate constructors ran on CPython. |
| 58 | Reference pass with no recorded native execution. |
| 13 | Original default-resource skip. The separately enabled SHA-256 four-GiB update passed natively. |

The [four-mode results](hashlib-linux.json) include the unchanged Werkzeug `generate_etag` function, compiled from its pinned wheel and compared to the original module on seven inputs. The [native runner](../../native_hashlib.py) executes the four original SHA-1 vectors, four SHA-256 vectors and the original `test_large_update` check. The full upstream class setup does not run natively.

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

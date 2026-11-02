# Regex capture replay on Linux x86-64

Three unchanged CPython 3.14.5 tests moved from `ERROR` to `ok` at both optimization levels between Imre `91cea5c91e9ec91cbfc6153c827f2943194899a7` and `bbe8d3f08def4a21e9310994fe875830f77ed0ce`. A fourth test, `test_bug_29444`, passed on both revisions. It checks that a match reads the current contents of a bytearray after it shrinks.

| Loader ordinal (of 165) | Registered ID | CPython | Baseline O0 | Baseline O3 | Merged O0 | Merged O3 |
| ---: | --- | --- | --- | --- | --- | --- |
| 47 | `test.test_re.ReTests.test_bug_29444` | ok | ok | ok | ok | ok |
| 83 | `test.test_re.ReTests.test_expand` | ok | ERROR | ERROR | ok | ok |
| 101 | `test.test_re.ReTests.test_keep_buffer` | ok | ERROR | ERROR | ok | ok |
| 111 | `test.test_re.ReTests.test_match_getitem` | ok | ERROR | ERROR | ok | ok |

On the baseline, `test_expand` stopped at the missing `Match.expand` method. `test_keep_buffer` stopped when the regex scanner refused a bytearray export. `test_match_getitem` stopped while evaluating its named-group formatting check because boxed strings lacked the `format_map` method. The merged tests exercise substitution of numbered, named and unmatched groups, a resize that succeeds after the iterator releases its buffer, and named-field lookup through a match. [#4072](https://github.com/astral-experiments/imre/pull/4072) · [#4075](https://github.com/astral-experiments/imre/pull/4075) · [#4078](https://github.com/astral-experiments/imre/pull/4078)

## Selected run terminals

Each run selected the four IDs above in order and reported zero parent skips and zero subtests. Both native revisions used ordinary builds with `--opt-level 0` or `--opt-level 3`. Both merged builds freshly compiled the driver to a native object and linked it to the merged runtime.

| Product | Passed | Errors | Exit | Raw unittest count | Raw unittest result |
| --- | ---: | ---: | ---: | --- | --- |
| CPython 3.14.5 | 4 | 0 | 0 | `Ran 4 tests in 0.015s` | `OK` |
| Baseline O0 | 1 | 3 | 1 | `Ran 4 tests in 9.143s` | `FAILED (errors=3)` |
| Baseline O3 | 1 | 3 | 1 | `Ran 4 tests in 8.459s` | `FAILED (errors=3)` |
| Merged O0 | 4 | 0 | 0 | `Ran 4 tests in 4.207s` | `OK` |
| Merged O3 | 4 | 0 | 0 | `Ran 4 tests in 4.437s` | `OK` |

The earlier intact 165-parent run recorded 162 passes and three skips on CPython. At Imre `ab6da8b93c62a20bb8d62126ee2afa50e9d99754`, each optimization level recorded 97 passes, 13 failures, 41 errors and 14 skips. The public record omits the full list of IDs behind the historical 136 paired passes. Membership of these four IDs remains unconfirmed, and this selected replay contributes zero new historical credit. [Intact-module results](https://github.com/astral-experiments/imre/pull/4035)

## Reproduce the selection

Run from the Imre repository root using Bash, Clang 19 and LLD. `CPYTHON_SRC` is a CPython Git checkout containing the pinned revision. Set `PYTHON314` to CPython 3.14.5, `CLANG` to the Clang executable and `BASE_PRODUCTS` and `MERGED_PRODUCTS` to directories containing the respective Linux `imre` and `libimre_runtime.a` builds from the two revisions above. The selected driver and staged test module have SHA-256 hashes `ea44b6cc39cb6f1a8ac7b97a3410f00cdbe4b9eb114f714e4b08482169475131` and `879c8b562a5bddb413e73ad6d026a6199785bd08fa1c2c5db1ef831b4e1c47e2`. [Selected driver](selected_originals.py)

```bash
set -eu -o pipefail
stage=$(mktemp -d)
pin=5607950ef232dad16d75c0cf53101d9649d89115
git -C "$CPYTHON_SRC" archive "$pin" Lib/test | tar -xf - --strip-components=1 -C "$stage"
cp benchmarks/cpython_stdlib/full/re/selected_originals.py "$stage/"
ids=(test.test_re.ReTests.{test_bug_29444,test_expand,test_keep_buffer,test_match_getitem})

unset IMREFLAGS IMRE_ENCODED_FLAGS IMRE_RUNTIME_IMPORTS IMRE_FORCE_RUNTIME_IMPORTS PYTHONPATH PYTHONHOME
export PYTHONDONTWRITEBYTECODE=1 PYTHONWARNINGS=default
"$PYTHON314" -B "$stage/selected_originals.py" "${ids[@]}"
for products in "$BASE_PRODUCTS" "$MERGED_PRODUCTS"; do
    for level in 0 3; do
        "$products/imre" build "$stage/selected_originals.py" --module-root "$stage" \
            --opt-level "$level" --no-cache --runtime "$products/libimre_runtime.a" \
            --clang "$CLANG" -C link-arg=-fuse-ld=lld -C link-arg=-Wl,--threads=1 \
            -o "$stage/cases"
        rc=0
        "$stage/cases" "${ids[@]}" || rc=$?
        printf 'exit %s\n' "$rc"
    done
done
```

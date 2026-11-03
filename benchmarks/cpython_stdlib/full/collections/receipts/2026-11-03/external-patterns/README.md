# Collections in structural pattern matching

All 14 original CPython methods passed individually on pinned stock and both native optimization levels with collection ABC registration and native list iteration support. Imre revision `63870c091308c11fc6789a3884f67f7c24f73ac0` passed 8 of the same methods on both levels. Each original contributes one occurrence in `test.test_patma`. The separate collections denominator remains 659.

| Selection | Public 63870c0 | Combined source |
| --- | ---: | ---: |
| `TestPatma` | 7/10 | 10/10 |
| `TestInheritance` | 1/4 | 4/4 |

The three numbered sequence cases exercise `[]` with only a length, `[*_]` with no item access, and `[x, *_, y]` capturing indices 0 and 41. The inheritance class checks mapping and sequence precedence in both case orders, including existing and new descendants of a virtually registered class. Its `S3(list, C, Mapping)` also relies on native list iteration to satisfy the abstract method.

The following originals gained a passing result at both levels.

- `TestPatma.test_patma_184`
- `TestPatma.test_patma_185`
- `TestPatma.test_patma_186`
- `TestInheritance.test_late_registration_sequence`
- `TestInheritance.test_multiple_inheritance_mapping`
- `TestInheritance.test_multiple_inheritance_sequence`

The callback file records one signed process exit per original and level, grouped exits, exact callback lines and stream hashes. The native builds used the matched compiler and static archive from each source.

The public source changes are recorded in [PR 4677](https://github.com/astral-experiments/imre/pull/4677) and [PR 4676](https://github.com/astral-experiments/imre/pull/4676).

## Rebuild the native pair

A clean Imre checkout can reconstruct the measured production sources from the two public review heads. The resulting tree differs from the measured integration tree in a fixture lookup. All crate and Cargo inputs are identical. Run the following in that clean checkout, using the recorded Rust toolchain and Clang 18.

```bash
git fetch origin refs/pull/4677/head:refs/remotes/origin/pattern-review \
    refs/pull/4676/head:refs/remotes/origin/list-slot-review
git switch --detach e066f4ef0f5454729899b4b6a07f8ef8a58511b7
git diff --binary bd65d7f6da2e14ffe937ffef899805fb7be865ac \
    f00b38108997002885020e0ba2dad6bd2f54e7f0 | git apply --index
test "$(git write-tree)" = ee63ce4286f62e9582f7f02726bc39606f07f065
cargo +1.98.1 build --locked --release -p imre-compiler -p imre-runtime
```

The resulting products are `target/release/imre` and `target/release/libimre_runtime.a`. For the baseline pair, build the same packages together in a separate clean checkout of `63870c091308c11fc6789a3884f67f7c24f73ac0`.

## Linux commands

Use CPython 3.14.5 revision `5607950ef232dad16d75c0cf53101d9649d89115`. Set CPYTHON to its checkout, PYTHON to the pinned stock interpreter and CLANG to Clang 18. Set IMRE and RUNTIME to products built together from the same Imre revision. Run the stock selection once, and the native section once for each recorded Imre build.

```bash
set -eu
stage=$(mktemp -d)
git -C "$CPYTHON" archive 5607950ef232dad16d75c0cf53101d9649d89115 Lib/test |
    tar -C "$stage" --strip-components=1 -xf -
cp events.py fourteen.py "$stage/"
cd "$stage"
export PYTHONHASHSEED=1 PYTHONDONTWRITEBYTECODE=1
unset PYTHONPATH PYTHONHOME IMREFLAGS IMRE_ENCODED_FLAGS IMRE_RUNTIME_IMPORTS IMRE_FORCE_RUNTIME_IMPORTS

parents() {
    for n in 119 120 133 134 135 184 185 186 239 240; do
        rc=0
        "$@" TestPatma "test_patma_$n" || rc=$?
        printf 'TestPatma.test_patma_%s exit %s\n' "$n" "$rc"
    done
    for method in test_late_registration_mapping test_late_registration_sequence \
        test_multiple_inheritance_mapping test_multiple_inheritance_sequence; do
        rc=0
        "$@" TestInheritance "$method" || rc=$?
        printf 'TestInheritance.%s exit %s\n' "$method" "$rc"
    done
    for group in TestPatma TestInheritance; do
        rc=0
        "$@" "$group" || rc=$?
        printf '%s selection exit %s\n' "$group" "$rc"
    done
}

parents "$PYTHON" -B fourteen.py
for level in 0 3; do
    "$IMRE" build "$stage/fourteen.py" --module-root "$stage" --runtime-imports \
        --no-cache --opt-level "$level" --runtime "$RUNTIME" --clang "$CLANG" \
        -o "$stage/patterns-O$level"
    parents "$stage/patterns-O$level"
done
```

The native runs on the managed worker compiled the emitted LLVM with the specified Clang. Where the complete emitted IR and option were identical to a previously compiled object in the same toolchain, the worker reused the verified object and linked the selected archive. The source inputs and per-build backend mode, IR, object and executable hashes identify each native result.

The raw stdout digest includes the absolute test-module path printed by the managed worker. The unittest stderr includes elapsed time. Compare the recorded callback sequence, occurrence and signed exit when replaying from another directory.

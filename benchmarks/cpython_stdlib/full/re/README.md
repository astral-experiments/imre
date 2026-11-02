# Regex split and replacement counts

The CPython 3.14.5 loader discovered 165 registrations in `test.test_re`. The driver selected three instances of `ReTests` and ran their original bodies with `unittest.TextTestRunner`. The long `test_re_tests` registration appears only in discovery. The input module is pinned by the [CPython manifest](../../UPSTREAM.json).

| Ordinal | Original method | Checks exercised |
| ---: | --- | --- |
| 126 | `test_qualified_re_split` | Bounded splits, retained captures, empty matches, the positional `maxsplit` warning and caller filename, and invalid argument combinations. |
| 140 | `test_re_split` | Text and bytes, their subclasses, bytearrays and memoryviews, Unicode, separators with captures or unparticipating groups, and adjacent empty matches. |
| 141 | `test_re_subn` | The result and replacement count for matching, nonmatching and zero-width patterns, positional and keyword counts, the positional warning and caller filename, and invalid argument combinations. |

The driver imports `test.support` through `importlib` before loading the intact test module. Ordinary builds select the object runtime through this import. Boxed builds explicitly pass `--runtime-imports`. The same three original methods passed at O0 and O3 in both modes on public main before and after the standalone compiler fix. The compiler change lets ordinary scripts reach the existing native split and substitution-count implementations. [Compiler change](https://github.com/astral-experiments/imre/pull/3991)

For the inverse profile, a separate class inherits the upstream helpers and copies the three bodies at their original line numbers. Each copied method changes one expected value and fails at its own assertion. The qualified split expects one extra colon in the unsplit remainder, the other split expects one fewer colon in a retained capture, and the substitution expects five replacements where the engine made four. The keyed table retains the original and companion IDs. The terminal table records both return codes and the verbatim unittest footer for each run. [Keyed outcomes](split-subn-linux.tsv) · [Terminal counts](split-subn-terminals-linux.tsv)

A second ordinary driver omitted only the initial `import re_registry`. Both native executables on the merged build discovered the same 165 IDs and passed the same three originals. The raw unittest summaries reported 18.869 seconds at O0 and 15.689 seconds at O3.

On the same merged build, invoking the three IDs with `imre -m unittest -v` exited 1 before discovery because Imre could not load `unittest.__main__`. Stock CPython passed all three through that command. The native ledger above comes from the selected `TextTestRunner` driver.

## Reproduce on Linux

Use a checkout of CPython `5607950ef232dad16d75c0cf53101d9649d89115`, its 3.14.5 interpreter, a matching Imre compiler and runtime, and clang with lld. Run these commands from the Imre repository root in Bash. Set `CPYTHON_SRC`, `PYTHON314`, `IMRE`, `IMRE_RUNTIME` and `CLANG` to the corresponding paths.

```bash
case_dir=benchmarks/cpython_stdlib/full/re
stage=$(mktemp -d)
cp -R "$CPYTHON_SRC/Lib/test" "$stage/test"
cp "$case_dir/re_case.py" "$case_dir/re_registry.py" "$stage/"
"$PYTHON314" -B "$case_dir/prepare_split_subn.py" "$stage"

export PYTHONDONTWRITEBYTECODE=1 PYTHONWARNINGS=default RAYON_NUM_THREADS=1
"$PYTHON314" -B "$stage/re_case.py" original
"$PYTHON314" -B "$stage/re_case.py" inverse  # expected to exit 1

for mode in ordinary boxed; do
    flags=()
    if [[ $mode == boxed ]]; then flags+=(--runtime-imports); fi
    for level in 0 3; do
        exe="$stage/re-$mode-O$level"
        "$IMRE" build "$stage/re_case.py" --module-root "$stage" \
            --opt-level "$level" --no-cache --runtime "$IMRE_RUNTIME" --clang "$CLANG" \
            -C link-arg=-fuse-ld=lld -C link-arg=-Wl,--threads=1 "${flags[@]}" -o "$exe"
        "$exe" original
        "$exe" inverse  # expected to exit 1
    done
done
```

The original runs end in `Ran 3 tests` and `OK`. The inverse runs end in `FAILED (failures=3)` with assertion call sites at upstream lines 463, 429 and 383 respectively. The original source SHA-256 is `879c8b562a5bddb413e73ad6d026a6199785bd08fa1c2c5db1ef831b4e1c47e2`.

A separate four-case replay covers match expansion, bytearray buffer lifetime, and named-field lookup through a match. [Capture replay](capture-replay.md)

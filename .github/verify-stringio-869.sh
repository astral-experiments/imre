#!/usr/bin/env bash
# Verification branch only. A read-only token is used exclusively for status.
set -euo pipefail
token=$GH_TOKEN
unset GH_TOKEN
out="$RUNNER_TEMP/stringio-869"
mkdir "$out"
cat > "$out/whence.py" <<'PY'
from io import StringIO

stream = StringIO("x")
for whence in [0, 1, 2, 3, -1, (1 << 31) - 1, 1 << 31, -1 - (1 << 31), 1 << 100]:
    try:
        print(whence, stream.seek(0, whence))
    except OverflowError as error:
        print(whence, "OverflowError", str(error))
    except ValueError as error:
        print(whence, "ValueError", str(error))
PY
deadline=$((SECONDS + 4200))
known=
while (( SECONDS < deadline )); do
    git fetch --no-tags origin '+refs/heads/main:refs/remotes/origin/main' '+refs/pull/869/head:refs/remotes/origin/stringio-869'
    base=$(git rev-parse refs/remotes/origin/main)
    head=$(git rev-parse refs/remotes/origin/stringio-869)
    GH_TOKEN="$token" gh api "repos/$GITHUB_REPOSITORY/pulls/869" --jq '{head:.head.sha,merged,state,merge:.merge_commit_sha}' > "$out/pr.json"
    if jq -e .merged "$out/pr.json" >/dev/null; then
        sha=$(jq -r .merge "$out/pr.json")
        git fetch --no-tags origin "$sha"
        tree=$(git rev-parse "$sha^{tree}")
    else
        jq -e '.state=="open"' "$out/pr.json" >/dev/null
        git merge-tree --write-tree "$base" "$head" > "$out/merge-tree.txt" || { cat "$out/merge-tree.txt"; exit 1; }
        tree=$(head -1 "$out/merge-tree.txt")
    fi
    if [[ $tree != "$known" ]]; then
        commit=$(git -c user.name='Imre CI' -c user.email='ci@localhost' commit-tree "$tree" -p "$base" -p "$head" -m 'StringIO prospective source')
        git switch --detach "$commit"
        receipt="$out/$tree"
        mkdir -p "$receipt"
        jq -n --arg head "$head" --arg base "$base" --arg tree "$tree" '{head:$head,base:$base,tree:$tree}' | tee "$receipt/source.json"
        grep -Fx 'use crate::objects::{bytes::native_size, c_int, exceptions::string_repr};' crates/imre-runtime/src/objects/files/text_memory.rs
        cargo build --locked --release -j 2 -p imre-compiler -p imre-runtime --features imre-compiler/verify-ir
        sha256sum target/release/{imre,libimre_runtime.a} > "$receipt/binaries.sha256"
        for label in fixture whence; do
            file=tests/fixtures/files/string_io.py
            if [[ $label == whence ]]; then file="$out/whence.py"; fi
            python -IB "$file" > "$receipt/$label.cpython" 2> "$receipt/$label.cpython.stderr"
            for opt in 0 3; do
                target/release/imre build "$file" --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --opt-level "$opt" --no-cache -o "$RUNNER_TEMP/869-native"
                "$RUNNER_TEMP/869-native" > "$receipt/$label.O$opt" 2> "$receipt/$label.O$opt.stderr"
                diff -u "$receipt/$label.cpython" "$receipt/$label.O$opt"
                diff -u "$receipt/$label.cpython.stderr" "$receipt/$label.O$opt.stderr"
            done
        done
        cp "$receipt/source.json" "$out/last-passed.json"
        known=$tree
        continue
    fi
    if jq -e .merged "$out/pr.json" >/dev/null; then exit 0; fi
    GH_TOKEN="$token" gh api "repos/$GITHUB_REPOSITORY/commits/$head/check-runs?per_page=100" --jq '[.check_runs[]|{name,status,conclusion}]' > "$out/checks.json"
    if jq -e 'length >= 8 and all(.[]; .status=="completed" and .conclusion=="success")' "$out/checks.json" >/dev/null; then
        echo "All required checks passed; verified tree $tree for $head on $base."
        exit 0
    fi
    if jq -e 'any(.[]; .conclusion=="failure" or .conclusion=="timed_out")' "$out/checks.json" >/dev/null; then exit 1; fi
    sleep 30
done
echo "The PR checks or main were still changing at the end of the hosted window."
exit 1

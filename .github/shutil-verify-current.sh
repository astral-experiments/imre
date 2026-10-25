#!/usr/bin/env bash
# Verification branch only. Every run keeps the old and new results and never merges the PR.
set -euo pipefail
checks_token=$GH_TOKEN
unset GH_TOKEN
out="$RUNNER_TEMP/shutil"
mod=benchmarks/cpython_stdlib/full/shutil
opts=(--imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18)
ref=(--cpython-source .cpython-stdlib-reference)
branch=$SHUTIL_BRANCH
deadline=$((SECONDS + 3600))
base=$(jq -r .base "$out/source.json")
head=$(jq -r .head "$out/source.json")
tree=$(jq -r .tree "$out/source.json")

# Make the initial, already tested --no-commit merge switchable on this disposable runner.
if git rev-parse --verify --quiet MERGE_HEAD >/dev/null; then
    git -c user.name='Imre CI' -c user.email='ci@localhost' commit -qm 'Save verified source on runner'
fi
first="$out/checked-$tree"
mkdir "$first"
cp "$out/"*.json "$out/binaries.sha256" "$first"
while (( SECONDS < deadline )); do
    git fetch --no-tags origin "+refs/heads/main:refs/remotes/origin/main" "+refs/heads/$branch:refs/remotes/origin/$branch"
    new_base=$(git rev-parse refs/remotes/origin/main)
    new_head=$(git rev-parse "refs/remotes/origin/$branch")
    GH_TOKEN="$checks_token" gh api "repos/$GITHUB_REPOSITORY/pulls/$SHUTIL_PR" --jq '{headRefOid:.head.sha,state:(if .merged then "MERGED" else (.state|ascii_upcase) end)}' > "$out/pr-state.json"
    GH_TOKEN="$checks_token" gh api "repos/$GITHUB_REPOSITORY/commits/$new_head/check-runs?per_page=100" --jq '[.check_runs[]|{name,status:(.status|ascii_upcase),conclusion:((.conclusion//"")|ascii_upcase)}]' > "$out/head-checks.json"
    jq -s '.[0] + {statusCheckRollup:.[1]}' "$out/pr-state.json" "$out/head-checks.json" > "$out/pr-checks.json"
    if [[ $(jq -r .state "$out/pr-checks.json") != OPEN ]]; then
        jq -e '.state=="MERGED"' "$out/pr-checks.json" >/dev/null
        exit 0
    fi
    if [[ $new_base == "$base" && $new_head == "$head" ]]; then
        # sys-source only runs on a manual dispatch with an explicit source.
        # Require all eight ordinary checks by name, even if one disappears.
        if jq -e --arg head "$head" '
            ["check", "macos-arm64", "windows-compiler", "thinlto",
             "build-release-artifacts / Shell installer",
             "build-release-artifacts / x86_64-unknown-linux-gnu",
             "build-release-artifacts / aarch64-unknown-linux-gnu",
             "build-release-artifacts / aarch64-apple-darwin"] as $required |
            [.statusCheckRollup[] | select(.status == "COMPLETED" and .conclusion == "SUCCESS") | .name] as $passed |
            .headRefOid == $head and (($required - $passed) | length == 0) and
            all(.statusCheckRollup[]; .status == "COMPLETED" and
                (.conclusion == "SUCCESS" or (.name == "sys-source" and .conclusion == "SKIPPED")))
        ' "$out/pr-checks.json" >/dev/null; then
            echo "Verified current main $base, head $head, tree $tree; all eight PR checks passed."
            exit 0
        fi
        if jq -e --arg head "$head" '.headRefOid == $head and any(.statusCheckRollup[]; .status == "COMPLETED" and (.conclusion == "FAILURE" or .conclusion == "TIMED_OUT"))' "$out/pr-checks.json" >/dev/null; then
            echo 'A public PR check failed; inspect pr-checks.json.'
            exit 1
        fi
        sleep 30
        continue
    fi
    echo "Testing prospective source again: $new_base and $new_head"
    git merge-tree --write-tree "$new_base" "$new_head" > "$out/merge-tree.txt" || { cat "$out/merge-tree.txt"; exit 1; }
    new_tree=$(head -1 "$out/merge-tree.txt")
    commit=$(git -c user.name='Imre CI' -c user.email='ci@localhost' commit-tree "$new_tree" -p "$new_base" -p "$new_head" -m 'Prospective CI source')
    git switch --detach "$commit"
    receipt="$out/checked-$new_tree"
    mkdir -p "$receipt"
    jq -n --arg base "$new_base" --arg head "$new_head" --arg tree "$new_tree" '{base:$base,head:$head,tree:$tree}' | tee "$receipt/source.json"
    cargo build --release --locked -j 2 -p imre-compiler -p imre-runtime --features imre-compiler/verify-ir
    sha256sum target/release/{imre,libimre_runtime.a} > "$receipt/binaries.sha256"
    PYTHONPATH="$GITHUB_WORKSPACE/.cpython-stdlib-reference/Lib" python3 -B "$mod/attempt_methods.py" "${opts[@]}" --compiler-tree "$new_tree" --result "$receipt/method-attempts-linux.json"
    python3 -B "$mod/evidence.py" "${opts[@]}" "${ref[@]}" --method-attempts "$receipt/method-attempts-linux.json" --result "$receipt/evidence-linux.json"
    python3 -B benchmarks/cpython_stdlib/suite_evidence.py "$mod/reference-linux.json" "$receipt/evidence-linux.json"
    jq -e 'length == 2 and all(.[]; .denominator == 223 and ([.records[] | select(.id|endswith(".test_environ_path_missing"))] | length==2 and all(.[]; .classification=="mixed" and .evidence.native_calls==2 and .evidence.native_environment=={"PATH":"absent","processes":2})))' "$receipt/evidence-linux.json" >/dev/null
    for helper in check_csv_handlers copy_error_controls move_cross_device; do
        python3 -B "$mod/$helper.py" "${opts[@]}"
    done
    for helper in copyfile native_copy native_rmtree tree_ops move_ops; do
        python3 -B "$mod/$helper.py" "${opts[@]}" "${ref[@]}" --result "$receipt/$helper-linux.json"
    done
    python3 -B tests/paths.py --compiler target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --opt-levels 0 3
    cp "$receipt/"*.json "$receipt/binaries.sha256" "$out/"
    base=$new_base
    head=$new_head
    tree=$new_tree
done
echo 'Main was still changing or a public check was pending at the hosted verification deadline.'
exit 1

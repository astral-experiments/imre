set -euo pipefail
out="$RUNNER_TEMP/current-$1"
mkdir -p "$out"
git fetch --no-tags origin "+refs/heads/main:refs/remotes/origin/main" "+refs/heads/$SHUTIL_BRANCH:refs/remotes/origin/$SHUTIL_BRANCH"
base=$(git rev-parse refs/remotes/origin/main)
head=$(git rev-parse "refs/remotes/origin/$SHUTIL_BRANCH")
test "$head" = 20f4558f8938b5ced1790c8ba0655845d5628b13
tree=$(git merge-tree --write-tree "$base" "$head")
commit=$(git -c user.name='Imre CI' -c user.email='ci@localhost' commit-tree "$tree" -p "$base" -p "$head" -m 'Test prospective source')
git reset --merge
git switch --detach "$commit"
test "$(git write-tree)" = "$tree"
jq -n --arg base "$base" --arg head "$head" --arg tree "$tree" '{base:$base,head:$head,tree:$tree}' > "$out/source.json"
cargo build --release --locked -j 2 -p imre-compiler -p imre-runtime --features imre-compiler/verify-ir
sha256sum target/release/{imre,libimre_runtime.a} > "$out/binaries.sha256"
mod=benchmarks/cpython_stdlib/full/shutil
opts=(--imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18)
for helper in check_csv_handlers copy_error_controls move_cross_device; do
    python3 -B "$mod/$helper.py" "${opts[@]}" | tee "$out/$helper.log"
done
python3 -B "$mod/move_ops.py" "${opts[@]}" --cpython-source .cpython-stdlib-reference --result "$out/move_ops-linux.json"
jq -e '.optimization_levels | keys == ["0","3"]' "$out/move_ops-linux.json" >/dev/null
echo "Tested actual source $base + $head = $tree"

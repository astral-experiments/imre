#!/usr/bin/env bash
# Package already-built release artifacts and smoke-test the extracted distribution.
set -euo pipefail

target=${1:?usage: package-release.sh TARGET BUILD_DIR DIST_DIR}
build_dir=$(realpath "${2:?missing build directory}")
dist_dir=$(realpath -m "${3:?missing distribution directory}")
root=$(cd "$(dirname "$0")/.." && pwd)
case "$target" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
    *) echo "Unsupported release target: $target" >&2; exit 1 ;;
esac
if [[ $(uname -s) != Linux || $(uname -m) != "${target%%-*}" ]]; then
    echo "Run the release smoke test on the native $target host." >&2
    exit 1
fi

name="relar-$target"
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
bundle="$scratch/$name"
mkdir -m 755 "$bundle"
mkdir -p "$dist_dir" "$scratch/smoke"
install -m 755 "$build_dir/relar" "$bundle/relar"
install -m 644 "$build_dir/librelar_runtime.a" "$root"/LICENSE-{APACHE,MIT} "$bundle/"
# Ruff and ty share Ruff's license. ty also embeds typeshed in the compiler.
install -m 644 "$root/examples/conformance_report/upstream/LICENSE" "$bundle/LICENSE-Ruff"
typeshed_manifest=$("${CARGO:-cargo}" metadata --manifest-path "$root/Cargo.toml" --locked --offline \
    --format-version 1 --filter-platform "$target" | jq -er '.packages[] | select(.name == "ty_vendored") | .manifest_path')
install -m 644 "$(dirname "$typeshed_manifest")/vendor/typeshed/LICENSE" "$bundle/LICENSE-typeshed"

tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
    -C "$scratch" -czf "$scratch/$name.tar.gz" "$name"
cd "$scratch/smoke"
tar -xzf "$scratch/$name.tar.gz"
version=$(python3 "$root/scripts/release-version.py")
[[ $("./$name/relar" --version) == "relar $version ("* ]]
cat > hello.py <<'PY'
import sys

print(f"Hello, {sys.argv[1]}!")
PY
printf 'Hello, release!\n' > expected
"./$name/relar" run hello.py -- release > actual
cmp expected actual

mv "$scratch/$name.tar.gz" "$dist_dir/"
cd "$dist_dir"
sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"

#!/usr/bin/env bash
# Build a native cargo-dist archive and smoke-test the extracted distribution.
set -euo pipefail
umask 022

target=${1:?usage: package-release.sh TARGET DIST_DIR}
dist_dir=$(realpath -m "${2:?missing distribution directory}")
root=$(cd "$(dirname "$0")/.." && pwd)
case "$target" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
    *) echo "Unsupported release target: $target" >&2; exit 1 ;;
esac
if [[ $(uname -s) != Linux || $(uname -m) != "${target%%-*}" ]]; then
    echo "Run the release smoke test on the native $target host." >&2
    exit 1
fi

scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
cd "$root"
version=$(python3 scripts/release-version.py)
cargo=${CARGO:-cargo}
"$cargo" build --locked --profile dist --target "$target" --workspace
"$cargo" metadata --locked --format-version 1 --filter-platform "$target" > "$scratch/metadata.json"
target_dir=$(jq -er '.target_directory' "$scratch/metadata.json")
typeshed_manifest=$(jq -er '.packages[] | select(.name == "ty_vendored") | .manifest_path' "$scratch/metadata.json")
# The compiler and runtime are separate Cargo packages. Ship the runtime beside the executable.
assets=target/release-assets
mkdir -p "$assets"
build_dir="$target_dir/$target/dist"
chmod 755 "$build_dir/relar"
install -m 644 "$build_dir/librelar_runtime.a" LICENSE-APACHE LICENSE-MIT "$assets/"
install -m 644 examples/conformance_report/upstream/LICENSE "$assets/LICENSE-Ruff"
install -m 644 "$(dirname "$typeshed_manifest")/vendor/typeshed/LICENSE" "$assets/LICENSE-typeshed"

# cargo-dist's Cargo invocation has no --locked flag. Check the lockfile across its build.
sha256sum Cargo.lock > "$scratch/lock.sha256"
dist build --tag "$version" --artifacts local --target "$target"
sha256sum --check --status "$scratch/lock.sha256"
name="relar-$target"
archive="$root/target/distrib/$name.tar.gz"
cd "$scratch"
tar -xzf "$archive"
[[ $("./$name/relar" --version) == "relar $version ("* ]]
cat > hello.py <<'PY'
import sys

print(f"Hello, {sys.argv[1]}!")
PY
printf 'Hello, release!\n' > expected
"./$name/relar" run hello.py -- release > actual
cmp expected actual

mkdir -p "$dist_dir"
cp "$archive" "$archive.sha256" "$dist_dir/"
(cd "$dist_dir" && sha256sum --check --strict "$name.tar.gz.sha256")

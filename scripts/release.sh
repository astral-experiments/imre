#!/usr/bin/env bash
# Prepare for a release. Pass additional options to rooster.
set -eu

cd "$(dirname "$0")/.."
workspace_version() { cargo metadata --no-deps --format-version 1 | jq -er '.packages[] | select(.name == "imre-compiler").version'; }

# Rooster assumes 0.0.0 before the first tag. Start at the Cargo workspace version.
if [[ -z $(git tag --list '[0-9]*') ]]; then
    current=$(workspace_version)
    if [[ $# == 0 || ( $1 == --version && ${2:-} == "$current" ) ]]; then
        set -- --version "$current" --no-update-version-files
    fi
fi
uv run --locked --python 3.12 --only-group release rooster release "$@"
cargo update --workspace
version=$(workspace_version)
git checkout -b "release/$version"
git add CHANGELOG.md Cargo.toml Cargo.lock
git commit -m "Bump version to $version"

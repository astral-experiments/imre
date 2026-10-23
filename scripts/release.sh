#!/usr/bin/env bash
# Prepare the release branch with Rooster.
set -euo pipefail

cd "$(dirname "$0")/.."
if [[ -n $(git status --porcelain) ]]; then
    echo 'Release preparation requires a clean checkout.' >&2
    exit 1
fi

current=$(python3 scripts/release-version.py)
tool=(uv run --locked --python 3.12 --only-group release)
if [[ -z $(git tag --list '[0-9]*.[0-9]*.[0-9]*') ]]; then
    # Rooster assumes 0.0.0 before the first tag. Start with the Cargo version.
    if (( $# )) && [[ ${1:-} != --version || ${2:-} != "$current" ]]; then
        echo "The first release must use $current." >&2
        exit 1
    fi
    set -- --version "$current" --no-update-version-files
elif [[ ${1:-} == --version ]]; then
    "${tool[@]}" python - "$current" "$2" <<'PY'
import sys
from packaging.version import Version

current, requested = sys.argv[1:]
if Version(requested) <= Version(current):
    sys.exit(f"Release {requested} must be newer than {current}.")
PY
fi

"${tool[@]}" rooster release "$@"
version=$(python3 scripts/release-version.py)
python3 scripts/release-version.py --notes -- "$version" > /dev/null
cargo update -p relar-compiler -p relar-runtime

git switch -c "release/$version"
git add -- CHANGELOG.md Cargo.lock crates/relar-compiler/Cargo.toml crates/relar-runtime/Cargo.toml
git commit -m "Bump version to $version"

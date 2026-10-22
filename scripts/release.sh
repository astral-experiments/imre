#!/usr/bin/env bash
# Run Rooster and prepare a local release branch for review.
set -euo pipefail

cd "$(dirname "$0")/.."
root=$PWD
if [[ $(git branch --show-current) != main || -n $(git status --porcelain) || $(git rev-parse --is-shallow-repository) == true ]]; then
    echo 'Prepare a release from a clean, full checkout of main with its tags.' >&2
    exit 1
fi

current=$(python3 scripts/release-version.py)
requested=
if (( $# )); then
    if [[ $# != 2 || ( $1 != --version && $1 != --bump ) ]]; then
        echo 'Usage: scripts/release.sh [--version VERSION | --bump major|minor|patch|pre]' >&2
        exit 2
    fi
    if [[ $1 == --version ]]; then
        requested=$2
        number='(0|[1-9][0-9]*)'
        if [[ ! $requested =~ ^$number\.$number\.$number(-(alpha|beta|rc)\.$number)?$ ]]; then
            echo "Expected a Cargo version such as 0.2.0 or 0.2.0-rc.1." >&2
            exit 2
        fi
    fi
fi

# With no tag, Rooster expects 0.0.0 in the manifests. Relar's first release
# starts at the version already declared by the compiler and runtime.
tags=$(git tag --list '[0-9]*.[0-9]*.[0-9]*')
if [[ -z $tags ]]; then
    if (( $# )) && [[ $requested != "$current" ]]; then
        echo "The first release must use the current Cargo version, $current." >&2
        exit 2
    fi
    set -- --version "$current" --no-update-version-files
fi

# Keep partial edits out of the caller's checkout if preparation fails.
scratch=$(mktemp -d)
git worktree add --quiet --detach "$scratch/checkout" HEAD
trap 'git -C "$root" worktree remove --force "$scratch/checkout"; rm -rf -- "$scratch"' EXIT
cd "$scratch/checkout"
uv venv --python 3.12 "$scratch/tools"
python="$scratch/tools/bin/python"
uv pip sync --require-hashes --only-binary :all: --python "$python" scripts/requirements-release.txt
if [[ -n $tags && -n $requested ]]; then
    "$python" - "$current" "$requested" <<'PY'
import sys
from packaging.version import Version

current, requested = sys.argv[1:]
if Version(requested) <= Version(current):
    sys.exit(f"Release {requested} must be newer than {current}.")
PY
fi

ROOSTER_NO_CACHE=1 "$scratch/tools/bin/rooster" release "$@"
version=$(python3 scripts/release-version.py)
if [[ -n $requested && $requested != "$version" ]]; then
    echo "Rooster prepared $version, expected $requested." >&2
    exit 1
fi
python3 scripts/release-version.py --notes -- "$version" > /dev/null
cargo update -p relar-compiler -p relar-runtime

git add -- CHANGELOG.md Cargo.lock crates/relar-compiler/Cargo.toml crates/relar-runtime/Cargo.toml
git commit -m "Bump version to $version"
git -C "$root" switch --create "release/$version" "$(git rev-parse HEAD)"

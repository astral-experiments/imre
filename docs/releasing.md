# Publishing Relar

## Prepare the release

Start from a clean, full checkout with the latest `main` and tags. Install uv,
Python 3.11 or newer, the pinned Rust toolchain, and authenticate the GitHub CLI
so Rooster can read the merged pull requests.

```sh
git switch main
git pull --ff-only --tags origin main
./scripts/release.sh
git show HEAD
```

Rooster writes the original PR titles and links to `CHANGELOG.md`, grouping them
by label. Unlabeled PRs appear under **Other changes**. The first release uses
the existing Cargo version, `0.1.0`. Later releases default to a patch bump,
or a minor bump when a PR has the `breaking` label. You can choose a version
explicitly with `./scripts/release.sh --version 0.2.0-rc.1`.

The script commits the changelog and any version changes in both Cargo manifests
and `Cargo.lock` on a local `release/<version>` branch. Review the diff, push
the branch, open a pull request, and merge it into `main`.

```sh
git push origin HEAD
gh pr create --base main --fill
```

The `release` dependency group pins Rooster. When that pin changes, regenerate
the hash-checked tool dependencies with
`uv pip compile --group release --python-version 3.12 --universal --generate-hashes --no-header --no-annotate -o scripts/requirements-release.txt`.

## Build and publish the binaries

To rehearse the release, dispatch the Release workflow with its default input.
Cargo-dist builds GNU/Linux archives on native x86-64 and ARM64 Ubuntu 24.04
runners. Each runner tests the runtime, extracts its archive, and compiles and
runs a script. The archives and their SHA-256 sidecars are available from the
workflow run.

```sh
gh workflow run release.yml --repo astral-sh/relar --ref main -f tag=dry-run
```

To publish, dispatch the workflow from `main` with the reviewed Cargo version.

```sh
gh workflow run release.yml --repo astral-sh/relar --ref main -f tag=0.1.0
```

The workflow verifies both archives and publishes them and their checksums to
GitHub Releases. The release body comes from the matching `## <version>`
section of the merged changelog. The bare version tag points to the tested
commit. An existing tag must point to that same commit, and an existing release
or draft stops publication. Versions such as `0.2.0-rc.1` are marked as prereleases.

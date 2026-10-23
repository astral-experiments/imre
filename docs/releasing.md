# Publishing Relar

## Prepare the release

Run **Prepare release** from `main` in GitHub Actions, or use the CLI.

```sh
gh workflow run release-prepare.yml --repo astral-sh/relar --ref main
```

The workflow runs Rooster and opens a `release/<version>` pull request against
`main`. Rooster groups the original PR titles and links by label in
`CHANGELOG.md`. Version bumps update both Cargo manifests and `Cargo.lock`.
Unlabeled PRs appear under **Other changes**. The first release uses the
existing Cargo version, `0.1.0`. Later releases default to a patch bump, or a
minor bump when a PR has the `breaking` label. To choose a version, fill in the
workflow’s optional `version` input or add `-f version=0.2.0-rc.1` to the command.

Open the PR linked in the workflow summary. Select **Approve workflows to run**
to start CI on the bot-created PR, review the changelog, and merge it. Running
**Prepare release** again for the same version regenerates the branch from
`main` and updates the open PR. Make any manual changelog edits after the last run.

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

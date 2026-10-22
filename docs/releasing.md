# Publishing Relar

The Release workflow builds a compiler and matching runtime on native x86-64 and
ARM64 Ubuntu 24.04 runners. Each runner tests the runtime, unpacks its archive,
and uses it to compile and run a Python script. Pull requests that change the
release workflow, packaging inputs, or toolchain run the same check.

1. Set the same version in `crates/relar-compiler/Cargo.toml` and
   `crates/relar-runtime/Cargo.toml`. Commit the updated `Cargo.lock` and merge
   the version change into `main`.
2. To rehearse the release, run the workflow with its default `dry-run` input.
   The resulting archives and checksums are available as workflow artifacts.

   ```console
   gh workflow run release.yml --repo astral-sh/relar --ref main -f tag=dry-run
   ```

3. To publish, dispatch the workflow from `main` with the exact Cargo version.
   This example publishes `0.1.0`.

   ```console
   gh workflow run release.yml --repo astral-sh/relar --ref main -f tag=0.1.0
   ```

After both packages pass their smoke tests, the workflow checks their SHA-256
files and publishes the two archives and two checksums to GitHub Releases. It
creates a bare version tag at the commit selected by the dispatch. An existing
tag must already point to that commit. An existing release, including a draft,
stops publication. Versions such as `0.2.0-rc.1` are marked as prereleases.

#!/usr/bin/env bash
set -euo pipefail

if [[ "$GITHUB_EVENT_NAME" == pull_request ]]; then
  # The first parent of the PR merge commit is the base we are testing against.
  # Include both sides of renames, even when a change moves out of a watched path.
  git diff --no-renames --name-only -z HEAD^1 HEAD > "$RUNNER_TEMP/ci-paths"
  while IFS= read -r -d '' path; do
    case "$path" in
      .github/workflows/ci.yml|.github/scripts/ci-scope.sh|.cargo/*|\
      Cargo.toml|Cargo.lock|crates/*/Cargo.toml|rust-toolchain.toml)
        RELEASE_ARTIFACTS=true
        FULL_ASAN=true ;;
      .github/workflows/*release*.yml|.github/scripts/select-macos-sdk.sh|\
      dist-workspace.toml|pyproject.toml|uv.lock|scripts/release.sh|\
      scripts/bundle-linux-toolchain.py|scripts/prepare-shell-installer.py|\
      LICENSE-*|licenses/*|examples/conformance_report/upstream/LICENSE|\
      crates/imre-runtime-jemalloc/*|crates/*/build.rs|crates/imre-compiler/src/main.rs|\
      crates/imre-compiler/src/cli.rs|crates/imre-compiler/src/build_cache.rs|crates/imre-compiler/src/build_output.rs)
        RELEASE_ARTIFACTS=true ;;
    esac
    case "$path" in
      crates/imre-compiler/src/frontend.rs|crates/imre-compiler/src/frontend/*) ;;
      crates/imre-compiler/src/*|crates/imre-runtime*/*|crates/imre-regex/*|\
      tests/run_suites.py|tests/differential.py|tests/extension_wheel.py|tests/wheel_record.py)
        FULL_ASAN=true ;;
    esac
  done < "$RUNNER_TEMP/ci-paths"
fi

echo "release_artifacts=$RELEASE_ARTIFACTS" >> "$GITHUB_OUTPUT"
echo "full_asan=$FULL_ASAN" >> "$GITHUB_OUTPUT"

#!/usr/bin/env bash
set -euo pipefail

thinlto=false
if [[ "$GITHUB_EVENT_NAME" == pull_request ]]; then
  # The first parent of the PR merge commit is the base we are testing against.
  # Include both sides of renames, even when a change moves out of a watched path.
  git diff --no-renames --name-only -z HEAD^1 HEAD > "$RUNNER_TEMP/ci-paths"
  while IFS= read -r -d '' path; do
    case "$path" in
      .github/workflows/ci.yml|.github/scripts/ci-scope.sh|.cargo/*|\
      Cargo.toml|Cargo.lock|crates/*/Cargo.toml|rust-toolchain.toml)
        RELEASE_ARTIFACTS=true
        FULL_ASAN=true
        thinlto=true ;;
      crates/*/build.rs|crates/imre-compiler/src/main.rs|crates/imre-compiler/src/cli.rs|\
      crates/imre-compiler/src/build_cache.rs|crates/imre-compiler/src/build_output.rs)
        RELEASE_ARTIFACTS=true
        thinlto=true ;;
      .github/workflows/*release*.yml|.github/scripts/select-macos-sdk.sh|\
      dist-workspace.toml|crates/*/dist.toml|pyproject.toml|uv.lock|scripts/release.sh|\
      scripts/bundle-linux-toolchain.py|scripts/prepare-shell-installer.py|\
      LICENSE-*|licenses/*|examples/conformance_report/upstream/LICENSE|crates/imre-runtime-jemalloc/*)
        RELEASE_ARTIFACTS=true ;;
      # module_values also imports helpers from assignments, comprehensions and imports.
      .github/scripts/ci-thinlto-ld.lld|tests/lto.py|tests/programs/short_strings.py|\
      tests/run_suites.py|tests/run_suites_children.py|tests/module_values.py|\
      tests/fixtures/module_values/*|tests/assignments.py|tests/comprehensions.py|tests/imports.py)
        thinlto=true ;;
    esac
    case "$path" in
      crates/imre-compiler/src/frontend.rs|crates/imre-compiler/src/frontend/*) ;;
      crates/imre-compiler/src/*|crates/imre-runtime*/*|crates/imre-regex/*|\
      tests/run_suites.py|tests/assignments.py|tests/containers.py|tests/differential.py|\
      tests/extension_wheel.py|tests/wheel_record.py)
        FULL_ASAN=true ;;
    esac
  done < "$RUNNER_TEMP/ci-paths"
elif [[ "$GITHUB_EVENT_NAME" == workflow_dispatch ]]; then
  thinlto=true
fi

echo "release_artifacts=$RELEASE_ARTIFACTS" >> "$GITHUB_OUTPUT"
echo "full_asan=$FULL_ASAN" >> "$GITHUB_OUTPUT"
echo "thinlto=$thinlto" >> "$GITHUB_OUTPUT"

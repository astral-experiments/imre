#!/usr/bin/env bash
# CPython 3.14.5 for macOS uses the Darwin 25 errno namespace (SDK 26).
# Use an installed SDK with that namespace for native CI and release archives.
set -eu
shopt -s nullglob
for dev in "$(xcode-select -p)" /Applications/Xcode_26*.app/Contents/Developer; do
    sdk=$(DEVELOPER_DIR="$dev" xcrun --sdk macosx --show-sdk-path) || continue
    version=$(DEVELOPER_DIR="$dev" xcrun --sdk macosx --show-sdk-version) || continue
    if [[ ${version%%.*} == 26 ]]; then
        echo "Using macOS SDK $version at $sdk"
        echo "DEVELOPER_DIR=$dev" >> "$GITHUB_ENV"
        echo "SDKROOT=$sdk" >> "$GITHUB_ENV"
        # SDK selection changes the default minimum in Apple's C compiler. Keep
        # the runtime objects linkable on older supported Apple Silicon systems.
        echo "MACOSX_DEPLOYMENT_TARGET=11.0" >> "$GITHUB_ENV"
        exit 0
    fi
done
printf 'Cannot find macOS SDK 26 for CPython 3.14.5. Installed Xcodes: %s\n' /Applications/Xcode*.app >&2
exit 1

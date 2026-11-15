"""Install Imre's private directories alongside the cargo-dist binaries."""

import sys
from pathlib import Path

installer = Path(sys.argv[1])
install = '''    for _bin_name in $_bins; do
        ensure mv "$_install_temp/$_bin_name" "$_install_dir"'''
# Each replacement and its backup are on the destination filesystem.
replace_directories = '''    # Arguments: staged directory, destination, backup, ownership marker.
    replace_directory() {
        if [ -e "$2" ] || [ -L "$2" ]; then
            if [ -L "$2" ] || [ ! -f "$2/$4" ]; then
                err "refusing to replace an unrecognized $2"
            fi
            ensure mv "$2" "$3"
            if ! mv "$1" "$2"; then
                mv "$3" "$2" || err "could not restore $2 from $3"
                err "could not install $1. Restored $2"
            fi
        else
            ensure mv "$1" "$2"
        fi
    }

    _incoming="$_install_temp/imre-toolchain"
    if [ -e "$_incoming" ] || [ -L "$_incoming" ]; then
        if [ -L "$_incoming" ] || [ ! -s "$_incoming/licenses/sources.tsv" ] ||
           [ ! -x "$_incoming/bin/clang" ] || [ ! -x "$_incoming/bin/ld.lld" ] ||
           [ ! -s "$_incoming/link/Scrt1.o" ]; then
            err "incomplete imre-toolchain in the release archive"
        fi
    fi

    # Match InterpreterLayout for a bin install and a flat archive.
    _prefix="$_install_dir"
    if [ "$(basename "$_install_dir")" = bin ]; then
        _prefix=$(dirname "$_install_dir")
    fi
    _stdlib="$_prefix/lib/imre3.14"
    ensure mkdir -p "$_stdlib"
    _staging=$(mktemp -d "$_stdlib/.ensurepip.XXXXXXXXXX") || err "could not stage ensurepip"
    if ! (ensure cp -R "$_src_dir/lib/imre3.14/ensurepip" "$_staging/ensurepip"
          replace_directory "$_staging/ensurepip" "$_stdlib/ensurepip" \\
              "$_staging/previous-ensurepip" CPython.txt); then
        # Keep the backup if restoring the previous directory failed.
        [ -e "$_staging/previous-ensurepip" ] || rm -rf "$_staging"
        err "could not install ensurepip in $_stdlib"
    fi
    ensure rm -rf "$_staging"

    if [ -e "$_incoming" ]; then
        replace_directory "$_incoming" "$_install_dir/imre-toolchain" \\
            "$_install_temp/previous-toolchain" licenses/sources.tsv
    fi
    for _bin_name in $_bins; do
        [ "$_bin_name" = imre-toolchain ] && continue
        ensure mv "$_install_temp/$_bin_name" "$_install_dir"'''
text = installer.read_text()
if text.count(install) != 1:
    raise SystemExit("unexpected cargo-dist shell installation layout")
installer.write_text(text.replace(install, replace_directories))

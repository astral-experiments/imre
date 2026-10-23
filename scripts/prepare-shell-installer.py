"""Teach the cargo-dist shell installer to upgrade the Linux toolchain directory."""

import sys
from pathlib import Path

installer = Path(sys.argv[1])
install = '''    for _bin_name in $_bins; do
        ensure mv "$_install_temp/$_bin_name" "$_install_dir"'''
# cargo-dist stages each member of dist.binaries on the destination filesystem.
# Replace our nonempty directory before installing any of the other new members.
replace_directory = '''    _incoming="$_install_temp/relar-toolchain"
    _current="$_install_dir/relar-toolchain"
    _backup="$_install_temp/previous-toolchain"
    if [ -e "$_incoming" ] || [ -L "$_incoming" ]; then
        if [ -L "$_incoming" ] || [ ! -s "$_incoming/licenses/sources.tsv" ] ||
           [ ! -x "$_incoming/bin/clang" ] || [ ! -x "$_incoming/bin/ld.lld" ] ||
           [ ! -s "$_incoming/link/Scrt1.o" ]; then
            err "incomplete relar-toolchain in the release archive"
        fi
        if [ -e "$_current" ] || [ -L "$_current" ]; then
            if [ -L "$_current" ] || [ ! -f "$_current/licenses/sources.tsv" ]; then
                err "refusing to replace an unrecognized $_current"
            fi
            ensure mv "$_current" "$_backup"
            if ! mv "$_incoming" "$_current"; then
                mv "$_backup" "$_current" || err "could not restore the previous toolchain from $_backup"
                err "could not install the new toolchain. Restored $_current"
            fi
        else
            ensure mv "$_incoming" "$_current"
        fi
    fi
    for _bin_name in $_bins; do
        [ "$_bin_name" = relar-toolchain ] && continue
        ensure mv "$_install_temp/$_bin_name" "$_install_dir"'''
text = installer.read_text()
if text.count(install) != 1:
    raise SystemExit("unexpected cargo-dist shell installation layout")
installer.write_text(text.replace(install, replace_directory))

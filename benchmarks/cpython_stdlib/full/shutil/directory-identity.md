# Refusing a replaced directory in `rmtree`

When a filesystem supplies no directory entry type (`DT_UNKNOWN`), CPython's `entry.is_dir(follow_symlinks=False)` looks it up. Its `DirEntry` retains the result. Before descending, `rmtree` compares that device and inode with the opened file descriptor.

Imre used to look up the name twice. If another process exchanged directories between those lookups, the second lookup and the open both described the replacement. Imre then deleted the replacement's contents. The walker now retains the first identity. Entries with known types still need only their usual lookup before opening.

The [Linux control](rmtree_identity_controls.py) traces only its forked child in a fresh temporary tree. To model a filesystem with no type hint, it clears the optional type byte for one real directory entry. When the first successful stat returns, the parent moves the original directory aside and renames a second directory into its place. The stat results, opens, renames, unlinks, and resulting filesystem state are real.

| Scenario | Before | After, at O0 and O3 |
| --- | --- | --- |
| Stable directory, known type | Removed; one lookup | Removed; one lookup |
| Stable directory, unknown type | Removed; two lookups | Removed; one lookup |
| Exchanged directory, known type | Both trees preserved | Both trees preserved |
| Exchanged directory, unknown type | Replacement deleted | Both trees preserved |

For both exchanges, the corrected binary and CPython raised `OSError('Cannot call rmtree on a symbolic link')` with `filename == 'root/victim'`. The control checks both sentinel files as well as the exception.

The [before receipt](directory-identity-cffd1da-linux.json) and [after receipt](directory-identity-f81f349-linux.json) identify the source and binaries. The [hosted run](https://github.com/astral-sh/imre/actions/runs/40214243775) reran all 24 original `TestRmTree` methods: eight host-driven native operations, eight mixed, three blocked on Python replacements of OS calls, and five reference skips at each level. The race adds no original test credit. The after receipt reports the full 223-ID denominator separately.

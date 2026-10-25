# File cleanup used by logging tests

On POSIX, native `os.unlink(path, *, dir_fd=None)` and `os.remove` call
`unlinkat` on the supplied name. This removes a symbolic link itself, even when
its target does not exist. A directory descriptor supplies the starting
directory for a relative path. Text, bytes, and native pathlib paths preserve
the spelling and exception filename that CPython passes on to the caller.

The seeded [filesystem fixture](../../../../tests/fixtures/paths/os_unlink.py)
compares the operations and resulting directory entries with CPython. It also
checks trailing separators, invalid byte names, embedded NULs, and descriptor
overflow. The public FileHandler tests that use this cleanup are reported
individually in the logging evidence; this fixture adds no logging test IDs.

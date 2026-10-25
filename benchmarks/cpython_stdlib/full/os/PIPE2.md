# Linux pipes with explicit flags

`os.pipe2(flags)` and `posix.pipe2(flags)` create both descriptors with the
requested flags in one Linux syscall. With zero flags, the ends are inheritable.
Ordinary `os.pipe()` continues to create non-inheritable ends. Flags are
positional-only, use the integer index protocol and must fit in a C int.

The [O0](observations/699212be/original-O0.json) and
[O3](observations/699212be/original-O3.json) receipts cover both unchanged
CPython 3.14.5 POSIX pipe2 methods:

| Method | Native work | Host work |
| --- | --- | --- |
| `PosixTester.test_pipe2` | Four pipe creations, four closes and four descriptor flag checks | Original body and assertions, fixture hooks, `os.read` and `os.write` |
| `PosixTester.test_pipe2_c_limits` | Both overflowing calls to `os.pipe2` | Original body, assertions, fixture hooks and the local import of `_testcapi` |

The first method is mixed and the second is host-driven. The [60-line flag
control](observations/699212be/flags-linux.json) matched the pinned
interpreter at O0 and O3 in both definition scopes, including evaluation order,
integer overflow and invalid kernel flags. This control adds no upstream ID.

The [tuple controls](observations/20a42eac/tuple-flags-linux.json) also check
an empty tuple, one flag (including `__index__`), and two operands with visible
side effects. All 81 output lines matched CPython in each mode on the combined
pipe2 and iterator-repair source. Both original method reports were unchanged.

The [verification](observations/699212be/verification-linux.json) identifies
the compiler, runtime and 44 distinct original methods rerun at each level.
Only the two POSIX rows above change; the denominators remain 375 OS, 179
POSIX and 25 genericpath occurrences.

# Standard input at the native program boundary

The [input control](stdin_case.py) is a complete native program at O0 and O3.
It reads real descriptor 0 with `input()`, reports each line's character count
and code-point checksum, and tries reading again after EOF or a decoding error.
The [driver](stdin_encoding.py) feeds identical bytes to Imre and the pinned
CPython 3.14.5 binary. It compares status, stdout and stderr. A regular file
makes the 8192-byte chunk boundaries reproducible.

The [Linux receipt](stdin-encoding-linux.json) has 26 matching comparisons per
level. These exercise UTF-8, UTF-8 with BOM, ASCII and Latin-1, the stream's
error policy and field identities, split characters, failed decoder state,
partial lines, and CLI settings in C, POSIX and C.UTF-8 locales. A trailing
`-I` after the `-c` source is a script argument, as it is in the original
locale helper; `-I` before the command is tested separately.

One additional comparison differs. For byte `ff` with UTF-8/surrogateescape,
CPython returns U+DCFF. Imre's strings cannot store surrogate code points;
the native decoder reports `NotImplementedError`. Both observations are in
the receipt. These controls add no upstream method passes. Both original
C/POSIX locale methods still need dynamic lookup of a stream on the sys module.

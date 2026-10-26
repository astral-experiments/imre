# Shell output

The [O0/O3 observations](shell-output-linux.json) cover unchanged
`test.test_subprocess.MiscTests.test_getoutput` on Linux. The tested source
is [#1103](https://github.com/astral-sh/imre/pull/1103) at `31cca90`,
tree `e48bd67c8875adc91aaeefb7932de415d44c6940`. CPython 3.14.5 runs the
method, assertions and temporary directory fixture. Imre implements the
subprocess operations; the children are the system shell and its `cat`.

| Operation | Child writer | Bytes drained | Reaped status |
| --- | --- | ---: | ---: |
| `getoutput('echo xyzzy')` | shell stdout | 6 | 0 |
| `getstatusoutput('echo xyzzy')` | shell stdout | 6 | 0 |
| `getstatusoutput('cat ' + missing_filename)` | cat stderr | 53 in this run | 1 |

The driver identifies the native parent and shell, checks that stderr was
duplicated to stdout's pipe, matches child writes to native reads through EOF,
and observes that parent reap the shell. Withholding the failing status caused
the original `AssertionError: 0 == 0`. Both runs removed the original directory.

Run `shell_output_original.py` with pinned CPython and `--imre <compiler>
--runtime <archive> --clang <clang> --opt-level 0 --result <O0.json>`.
Repeat at level 3. The driver uses Linux `strace`. The complete native method
still stops at its tuple comparison in `TestCase.assertEqual`.

Use `shell_output_evidence.py --previous <prior-pair> --shell-output
shell-output-linux.json --result evidence-linux.json` to reproduce both
353-occurrence reports. Only this test changes.

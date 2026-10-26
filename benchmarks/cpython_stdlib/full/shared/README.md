# Shared CPython test fixtures

This directory contains original sources shared by the module suites. The [manifest](UPSTREAM.json) records the CPython commit and Git blob ID of each file. The files are distributed under the [upstream Python license](../../upstream/LICENSE).

The [staging command](../stage_tests.py) verifies Git blob IDs and assembles these files together with the module owners’ fixtures into a single source root. It includes the original `test.support` and `test.libregrtest` packages, the translation tool, and the certificates and keys used by CPython’s HTTPS tests. Product modules still resolve through the interpreter or compiler under test.

To check every original occurrence using a test-enabled CPython 3.14.5 on Linux:

```sh
fixture_root=$(mktemp -d)
python3 benchmarks/cpython_stdlib/full/stage_tests.py "$fixture_root"
"$CPYTHON" -I -B benchmarks/cpython_stdlib/full/discover_staged.py "$fixture_root"
PYTHONPATH="$fixture_root/Lib" "$CPYTHON" -B -m test --single-process --randseed 1 test_argparse
```

Discovery checks the complete ID multisets, including the duplicate JSON occurrences. It reports the single original free-threading package skip separately on a GIL build. Use each owner's reference resource flags for execution; compiling or discovering alone cannot establish a native pass.

If regrtest produces missing or unidentifiable JUnit results, the summary records
that module as `incomplete`, with its expected occurrence count and no observed
counts (`instances: null`). Inspect the module's XML and log. Other modules
still run and the overall command exits unsuccessfully.

For a full Linux reference run, an upstream unittest CLI test launches `sys.executable -E`, deliberately ignoring `PYTHONPATH`. Give it a [disposable prefix](../reference_prefix.py) so the child resolves the same `test` package as its parent:

```sh
reference_prefix=$(mktemp -d)
"$CPYTHON" -I -B benchmarks/cpython_stdlib/full/reference_prefix.py "$reference_prefix"
results=$(mktemp -d)
"$reference_prefix/bin/python3.14" -I -B benchmarks/cpython_stdlib/full/run_staged_reference.py --output "$results"
```

The prefix combines the supplied reference interpreter and its standard library with the verified test fixtures. It does not copy the interpreter's test tree or third-party packages. The runner sets `PYTHONHOME` to this disposable prefix so children can find their own standard library even when the original subprocess tests deliberately pass an unresolvable executable name. The original unittest child that passes `-E` locates the same prefix from its executable.

The runner verifies the installed fixtures again and uses the resource options from each module's ledger. It records original JUnit XML, runner output and a per-module `summary.json` in the result directory. Pass module names after `--output "$results"` for a smaller run. On Linux, run as a regular user with ordinary process and file permissions: some upstream tests exercise ptrace and permission changes.

Two subprocess tests that skip in the reference source build run and pass in the installed prefix. Their original decorator checks `sysconfig.is_python_build()` (reason: `need an installed Python. See #7774`). Those two passes are reported by name. Any other outcome difference makes the command fail.

Reference ledgers named `reference-free-threading-linux.json` were captured on a free-threaded 3.14.5 build with the GIL disabled. Re-run those ledgers separately with the same configuration; package-level skips recorded on the ordinary build stay in its ledger. Both result files name their configuration.

```sh
free_prefix=$(mktemp -d)
"$CPYTHON_FREETHREADED" -I -B benchmarks/cpython_stdlib/full/reference_prefix.py "$free_prefix"
"$free_prefix/bin/python3.14t" -I -B benchmarks/cpython_stdlib/full/run_staged_reference.py --output "$(mktemp -d)"
```

The [Linux installed run](installed-linux.json) records 7,294 ordinary-build occurrences (6,701 passes and 593 original skips) plus five free-threaded passes (four functools and one itertools) on Imre revision `4798a585c0a0`. Across both configurations there were no failures, extra skips or missing occurrences. These are CPython reference results; native coverage is recorded separately by each module.

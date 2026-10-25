# Standard library conformance

Start with the [module inventory](scope.toml). Each first-wave module has a directory here for its unchanged source, the reference run, and native evidence. Existing subset runners live [one directory up](../README.md).

Run `python3 benchmarks/cpython_stdlib/full/reference_report.py` to summarize the pinned Linux reference runs, including companion suites and repeated test IDs. `--json` emits the same counts for tooling. Native execution is recorded separately in each module's evidence.

## Scope

Run `python3.14 -B benchmarks/cpython_stdlib/full/inventory.py` using **CPython 3.14.5**. It enumerates the interpreter's publicly named top-level modules, checks that assignments do not overlap, and prints every unassigned entry. Use `--json` for machine-readable output. The initial priority is the common application and data-processing modules. A proposed exclusion needs a reason; being unassigned or platform-specific does not remove a module. Include relevant dotted APIs, private implementation dependencies, package tests, and companion suites when a module is assigned.

## When a module is complete

1. Pin Python and tests to [CPython 3.14.5](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115). Vendor complete upstream test modules and their data, scripts and license; verify original blobs. Modifying an assertion or selecting only test methods the compiler can already handle changes the workload.
2. Discover with the upstream loader, including package `load_tests`. Record both executions and distinct IDs: e.g. test_json registers four IDs twice. Include generated tests, resource configuration and companion suites in the record.
3. Run the unchanged suite on reference CPython on the target. Keep each pass, failure and skip, including its original condition and reason. A reference failure or platform skip does not become a native pass.
4. On Imre, count a test as native only when the original body, assertions, fixtures and operation under test execute on Imre. A host runner may launch that binary and compare output. If CPython supplies an implementation, a test body, a fixture or an assertion, record the case as host-assisted. Record compile failures, runtime failures and cases not yet reached separately. Handwritten parity controls do not add upstream test IDs.
5. For every extra exception record the exact test ID and fixture variant, affected platform/configuration, reason, and a regression or issue to revisit. No whole-file, class, or feature-family exemptions. A module finishes only when every collected execution is accounted for and remaining differences have individually reviewed exceptions.

Land one reviewable module change or shared prerequisite at a time. Keep upstream test identities stable as fixes move between owners. A vendor-inventory PR or working application does not, by itself, mark a module complete.

## Finding the first compiler error in a whole suite

Run `python3 benchmarks/cpython_stdlib/full/check_suite_startup.py --compiler /path/to/imre --cpython-git /path/to/cpython test.test_argparse`. The CPython Git repository must contain the pinned commit; the command stages its original `Lib/test` from Git objects, including support code and data. Without suite names it checks all wave-one primary suites. List companion suites explicitly, e.g. `test.test_posix test.test_genericpath`.

Each suite gets two entry points: the import alone and a standard `unittest` loader and runner. Both are checked in ordinary and reachable-only mode. JSON records the diagnostics and the command exits nonzero when a check fails. No tests execute during a check. For a missing private Python helper, add its original source with e.g. `--support-file _colorize.py`. The rest of the standard library still resolves through Imre; staging an entire upstream `Lib` would shadow its native modules.

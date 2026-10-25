# Standard library conformance

Start with the [module inventory](scope.toml). Each first-wave module has a directory here for its unchanged source, the reference run, and native evidence. Existing subset runners live [one directory up](../README.md).

Run `python3 benchmarks/cpython_stdlib/full/reference_report.py` to summarize the pinned Linux reference runs, including companion suites and repeated test IDs. `--json` emits the same counts for tooling. Native execution is recorded separately in each module's evidence.

## Scope

Run `python3.14 -B benchmarks/cpython_stdlib/full/inventory.py` using **CPython 3.14.5**. It enumerates the interpreter's publicly named top-level modules, checks that assignments do not overlap, and prints every unassigned entry. Use `--json` for machine-readable output. The initial priority is the common application and data-processing modules. A proposed exclusion needs a reason; being unassigned or platform-specific does not remove a module. Include relevant dotted APIs, private implementation dependencies, package tests, and companion suites when a module is assigned.

## When a module is complete

Run `python3 benchmarks/cpython_stdlib/full/native_report.py` for coverage backed by [per-occurrence evidence](../NATIVE_EVIDENCE.md), or add `--json` for the role counts at each optimization level. The table lists the full reference denominator and the number of occurrences covered by a validated ledger. Only the same occurrence and fixture classified as fully native at **both** O0 and O3 earns paired credit. A dash means there is no validated ledger. The categories in a ledger may include unattempted cases and host-assisted work.

Save the two reports beside their reference ledger, replacing the `reference` filename prefix with `evidence` (including any companion suite suffix). The existing versioned `native` filename is also supported; older method summaries do not establish occurrence coverage.

1. Pin Python and tests to [CPython 3.14.5](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115). Vendor complete upstream test modules and their data, scripts and license; verify original blobs. Modifying an assertion or selecting only test methods the compiler can already handle changes the workload.
2. Discover with the upstream loader, including package `load_tests`. Record both executions and distinct IDs: e.g. test_json registers four IDs twice. Include generated tests, resource configuration and companion suites in the record.
3. Run the unchanged suite on reference CPython on the target. Keep each pass, failure and skip, including its original condition and reason. A reference failure or platform skip does not become a native pass.
4. On Imre, count a test as native only when the original body, assertions, fixtures and operation under test execute on Imre. A host runner may launch that binary and compare output. If CPython supplies an implementation, a test body, a fixture or an assertion, record the case as host-assisted. Record compile failures, runtime failures and cases not yet reached separately. Handwritten parity controls do not add upstream test IDs.
5. For every extra exception record the exact test ID and fixture variant, affected platform/configuration, reason, and a regression or issue to revisit. No whole-file, class, or feature-family exemptions. A module finishes only when every collected execution is accounted for and remaining differences have individually reviewed exceptions.

Land one reviewable module change or shared prerequisite at a time. Keep upstream test identities stable as fixes move between owners. A vendor-inventory PR or working application does not, by itself, mark a module complete.

# Intact Linux logging diagnostic

The unmodified CPython 3.14.5 test.test_logging module ran on stock CPython and on public Imre at O0 and O3. Each process discovered, started, and stopped all 272 original parents. This whole-suite diagnostic adds zero registrations to the separate selected-case ledger.

| Lane | Natural exit | Tests run | Failures | Errors | Skips |
| --- | ---: | ---: | ---: | ---: | ---: |
| stock | 0 | 272 | 0 | 0 | 3 |
| O0 | 1 | 272 | 117 | 21 | 5 |
| O3 | 1 | 272 | 117 | 21 | 5 |

These totals come from the unittest TestResult and include subtest failures or errors. The lane-specific parent-callbacks.tsv files preserve parent and subtest callbacks separately. Each suite.json records the command, process status and raw output hashes. Each outcome.json records callback counts and source bindings.

The observer is entry.py. The raw-run-files.tar.gz archive contains original stdout and stderr from all three processes, the pinned source, the worker runner and verifier, and the native build and deadline-extension diagnostics. Extract the archive here and run sha256sum -c source-SHA256SUMS to check the worker’s original complete intact diagnostic manifest.

Public Imre revision is 8d3fe99e55c190cc72bfd78d2b5afe46de48e8ce. CPython revision is 5607950ef232dad16d75c0cf53101d9649d89115. Source manifest SHA-256 is f0d2b1c379b064eee40dd4ac59124f0728f8a882f46f57c7630bb9209b7e81e3.

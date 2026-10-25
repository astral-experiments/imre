# POSIX readiness

The native product surface is `select.poll()` and `select.select()` for POSIX integer descriptors. A poller remembers its own interests between waits; no process-global table or socket implementation is involved. The six portable event constants come from the target's headers.

[UPSTREAM.json](UPSTREAM.json) pins the original CPython 3.14.5 C implementation, Python selector layer, three full test modules, and license by Git blob. The two companion manifests select the corresponding upstream test entry point for the existing reference runner.

| Complete Linux reference | Discovered | Passed | Skipped | Native observation at O0 and O3 |
|---|---:|---:|---:|---|
| [test_select](reference-select-linux.json) | 6 | 6 | 0 | [Two original bodies](evidence-select-linux.json): `test_errno` and `test_returned_list_identity` |
| [test_poll](reference-poll-linux.json) | 7 | 6 | 1 | [One original body](evidence-poll-linux.json), `test_poll3`; one host-driven original, `test_poll1` |
| [test_selectors](reference-selectors-linux.json) | 121 | 77 | 44 | Reference only |

Each native original runs its complete original method bytes, with native readiness operations and assertions. The host extracts and schedules the method. The two select assertion fixtures specialize list identity and optional errno; the poll overflow method uses Imre's native `TestCase.assertRaises`. In host-driven `test_poll1`, CPython executes the original 12-pipe test, including random choice, reads/writes, interest bookkeeping and assertions. A native executable registers those interests and waits on inherited fds at each call.

The negative controls leave the fd open, pass the same list twice, register a valid mask in place of the invalid one, or suppress writable events in the host bridge. Each must cause the relevant original assertion or readiness check to fail. Additional differential cases cover independent pollers, replacing masks, multiple idle/readable/writable fds, pipe backpressure and recovery, EOF, closed/reused numbers, and bounded and indefinite waits. None of these extra cases increments the original-method count.

The Linux reference uses default regrtest resources. One poll case needs `walltime`; the selector ledger records 42 unavailable kqueue/devpoll backend cases and two `cpu` resource skips. Other unobserved IDs remain reference-only. The unchanged modules do not start under the ordinary or reachable-only native compiler: the poll and selector suites import `random`, and the select suite performs a module-level call before its test class declaration. These boundaries were checked against the staged test tree in both compiler modes.

To reproduce with a test-enabled CPython 3.14.5:

```sh
dir=benchmarks/cpython_stdlib/full/select
python3.14 benchmarks/cpython_stdlib/suite_inventory.py "$dir/UPSTREAM.json" --result /tmp/reference-select.json
python3.14 benchmarks/cpython_stdlib/suite_inventory.py "$dir/poll-UPSTREAM.json" --result /tmp/reference-poll.json
python3.14 benchmarks/cpython_stdlib/suite_inventory.py "$dir/selectors-UPSTREAM.json" --result /tmp/reference-selectors.json
python3.14 tests/posix_readiness.py --compiler target/release/imre --runtime target/release/libimre_runtime.a \
    --clang clang --host-original --evidence-dir /tmp/readiness-evidence
```

Linux CI checks both optimization levels; Apple Silicon checks O3. The higher-level selector suite is retained as an inventory for the next interface.

# POSIX readiness

The native product surface is `select.poll()` and `select.select()` for POSIX integer descriptors. A poller remembers its own interests between waits; no process-global table or socket implementation is involved. The six portable event constants come from the target's headers. `selectors.PollSelector` builds on that poller; `DefaultSelector` selects the same backend because Imre does not currently expose epoll, kqueue or devpoll.

[UPSTREAM.json](UPSTREAM.json) pins the original CPython 3.14.5 C implementation, Python selector layer, three full test modules, and license by Git blob. The two companion manifests select the corresponding upstream test entry point for the existing reference runner.

| Complete Linux reference | Discovered | Passed | Skipped | Native observation at O0 and O3 |
|---|---:|---:|---:|---|
| [test_select](reference-select-linux.json) | 6 | 6 | 0 | [Two native bodies](evidence-select-linux.json): `test_errno`, `test_returned_list_identity`; one host-driven: `test_select` |
| [test_poll](reference-poll-linux.json) | 7 | 6 | 1 | [One native body](evidence-poll-linux.json): `test_poll3`; three host-driven: `test_poll1`, `test_poll_c_limits`, `test_poll_blocks_with_negative_ms` |
| [test_selectors](reference-selectors-linux.json) | 121 | 77 | 44 | [Four native IDs](evidence-selectors-linux.json): `test_empty_select` and `test_unregister_after_fd_close` from both `PollSelectorTestCase` and `DefaultSelectorTestCase`; two host-driven IDs: their inherited `test_unregister_after_fd_close_and_reuse` |

The seven native IDs execute their unchanged method bytes with native assertions and fd operations. The host extracts the method and supplies a typed fixture. The select fixtures specialize list identity and optional errno; the poll overflow method uses Imre's `TestCase.assertRaises`. The two selector classes inherit the same two method bodies. A native fixture defers selector cleanup and receives a real host socketpair; the original method closes and unregisters the integer fds itself.

In the four host-driven poll and select IDs, CPython owns the assertions and fixtures. Imre waits on the original 12 pipes, blocks on negative timeouts until a host thread writes, validates `_testcapi` boundary values, or selects the subprocess output pipe. Controls suppress writable events, force a zero timeout, hide an overflow, or add a spurious writer; each fails the original test. Native controls also leave a bad fd open, alias the same list, make an invalid mask valid, replace an empty result, or unregister the same fd twice.

For the two host-driven reuse IDs, the unchanged CPython method directs one persistent Imre selector. Both processes close the registered numbers and reuse them with `dup2`. Imre verifies the original pair was idle, the replacement pair has a byte queued, and neither registration survives unregister. Dropping a `dup2` or unregistering the same fd twice fails the test.

Additional differential scenarios cover independent pollers, backpressure and recovery, EOF and fd reuse, timeouts, key identity and data replacement, mask translation, and selector cleanup. The [two-client socket check](selector_sockets.py) keeps one connection idle while the other switches from read to write, then verifies readable EOF when the idle peer closes. The [subprocess backend check](../../../../tests/fixtures/processes/select_backend.py) uses the selector classes to choose how `Popen.communicate` waits for pipes. These observations do not increase the upstream method count.

The Linux reference uses default regrtest resources. One poll case needs `walltime`; the selector ledger records 42 unavailable kqueue/devpoll backend cases and two `cpu` resource skips. Other unobserved IDs remain reference-only. The native evidence covers the individual methods and operations listed above; the complete suite inventory comes from the unchanged CPython modules.

To reproduce with a test-enabled CPython 3.14.5:

```sh
dir=benchmarks/cpython_stdlib/full/select
python3.14 benchmarks/cpython_stdlib/suite_inventory.py "$dir/UPSTREAM.json" --result /tmp/reference-select.json
python3.14 benchmarks/cpython_stdlib/suite_inventory.py "$dir/poll-UPSTREAM.json" --result /tmp/reference-poll.json
python3.14 benchmarks/cpython_stdlib/suite_inventory.py "$dir/selectors-UPSTREAM.json" --result /tmp/reference-selectors.json
python3.14 tests/posix_readiness.py --compiler target/release/imre --runtime target/release/libimre_runtime.a \
    --clang clang --host-original --evidence-dir /tmp/readiness-evidence
```

Linux CI checks both optimization levels; Apple Silicon checks O3.

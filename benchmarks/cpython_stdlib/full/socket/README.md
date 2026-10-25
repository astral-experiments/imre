# CPython socket coverage

The manifest pins the original Python wrapper, socket C implementation and header,
complete test module, socket/thread fixture helpers and license to CPython 3.14.5.
The stock Linux regrtest run collected 746 distinct methods: 681 passed and 65
skipped. The reference records the reason for each skip; the native ledger links
each one to its upstream decorator or runtime condition.

| At both O0 and O3 | Methods | What ran |
| --- | ---: | --- |
| Native body, host fixture | 7 | `BasicTCPTest.testRecv`, `testOverFlowRecv`, `testSendAll` and `GeneralModuleTests.test_listen_backlog`; three `InheritanceTest` methods cover default and changed descriptor flags and duplication. |
| Native body, mixed operations | 3 | The same three TCP methods under `BasicTCPTest2`; its defining `socket.create_connection` call runs on CPython. |
| Host body, native operations | 2 | `GeneralModuleTests.testGetSockOpt` and `testSetSockOpt`. |
| Reference only | 669 | Stock CPython passed; no Imre execution is claimed. |
| Reference skip | 65 | Stock CPython skipped on this host; these are not Imre exclusions. |

The receiving methods and their original senders compile from complete,
unedited definitions. The stock module, threaded class setup and cleanup run
on CPython. Each peer gets its fixture's connected fd via subprocess inheritance;
`os.dup2` gives a native socket a copy while the host keeps the original fd.
Both method bodies perform the real send/recv/sendall on Imre. `BasicTCPTest2`
also runs its original `NetworkConnectionTest.clientSetUp`, which calls stock
`socket.create_connection`, so its three occurrences have mixed operations.
The native children use strict bytes equality in place of Imre's integer-typed
TestCase.assertEqual; the CPython children use the stock assertion. Each of
the six receiving occurrences must fail its own assertion on a corrupt wire.
A paired sender is evidence for the same occurrence, never an additional
method. The backlog method runs its original three binds, listens and context
cleanups under the stock module fixture.

The three unmodified InheritanceTest methods create or duplicate real native
descriptors under the stock module fixture. Their boolean comparisons run
through native TestCase.assertTrue; a deliberately inheritable socket must fail
the default-flag assertion. The separate socket-only check observes
FD_CLOEXEC through os in both directions, verifies that copying restores the
source socket's stored blocking mode and transmits data after the original
closes. Closed descriptor operations must report EBADF.

For the two option methods, stock unittest executes the unchanged test and
registered cleanup. A pipe forwards each constructor/method call to one native
socket. Initializing reuse incorrectly or clearing it when the test requested
setting it must make each original assertion fail. The result contains **zero
fully native fixture lifecycles**. The whole unmodified test module with a full
CPython Lib module root currently stops at standard-library shadowing of
`unittest`; this compile diagnostic is not an upstream skip.

Reproduce using a Linux test-enabled CPython 3.14.5 and the built Imre compiler:

```sh
dir=benchmarks/cpython_stdlib/full/socket
python3.14 benchmarks/cpython_stdlib/suite_inventory.py "$dir/UPSTREAM.json" --result /tmp/socket-reference.json
python3.14 "$dir/run.py" --compiler target/release/imre --runtime target/release/libimre_runtime.a --clang clang-18 --output /tmp/socket-evidence.json
python3.14 benchmarks/cpython_stdlib/suite_evidence.py "$dir/reference-linux.json" /tmp/socket-evidence.json
```

A different kernel, user, network namespace or set of enabled regrtest resources
can change the stock reference skips. The selected TCP methods need only
loopback. The separate `tests/socket_tcp.py` exercises the public HTTP socket
surface; it contributes no original-test method counts.

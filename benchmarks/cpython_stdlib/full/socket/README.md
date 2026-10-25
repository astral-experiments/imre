# CPython socket coverage

The manifest pins the original Python wrapper, socket C implementation and header,
complete test module, socket/thread fixture helpers and license to CPython 3.14.5.
The stock Linux regrtest run collected 746 distinct methods: 681 passed and 65
skipped. The reference records the reason for each skip; the native ledger links
each one to its upstream decorator or runtime condition.

| At both O0 and O3 | Methods | What ran |
| --- | ---: | --- |
| Native body, host fixture | 4 | `BasicTCPTest.testRecv`, `testOverFlowRecv`, `testSendAll` and `GeneralModuleTests.test_listen_backlog`. |
| Host body, native operations | 2 | `GeneralModuleTests.testGetSockOpt` and `testSetSockOpt`. |
| Reference only | 675 | Stock CPython passed; no Imre execution is claimed. |
| Reference skip | 65 | Stock CPython skipped on this host; these are not Imre exclusions. |

The three TCP receiving methods and their original sending methods compile from
complete, unedited definitions. Both peers use native sockets; the host starts
the processes, reads the ephemeral port, and runs the upstream module setup and
cleanup. Two processes replace the suite's threaded class fixture. The child
uses strict bytes equality in place of Imre's integer-typed TestCase.assertEqual;
the CPython child uses its stock assertion. Each unchanged receiving method
must fail its assertion when a peer sends incorrect bytes. A paired sender is
evidence for the same method, never an additional occurrence. The backlog method
runs its three binds and listens, including context cleanup, in one process.

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

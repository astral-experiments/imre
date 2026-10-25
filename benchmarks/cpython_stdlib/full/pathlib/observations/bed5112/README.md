# Complete Linux probes on bed5112

CPython 3.14.5 discovers 1,369 occurrences through the package. Its Linux reference passes 978 and skips 391. The complete-method probe tried all 978 passing bodies under their original host fixtures at both optimization levels. Each produced 121 native candidates, 818 compiler diagnostics and 39 original fixture wrappers. A candidate needs a review of its fixture and product operations before contributing to native compatibility. Every per-method record agrees with the independent sweep on `4375dae`.

A disk outage interrupted the first O3 run. The repeat used the same compiler, runtime and original test blobs for 112 missing and 14 contaminated occurrences. The combined report contains exactly 978 IDs at each level. Each method hash and outcome agrees across levels. The first attempt is recorded separately in [recovery-linux.json](recovery-linux.json).

The [default fixture report](default-fixtures-linux.json) applies the shared native unittest entry to all 978 reference passes. At each level it rejected 977 original fixture declarations and attempted `UnsupportedOperationTest.test_is_notimplemented`. Its first unchanged assertion, `assertIsSubclass`, did not compile (unknown class method). No default-fixture body ran. These 978 blocks describe only this entry; the primary report retains stronger measured operations under host fixtures.

Run `default_fixtures.py` with the pinned Linux CPython, `-B -X warn_default_encoding`, a matching Imre compiler/runtime, `--clang`, the full `--imre-commit`, and a `--result` path. The report includes the binary and runner hashes. [body-probes-linux.json](body-probes-linux.json) preserves both optimization levels for every passing reference ID.

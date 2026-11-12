# Logging fileConfig on #7178

CPython 3.14.5 passed all twelve original `ConfigFileTest` methods through FILE and unittest. At Imre commit `1031174e061b795536086b4242d4d8504f8c57d1`, FILE and declared-module unittest each ran all twelve methods and reported twelve errors at O0 and O3. Their paired credit was already zero at #6837. Logging retains 220 of 272 credited methods. [Per-occurrence results](logging-fileconfig-recheck-7178.tsv) · [#6837 results](logging-fileconfig-recheck-6837.tsv)

The complete upstream test file has SHA-256 `529fb98e8a0890d562d8d8903c09700635ad8f67cef4bad6f749f89d3442a563`. FILE printed each method under `__main__.ConfigFileTest`. The declared-module launcher included `unittest.__main__` and `test.test_logging` and printed the original fully qualified IDs. Both routes used the upstream fixture setup and assertions. [Upstream source](upstream/Lib/test/test_logging.py)

| Mode | PID | Raw Linux wait | Exit | Receipt in evidence archive |
|---|---:|---:|---:|---|
| CPython FILE | 89039 | 0 | 0 | `stock/file/receipt.json` |
| CPython unittest | 89020 | 0 | 0 | `stock/declared/receipt.json` |
| Imre O0 FILE | 91110 | 256 | 1 | `native/O0/file/receipt.json` |
| Imre O3 FILE | 91753 | 256 | 1 | `native/O3/file/receipt.json` |
| Imre O0 declared module | 92266 | 256 | 1 | `native/O0/declared-module/receipt.json` |
| Imre O3 declared module | 92899 | 256 | 1 | `native/O3/declared-module/receipt.json` |

In each tier the exception frames showed eight `StreamHandler`, two `logging.StreamHandler` and one `FileHandler` strings at `_install_handlers`, line 154. The remaining method reached `_create_formatters`, line 132, with the text `{"customfield": "defaultvalue"}` before handlers were cleared. The evaluator raised `NotImplementedError` for the missing compiled expression body. Separate stock controls observed the configured evaluator receiving the class, positional arguments and keyword arguments in order. A rebound `logging` global could select a different handler from absolute module resolution. [Configuration source](../../crates/imre-compiler/stdlib/logging/config.py)

Direct execution of the explicit `runpy.py` source stopped at its open `exec` on line 88 before collection. Explicit check and build of the complete test file stopped at `test_logging.py:1901:27` on the argument to `skipTest`. Neither tier emitted a standalone executable.

The Linux archive `logging-fileconfig-evidence-38ef2f4484fd2d8c.tar.zst` has SHA-256 `38ef2f4484fd2d8c7e3fc9679d8040d4a372a14d33e5afb7304e3db6943424b6`. Each process directory contains its original argv, wait and both raw streams. `analysis/exception-observer/verified.json` records each native frame value. `candidate-analysis/controls.json` records the evaluator and exception-order controls, and `candidate-analysis/dotted/receipt.json` records the rebound handler choice. The single verified transport piece for the #6837 input TSV was 2,107 bytes with SHA-256 `633c68faada04cdfe645754d409401b2fa918613a6822ffe5b47a257d2aec536`.

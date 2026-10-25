# Configured stdout and CP424

[Linux CI](https://github.com/astral-sh/imre/actions/runs/40208904863/job/121215714515) exercised the combined [#1105](https://github.com/astral-sh/imre/pull/1105) and [#1090](https://github.com/astral-sh/imre/pull/1090) source with the [#945](https://github.com/astral-sh/imre/pull/945) codec writer. [Source and hashes](source.json) identify the build. The [two versioned reports](evidence-linux.json) describe this source only.

At each of O0 and O3 the original `test.test_sys.SysModuleTest.test_ioencoding` ran all five native `-c` children, with CPython running the original decorators, fixture, method and assertions. The first four satisfy those assertions. CP424 emits `4a25` and ASCII replacement emits `3f0a`. Both strict ASCII settings return 1 and report `UnicodeEncodeError`; the full native stderr lacks CPython's traceback, which the original assertions do not require.

The fifth child, `print(chr(0xdcbd))` under `:surrogateescape`, exits 1 with empty stdout. Stock CPython exits 0 and writes `bd0a`; its original assertion sees `bd` after stripping. The method remains failed. Each 97-occurrence report contains 81 blocked, 1 failed, 3 host-driven native operations, 2 mixed, 2 native bodies under host fixtures and 8 reference skips. No exclusions were approved.

# Native StringIO prerequisite for doctest and CSV

CPython 3.14.5, commit `5607950ef232dad16d75c0cf53101d9649d89115`.
The JSON module docstring writes a list through `json.dump`, inspects the
StringIO with `getvalue`, and later reads it through `json.load`. Independently,
`test_csv.Test_Csv.test_write_lineterminator` writes rows into a StringIO
with five terminators; several `TestDictFields` methods also inspect, seek,
or iterate a memory stream. The CSV writer and its dialects belong to CSV.

| Original input | Git blob |
| --- | --- |
| `Lib/test/test_memoryio.py` | `482b183da23ffa259664e52592021d5c3c12d34d` |
| `Lib/test/test_csv.py` | `a0a1e91777d89345d543b27ee23049162cf3a0a3` |
| `Lib/json/__init__.py` | `9eaa4f3fbc16793adc061f9f5ee9dd0aab159b20` |
| `Modules/_io/stringio.c` | `b073200bf2120414f2974803e1b49a20bb931a9c` |
| [CPython license](../../itertools/upstream/LICENSE) | `20cf39097c68baa17cc566b64e76d34ebf034044` |

Run the [shared staging command](../../stage_tests.py) into an empty directory,
then pass that directory to `native.py` as `--cpython-source`, along with
`--imre`, `--runtime`, `--clang`, and `--result`, using the pinned Python.
It verifies the [vendored test module and license](../UPSTREAM.json) and the
staged test file, selects complete methods by their original IDs, and
uses the shared fixture runner. `observations-linux.json` records which
part ran where at each optimization level. These selected prerequisite
methods do not add occurrences to the primary itertools or JSON suites.

The [logging foundation audit](logging-base-linux.json) ran 19 of the 39
original `CStringIOTest` methods on PR #869's published compiler/runtime
(`9ebd301`). At each optimization level, seven complete native methods passed
under the host fixture; four host methods drove 34 native operations; eight
methods stopped at compilation. Separate CSV and JSON shared-stream controls
matched CPython. The other 20 methods were outside this prerequisite audit.

In the [combined implementation run](https://github.com/astral-sh/imre/actions/runs/40179882259/job/121128473529),
20 of the 39 original class methods passed at each optimization level. Eight
bodies and their assertions run in Imre under the original host fixture,
including the non-BMP overwrite/append and character-position checks. Twelve
more run unchanged on the host and make 183 calls per level on a live native
`StringIO` in a child process. These cover all five newline modes, the default
mode, cumulative newline history, relative seeks, reading after closing, and
writing after seeking beyond EOF, and closed/open stream flags. The 19
unselected methods are outside this prerequisite audit. Each observation records the actual roles,
the method hash and, for host bodies, the number of native calls by operation.

[The initial integration run](integration-linux.json) used the CSV reader from
#468, shallow copy from #603, and doctest from #646. In the
[run after annotationlib landed](integration-annotationlib-linux.json), all
four untouched CSV import/runner checks stop at `tempfile.TemporaryFile`;
all eight JSON and itertools checks reach `import inspect` at line 9 of
`test.support`. [After inspect #784 landed](integration-inspect-linux.json),
the CSV results stayed the same and the eight JSON and itertools checks reached
`import _opcode` at line 11. Both the vendored files and the pinned Git checkout
gave those results. None of these entry points executed a test body. Compiling the
unchanged 32-example JSON doctest directly advances its first diagnostic from
`io.StringIO` at example 7 to `import decimal` at example 26, at both `-O0`
and `-O3`. These compile diagnostics do not count earlier examples as passes.

`consumer_startup.py` invokes the shared startup probe for all twelve import
and runner combinations on every CI run. It checks that the vendored and full
Git fixtures agree and prints each compile result, including its first blocker.
These checks do not execute test bodies. [On the combined implementation](unified-startup-linux.json),
CSV reaches `tempfile.TemporaryFile`; both JSON and itertools reach `warnings`
in normal mode. Reachable mode stops at `_imp` for JSON and at an unbounded
starred tuple for itertools. The [original module doctest report](unified-doctest-linux.json)
records the separate direct checks at both optimization levels.

[After the StringIO foundation merged](merged-foundation-startup-linux.json),
normal and reachable compilation report the same first blocker for each suite.
For CSV it is `tempfile.TemporaryFile`, for JSON it is `_imp` imported by
`test.support.import_helper`, and for itertools it is the starred tuple in
`test.support.threading_helper` at line 277. All twelve checks agree between
the vendored fixtures and the pinned Git tree. The twenty selected stream
methods still reproduce the observations above at O0 and O3.

The separate `streams.py` transcript covers all five newline modes, Unicode
character offsets, shared iteration and reads, EOF followed by seek, retained
newline-history tuples, NUL padding past the end, and closed-stream errors.
It also interleaves a quoted multiline CSV reader with direct reads and seeks
on the same stream, and runs the JSON streaming operations. The native executable is compared
with the pinned interpreter at both `-O0` and `-O3`.

This is the native text stream for the supported Imre string representation.
Lone surrogate code points and arbitrary Python subclasses still need shared
string or object support. The complete upstream CSV and doctest suites have
additional dependencies; their results are recorded with their fixtures.

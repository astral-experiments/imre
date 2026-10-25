# ArgumentParserError fixture

The pinned [test_argparse.py](upstream/Lib/test/test_argparse.py) calls
`Exception.__init__(self, message, stdout, stderr)` and assigns four instance
fields. The error code is an instance field and does not appear in `.args`.

Compiling all 417 source-defined methods with [native_methods.py](native_methods.py)
on main (`bb0e2a0`) identified the methods that stopped at this initializer.
The [per-ID report](exception-fixture-blockers-linux.json) matches those same
methods against the compiler and runtime at `ce7f49a`. The two optimization
levels produce identical diagnostics and source locations:

| Upstream source | Affected methods | Next diagnostic |
| --- | ---: | --- |
| `class StdIOBuffer(io.TextIOWrapper):` (line 37) | 148 | the base must be a known fixed-layout class |
| `self.message = message` (line 164) | 4 | field "message" is not declared in the class |

The four are `TestAddSubparsers.test_parse_args_failures`,
`test_parse_args_failures_details`, `test_subparser1_help` and
`test_subparser2_help`. The other 148 include `test_abbreviation`. The
diagnostic locations were matched against the exact fixture assembled by the
shared runner; the original method, helpers, and assertions were unchanged.
None of these 152 method bodies executed. The separate exception regression
fixture exercises storage, formatting, and retrieval of the native arguments;
it matched CPython's 22 output lines, empty stderr, and exit status 0 at
both levels under AddressSanitizer and LeakSanitizer. The separate exception
regression suite passed 85 checks, including the distinction between
`ImportError.args` and the last successfully initialized `ImportError.msg`.

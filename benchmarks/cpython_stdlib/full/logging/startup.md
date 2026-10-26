# Importing the unchanged logging suite

The resource-enabled CPython suite discovers [272 IDs](reference-linux.json).
The full native entry imports `test.test_logging`; the runner additionally calls
`unittest.defaultTestLoader.loadTestsFromModule` and `TextTestRunner.run`.
Both are checked with and without `--reachable-only`. The module root contains
CPython's unchanged tests. An extra `Lib` source package can shadow an Imre
module before any fixture is reached.

| Compiler | First diagnostic from all four entries |
| --- | --- |
| Main `76d74f9` | `test_logging.py`: starred elements in tuple displays are unsupported. |
| Main `0ea6595` plus bounded display lowering | Line 22, `import logging.handlers`. |
| Main `e2c5130` (includes [#523](https://github.com/astral-sh/imre/pull/523)) plus bounded display lowering | Line 23, `import logging.config`: standard-library imports and shadowing are unsupported: `"logging"`. |
| Main `32d0289` plus [#588](https://github.com/astral-sh/imre/pull/588) at `5b8ed5b` | Line 23, `import logging.config`, in all four checks. This replay uses main's checked keyword diagnostics and adds only the exact tuple-list subscript diagnostic proved by lowering. |
| Main [`0cf6abb`](https://github.com/astral-sh/imre/commit/0cf6abbf39e6bc6f24c8027716c03e5c4b4a8871), including [#626](https://github.com/astral-sh/imre/pull/626) | Line 26, `import codecs`, in all four checks. The compiler used the exact published source tree. The import still precedes every logging fixture. |

The [record-field replay on 4fe0c598](startup-record-fields-linux.json) used a neutral directory containing the unchanged test blob. Both the module and unittest runner stop at line 27, `import configparser`, in ordinary compilation and line 32, `import pickle`, in reachable-only compilation at O0 and O3. No fixture starts.

The [same compiler on `6e9fbae`](https://github.com/astral-sh/imre/actions/runs/40181543433)
checked both source layouts. Each report records four exit codes, the
diagnostics and the compiler digest. Both executed **0 of 272** tests.

| Pinned CPython sources staged | First diagnostic in all four checks |
| --- | --- |
| [Whole `Lib/test`, no extra standard-library files](startup-tests-linux.json) | `test_logging.py:26`, `import codecs`: unsupported standard-library import. This matches the coordinator's test-only source layout. |
| [Whole `Lib/test` and all 720 other Python files in `Lib`](startup-full-lib-linux.json) | `test_logging.py:21`, `import logging`: the CPython source package shadows Imre's module. |

To repeat the first row, run `python3 benchmarks/cpython_stdlib/full/check_suite_startup.py --compiler /path/to/imre --cpython-git /path/to/cpython test.test_logging`.
For the second, also pass `--support-file` for each pinned `.py` outside
`Lib/test`, relative to `Lib`. Codec support is tracked in [#691](https://github.com/astral-sh/imre/pull/691).

There is one starred tuple **display** in the pinned test:
[`ConfigDictTest.test_config_queue_handler`, line 3990](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_logging.py#L3990).
After three ordinary fields it expands `*dqs` (two factory dictionaries from
a list comprehension) and `*qs` (two freshly constructed queue objects). The
result is a seven-element tuple containing the original four objects in order.
Its product with four listener specifications drives 28 configurations.
Different five-element tuples then drive 24 expected configuration errors.

```sh
/path/to/pinned/python -I -B benchmarks/cpython_stdlib/full/logging/queue_display.py
```

The probe verifies the pinned blobs and checks each of the 52 real calls, by
identity, under the original CPython fixtures. This is reference evidence only.
The compiler's separate unpacking regression compares field order, aliases and
exception timing against CPython at `-O0` and `-O3`; it is not a suite ID.
The guard against exponential tuple widths from [#244](https://github.com/astral-sh/imre/pull/244)
still applies. Fixed tuple call arguments ([#527](https://github.com/astral-sh/imre/pull/527))
and list casts following a runtime length guard ([#562](https://github.com/astral-sh/imre/pull/562))
are distinct operations.

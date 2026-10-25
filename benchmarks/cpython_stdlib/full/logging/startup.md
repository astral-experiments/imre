# Importing the unchanged logging suite

The resource-enabled CPython suite discovers [272 IDs](reference-linux.json).
The full native entry imports `test.test_logging`; the runner additionally calls
`unittest.defaultTestLoader.loadTestsFromModule` and `TextTestRunner.run`.
Both are checked with and without `--reachable-only`, with CPython's unmodified
`Lib` as the module root. The first failure occurs before any test fixture.

| Compiler | First diagnostic from all four entries |
| --- | --- |
| Main `76d74f9` | `test_logging.py`: starred elements in tuple displays are unsupported. |
| Main `0ea6595` plus bounded display lowering | Line 22, `import logging.handlers`. |
| Main `e2c5130` (includes [#523](https://github.com/astral-sh/imre/pull/523)) plus bounded display lowering | Line 23, `import logging.config`: standard-library imports and shadowing are unsupported: `"logging"`. |
| Main `32d0289` plus [#588](https://github.com/astral-sh/imre/pull/588) at `5b8ed5b` | Line 23, `import logging.config`, in all four checks. This replay uses main's checked keyword diagnostics and adds only the exact tuple-list subscript diagnostic proved by lowering. |
| Main [`0cf6abb`](https://github.com/astral-sh/imre/commit/0cf6abbf39e6bc6f24c8027716c03e5c4b4a8871), including [#626](https://github.com/astral-sh/imre/pull/626) | Line 26, `import codecs`, in all four checks. The compiler used the exact published source tree. The import still precedes every logging fixture. |

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

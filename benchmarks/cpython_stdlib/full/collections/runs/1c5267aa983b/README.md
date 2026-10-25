# Counting helper on Linux

Imre source `1c5267aa983b6cc2617faf2dfec9d2dce9e7f6b7` builds on the Mapping
dispatch in #642 (`66ae0727b4073ca06a60b248c192f1920cd03865`). All original
files are from CPython `5607950ef232dad16d75c0cf53101d9649d89115`.

The original `TestCounter.test_helper_function` calls the helper with a dict
and an OrderedDict, then constructs two Counter subclasses which override
`__setitem__` or `get`. The host driver passes the exact dict's initial
entries and original keys to the Imre binary. It copies the returned pairs
back to the receiver so the unchanged assertion observes native counts. The
OrderedDict and the two Counter subclasses perform their own operations on
CPython. Perturbing the first native count makes the original assertion fail
at line 2358. The test is mixed at both levels.

- `evidence-linux.json`: all 659 original occurrences at each optimization
  level, including the direct compilation diagnostic beside the mixed result.
- `counter-methods-O*.json`: the direct attempts for all 22 TestCounter methods.
- `count-helper-O*.json`: original method digest, execution locations, call
  counts and the failing control.
- `helper-four-modes.json`: the direct CPython comparison, including a
  generator failure after two mutations and dynamically dispatched native
  mapping overrides.
- `import-boundaries.json`: the public alias and three unknown export or
  attribute controls at both levels under both reachability settings.

Regenerate the raw routine, doctest and subtraction reports with
`routines.py`, `doctests.py` and `counter_host.py` as described in the suite
README; run `count_elements_host.py` with the same tool paths, revision and
levels. Pass both paired host reports to the version of `native_report.py`
recorded in `source.json` (#1041), which validates mixed observations.
Validate the result with `suite_evidence.py reference-linux.json evidence-linux.json`.

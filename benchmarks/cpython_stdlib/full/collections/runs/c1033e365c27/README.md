# Mapping on the stock argparse dependency

The [compiled Mapping fixture](mapping-four-modes.json) produced all 33 expected
observations in all four modes. Separately, CPython 3.14.5 produced nine
observations for the real `_colorize.ThemeSection`: its 13 fields, mapping
order, lookups, membership and the missing-key exception. The
[unchanged consumer](theme-section.json) stopped while importing
`dataclasses.Field` at `Lib/_colorize.py:5` at O0 and O3, with and without
reachability filtering. The native consumer ran no Mapping operation.
Both [unaltered argparse entry points](startup.json) stopped at that import at
O0 and O3 and ran zero tests.

The [per-occurrence reports](evidence-linux.json) cover the full 659
collections occurrences at each optimization level. All 24 original collection
ABC methods are blocked. `TestCollectionABCs.test_Mapping` stopped at the
original `assertIsInstance` on line 1844. No fixture measurement adds a suite
occurrence. The [source receipt](source.json) identifies both parents of this
hosted integration, its Git tree and its compiler and runtime.

# LoggerAdapter.manager

[adapter_properties.py](adapter_properties.py) verifies the pinned
`Lib/logging/__init__.py` blob and extracts the complete decorated getter and
setter without rewriting either method. A native holder and two native adapter
layers exercise identity when reading, replacing, and restoring the shared
manager. The [Linux result](adapter-properties-linux.json) records all five
observations at each optimization level.

This contributes **zero** `test_logging` occurrences. The original
`LoggerAdapterTest.test_nested` also logs through two custom `PrefixAdapter`
objects, inspects a captured record, and replaces the manager with an arbitrary
`object()`. Its real fixture and those operations are still required for that
body to pass.

# Complete CPython test.test_file

Original CPython 3.14.5 at 5607950ef232dad16d75c0cf53101d9649d89115. Public Imre revision 0dfd29b7478757ce95f926590fa80a6ba45ec44f. Each reported lane discovered and ran all 36 original cases in CPython registration order. The test driver prints IDs, then runs unittest against the same suite. See results.json for every disposition, two-level intersections and the earlier boxed O0 comparison. A native process exits 1 when any original fails or errors.

Reproduce against the pinned CPython Lib/test package, staged _pyio.py and the supplied driver. The build and run JSON files contain the exact commands. The ELF digest is in registration.json. Raw stdout and stderr include the full failure tracebacks.

import re_registry
import sys
import unittest
from test import test_re

names = ("test_qualified_re_split", "test_re_split", "test_re_subn")
wanted = tuple("test.test_re.ReTests." + name for name in names)
ldr = unittest.defaultTestLoader
registered = ldr.loadTestsFromModule(test_re)
assert not ldr.errors
count = registered.countTestCases()
print("REGISTERED", count, flush=True)
assert count == 165
print("ORIGINAL_MODULE", test_re.__file__, flush=True)
selected = unittest.TestSuite()
for panel in registered:
    for case in panel:
        ident = case.id()
        print("ID", ident)
        if ident in wanted:
            selected.addTest(case)
if sys.argv[1] == "inverse":
    from test import test_re_split_inverse
    selected = unittest.TestSuite()
    for name in names:
        selected.addTest(test_re_split_inverse.ReTests(name))
assert selected.countTestCases() == 3
for case in selected:
    print("SELECT", case.id())
print("SELECTED", selected.countTestCases(), flush=True)
result = unittest.TextTestRunner(verbosity=2).run(selected)
print("RAN", result.testsRun, flush=True)
assert result.testsRun == 3
raise SystemExit(not result.wasSuccessful())

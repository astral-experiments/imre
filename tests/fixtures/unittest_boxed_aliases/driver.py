import sys
import unittest as ut
import registry

suite = ut.defaultTestLoader.loadTestsFromModule(registry)
result = ut.TextTestRunner(verbosity=2).run(suite)
print('SUMMARY', result.testsRun, len(result.failures), len(result.errors), len(result.skipped), result.wasSuccessful())
for case, reason in result.skipped:
    print('SKIPPED', case.id(), reason)
for case, detail in result.failures:
    print('FAILED', case.id())
if not result.wasSuccessful():
    sys.exit(1)

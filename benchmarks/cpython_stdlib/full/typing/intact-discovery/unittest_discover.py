import unittest

loader = unittest.defaultTestLoader
suite = loader.discover('test', pattern='test_typing.py', top_level_dir='.')
print('DISCOVERED', suite.countTestCases(), flush=True)
for error in loader.errors:
    print('LOAD_ERROR', error, flush=True)
result = unittest.TextTestRunner(verbosity=2, descriptions=False).run(suite)
print('RUNNER_COUNT', result.testsRun, flush=True)
if not result.wasSuccessful():
    raise SystemExit(1)

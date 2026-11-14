import doctest
import json
import unittest


result = unittest.TestResult()
for module in (json, json.encoder, json.decoder):
    doctest.DocTestSuite(module).run(result)
assert result.testsRun == 2
assert result.wasSuccessful(), result.failures

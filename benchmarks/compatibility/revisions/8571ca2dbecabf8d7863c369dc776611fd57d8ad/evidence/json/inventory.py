"""Collect package or finite-launcher original IDs without executing tests."""
import collections
import importlib.util
import json
from pathlib import Path
import sys
import unittest

def cases(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from cases(item)
        else:
            yield item

if sys.argv[1] == 'package':
    suite = unittest.defaultTestLoader.loadTestsFromName('test.test_json')
else:
    path = Path(__file__).with_name('positive_launcher.py')
    spec = importlib.util.spec_from_file_location('positive_launcher', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    suite = module.collect()
seen = collections.Counter()
for test in cases(suite):
    test_id = test.id()
    seen[test_id] += 1
    print(json.dumps({'id': test_id, 'occurrence': seen[test_id]}, sort_keys=True))

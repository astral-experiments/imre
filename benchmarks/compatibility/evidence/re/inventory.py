"""Discover original IDs and occurrences from the pinned upstream suite(s)."""
import collections
import json
import sys
import unittest

family = sys.argv[1]
names = ('os', 'posix', 'genericpath') if family == 'os' else (family,)
counts = collections.Counter()

def cases(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from cases(item)
        else:
            yield item

for name in names:
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromName('test.test_' + name)
    if loader.errors:
        raise RuntimeError(loader.errors)
    for case in cases(suite):
        original_id = case.id()
        counts[original_id] += 1
        row = {'id': original_id, 'occurrence': counts[original_id]}
        if family == 'os':
            row['source'] = 'Lib/test/test_' + name + '.py'
        print(json.dumps(row, sort_keys=True))

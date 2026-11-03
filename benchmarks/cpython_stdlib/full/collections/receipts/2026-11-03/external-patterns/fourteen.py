"""Run exact original methods from the collection-related groups in test_patma."""
import sys
import unittest
from test import test_patma
from events import run

methods = (
    'test_patma_119', 'test_patma_120',
    'test_patma_133', 'test_patma_134', 'test_patma_135',
    'test_patma_184', 'test_patma_185', 'test_patma_186',
    'test_patma_239', 'test_patma_240',
)
inheritance = (
    'test_late_registration_mapping', 'test_late_registration_sequence',
    'test_multiple_inheritance_mapping', 'test_multiple_inheritance_sequence',
)
requested = sys.argv[1:]
group = requested[0] if requested else 'TestPatma'
if group == 'TestPatma':
    allowed = methods
    parent = test_patma.TestPatma
elif group == 'TestInheritance':
    allowed = inheritance
    parent = test_patma.TestInheritance
else:
    raise ValueError(group)
selected = requested[1:] if len(requested) > 1 else list(allowed)
print('MODULE', test_patma.__name__, test_patma.__file__, sep='\t', flush=True)
suite = unittest.TestSuite()
for method in selected:
    assert method in allowed
    case = parent(method)
    assert case.id() == 'test.test_patma.' + group + '.' + method
    suite.addTest(case)
run(suite, len(selected))

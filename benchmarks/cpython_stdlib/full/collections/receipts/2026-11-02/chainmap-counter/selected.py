"""Select one upstream unittest and retain its original guards."""
import sys
import unittest
from events import run

module_name, class_name, method_name = sys.argv[1:]
if module_name == 'test_collections':
    from test import test_collections as module
elif module_name == 'test_deque':
    from test import test_deque as module
elif module_name == 'test_ordered_dict':
    from test import test_ordered_dict as module
elif module_name == 'test_userstring':
    from test import test_userstring as module
else:
    raise ValueError(module_name)
case = getattr(module, class_name)(method_name)
assert case.id() == 'test.' + module_name + '.' + class_name + '.' + method_name
print('MODULE', module.__name__, module.__file__, sep='\t', flush=True)
run(unittest.TestSuite([case]), 1)

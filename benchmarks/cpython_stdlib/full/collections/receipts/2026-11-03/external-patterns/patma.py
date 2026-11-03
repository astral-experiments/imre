"""Run the four unchanged pinned collections ABC pattern inheritance originals."""
import unittest
from test import test_patma
from events import run

case = test_patma.TestInheritance
print('MODULE', test_patma.__name__, test_patma.__file__, sep='\t', flush=True)
run(unittest.defaultTestLoader.loadTestsFromTestCase(case), 4)

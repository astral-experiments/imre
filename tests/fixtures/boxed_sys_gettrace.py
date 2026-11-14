import sys
from sys import gettrace


assert gettrace is sys.gettrace
assert gettrace.__self__ is sys
assert gettrace() is None
sys.gettrace = lambda: 42
try:
    assert sys.gettrace() == 42
    assert gettrace() is None
finally:
    sys.gettrace = gettrace

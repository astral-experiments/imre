import pkg
import pkg.keep
import sys

marker = object()
assert sys.modules[__name__].marker is marker
pkg.attempts += 1
sys.modules[__name__] = pkg.keep
if pkg.attempts == 1:
    raise ValueError('first attempt')

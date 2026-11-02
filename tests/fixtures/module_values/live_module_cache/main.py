import sys
import warnings
from importlib import import_module
from importlib.util import find_spec
from sys import modules
from types import ModuleType


class Replacement:
    __spec__ = "replacement"


assert modules is sys.modules and modules['sys'] is sys
import pkg
original = pkg

class PluginModule(ModuleType):
    def identity(self):
        return self


plugin = PluginModule('pkg')
try:
    modules['pkg'] = plugin
    import pkg as loaded
    assert type(loaded) is PluginModule and loaded.identity() is plugin
    assert import_module('pkg') is plugin
finally:
    modules['pkg'] = original
assert import_module('pkg') is original

modules['pkg'] = Replacement
import pkg as cached
assert cached is Replacement

del modules['pkg']
import pkg as fresh
assert fresh is modules['pkg'] and fresh is not original
modules['pkg'] = None
try:
    import pkg as missing
except ModuleNotFoundError as exc:
    assert exc.name == 'pkg'
else:
    raise AssertionError('None in sys.modules must halt import')
modules['pkg'] = original

alternate = modules.copy()
alternate['only_in_copy'] = Replacement
alternate['pkg'] = Replacement
try:
    sys.modules = alternate
    import pkg as cached
    assert cached is original and __import__('pkg') is original
    assert import_module('pkg') is Replacement and find_spec('pkg') == 'replacement'
    assert find_spec('pkg.keep').name == 'pkg.keep'
    assert __import__('only_in_copy') is Replacement
    del sys.modules
    assert not hasattr(sys, 'modules')
    assert __import__('pkg') is original
    try:
        import_module('pkg')
    except AttributeError:
        pass
    else:
        raise AssertionError('import_module must read the public sys.modules')
finally:
    sys.modules = modules
assert 'only_in_copy' not in modules

try:
    import pkg.retry
except ValueError as exc:
    assert str(exc) == 'first attempt'
else:
    raise AssertionError('the initializer must propagate its failure')
assert 'pkg.retry' not in modules and not hasattr(original, 'retry')
assert original is modules['pkg']
assert original.keep is modules['pkg.keep']

import pkg.retry as result
assert result is original.keep and result is modules['pkg.retry']
assert original.retry is result and original.attempts == 2
import pkg.child
child = modules['pkg.child']
assert not hasattr(original, 'child')
assert modules['pkg'] is original.keep and original.keep.child is child

modules['pkg'] = original
original.replacement = 42
message = "Cannot set an attribute on 'pkg' for child module 'replace_parent'"
bootstrap = modules['_frozen_importlib']
try:
    modules['_frozen_importlib'] = None
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always', ImportWarning)
        child = import_module('pkg.replace_parent')
finally:
    modules['_frozen_importlib'] = bootstrap
assert child is modules['pkg.replace_parent'] and modules['pkg'] == 42
assert len(caught) == 1 and caught[0].category is ImportWarning
assert str(caught[0].message) == message
assert caught[0].filename == '<frozen importlib._bootstrap>' and caught[0].lineno == 1353

modules['pkg'] = original
del modules['pkg.replace_parent']
with warnings.catch_warnings():
    warnings.simplefilter('ignore', ImportWarning)
    warnings.filterwarnings('error', category=ImportWarning, module=r'^importlib[.]_bootstrap$', lineno=1353)
    try:
        import_module('pkg.replace_parent')
    except ImportWarning as exc:
        assert str(exc) == message and isinstance(exc.__context__, AttributeError)
    else:
        raise AssertionError('the active warning filter must take effect')
assert 'pkg.replace_parent' in modules

class RejectingParent:
    def __setattr__(self, name, value):
        raise ValueError('setter rejected child')

modules['pkg'] = original
del modules['pkg.replace_parent']
original.replacement = RejectingParent()
try:
    import_module('pkg.replace_parent')
except ValueError as exc:
    assert str(exc) == 'setter rejected child'
else:
    raise AssertionError('a non-AttributeError from setattr must propagate')
assert 'pkg.replace_parent' in modules
modules['pkg'] = original

assert __import__('returns_none') is None
try:
    import returns_none
except ModuleNotFoundError as exc:
    assert exc.name == 'returns_none'
else:
    raise AssertionError('the next import must observe the None sentinel')
print('live cache ok')

import importlib
import warnings
from sys import modules
from types import ModuleType

bootstrap = importlib._bootstrap


class Loader:
    def __init__(self, fail=False):
        self.depth = 0
        self.fail = fail

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        name = module.__name__
        assert modules[name] is module
        if self.fail:
            raise RuntimeError(name)
        if self.depth == 0:
            self.depth += 1
            try:
                bootstrap._exec(module.__spec__, module)
            finally:
                self.depth -= 1
        else:
            replacement = ModuleType(name)
            modules[name] = replacement


name = 'imre_spec_methods'


def compare_locks():
    first = bootstrap._ModuleLockManager(name)
    same = bootstrap._ModuleLockManager(name)
    different = bootstrap._ModuleLockManager(name + '_other')
    with first, same, different:
        assert first._lock is same._lock
        assert first._lock is not different._lock
    try:
        first.__exit__(None, None, None)
    except RuntimeError:
        pass
    else:
        raise AssertionError('released an unowned lock')


try:
    spec = bootstrap.ModuleSpec(name, Loader())
    module = bootstrap._load(spec)
    assert modules[name] is module
    assert module.__name__ == name and module.__spec__ is None
    assert not spec._initializing
    try:
        bootstrap._exec(spec, ModuleType(name))
    except ImportError as exc:
        assert exc.name == name
    else:
        raise AssertionError('reloaded an unregistered module')
    spec.loader = Loader(fail=True)
    try:
        bootstrap._load(spec)
    except RuntimeError as exc:
        assert exc.args == (name,)
    else:
        raise AssertionError('loader failure was lost')
    assert name not in modules and not spec._initializing

    class Legacy:
        def load_module(self, fullname):
            assert fullname == name
            return modules[fullname]

    spec.loader = Legacy()
    modules[name] = module
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always', ImportWarning)
        assert bootstrap._exec(spec, module) is module
    assert len(caught) == 1 and caught[0].category is ImportWarning
    assert caught[0].lineno == 866
    assert 'falling back to load_module()' in str(caught[0].message)
finally:
    modules.pop(name, None)
compare_locks()
assert name not in bootstrap._module_locks
assert name + '_other' not in bootstrap._module_locks

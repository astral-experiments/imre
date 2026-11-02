import sys
from importlib import import_module
original = sys.modules


class Cache:
    def __init__(self):
        self.data = original.copy()
        self.seen = None

    def get(self, key, default=None):
        value = self.data.get(key, default)
        if key == 'marker':
            self.data[key] = new
        return value

    def __contains__(self, key):
        return key in self.data

    def __getitem__(self, key):
        return self.data[key]

    def __setitem__(self, key, value):
        self.data[key] = value

    def __delitem__(self, key):
        del self.data[key]
        self.seen = sys.exception()
        raise ArithmeticError('during cleanup')


class BrokenSpec:
    @property
    def __spec__(self):
        raise ValueError('failed to read spec')

old = object()
new = object()
proxy = Cache()
proxy.data['marker'] = old
proxy.data['broken_spec'] = BrokenSpec()
try:
    sys.modules = proxy
    assert import_module('marker') is new
    try:
        import_module('broken_spec')
    except ValueError as exc:
        assert str(exc) == 'failed to read spec'
    else:
        raise AssertionError('errors reading the cached spec must propagate')

    try:
        import_module('flaky')
    except ArithmeticError as exc:
        assert isinstance(proxy.seen, ValueError)
        assert exc.__context__ is proxy.seen
        assert str(proxy.seen) == 'during initialization'
    else:
        raise AssertionError('must propagate the cleanup error')
finally:
    sys.modules = original
print('failed initializer is active during cleanup')

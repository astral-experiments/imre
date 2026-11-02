import unittest


class Sample(unittest.TestCase):
    def test_one(self):
        pass


class Loader(unittest.TestLoader):
    pass


class CustomLoader(Loader):
    def loadTestsFromName(self, name, module=None):
        return self.suiteClass([Sample('test_one')])


loader = Loader()
load = loader.loadTestsFromName
suite = load('__main__.Sample.test_one')
assert suite.countTestCases() == 1
assert next(iter(suite)).id() == '__main__.Sample.test_one'
assert loader.loadTestsFromNames([]).countTestCases() == 0
assert CustomLoader().loadTestsFromNames(['first', 'second']).countTestCases() == 2
failed = load('__main__.Sample.missing')
assert len(loader.errors) == 1 and 'Failed to access attribute:' in loader.errors[0]
try:
    next(iter(failed)).missing()
except AttributeError as error:
    assert 'missing' in str(error)
else:
    raise AssertionError('missing name did not raise its deferred error')
print('named loader checked')

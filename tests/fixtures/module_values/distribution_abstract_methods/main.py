from abc import abstractmethod
from importlib.metadata import Distribution
import warnings


class Partial(Distribution):
    def read_text(self, filename):
        return filename


class Complete(Partial):
    def locate_file(self, path):
        return path


class Extended(Complete):
    @abstractmethod
    def extra(self):
        pass


Provider = type('Provider', (Partial,), {})
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter('always')
    provider = Provider()
    complete = Complete()
    Extended()

assert type(provider) is Provider
assert provider.read_text('METADATA') == 'METADATA'
assert provider.locate_file('root') is None
assert complete.locate_file('root') == 'root'
assert [item.category for item in caught] == [DeprecationWarning, DeprecationWarning]
assert str(caught[0].message) == "Unimplemented abstract methods {'locate_file'}"
assert str(caught[1].message) == "Unimplemented abstract methods {'extra'}"

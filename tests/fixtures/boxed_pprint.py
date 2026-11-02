from pprint import PrettyPrinter, pformat, saferepr
from types import MappingProxyType


def current():
    assert pformat({'items': [1, 2, 3]}, width=12) == "{'items': [1,\n           2,\n           3]}"
    assert saferepr([1, 2, 3]) == '[1, 2, 3]'
    printer = PrettyPrinter(width=12)
    assert printer.pformat(MappingProxyType({'a': 'a' * 10})) == "mappingproxy({'a': 'aaaaaaaaaa'})"
    return 0

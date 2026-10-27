import os
import pkgutil
from pkgutil import get_data, iter_modules


def current():
    root = os.path.dirname(__file__)
    class Directory:
        def __fspath__(self):
            return root

    found = [item for item in iter_modules([Directory(), root], 'found.') if item.name == 'found.' + __name__]
    assert len(found) == 1
    finder, name, is_package = found[0]
    assert isinstance(found[0], tuple) and isinstance(found[0], pkgutil.ModuleInfo)
    assert (finder.path, name, is_package) == (root, 'found.' + __name__, False)
    assert get_data(__name__, os.path.basename(__file__))[:10] == b'import os\n'
    assert get_data(package='pkgutil_resource', resource='payload.bin') == b'\x00\xff\r\n'
    assert get_data('missing_pkgutil_fixture_module', 'data') is None
    try:
        get_data(__name__ + '.child', 'unused')
    except ModuleNotFoundError as error:
        assert error.name == __name__ + '.child'
        assert isinstance(error.__cause__, AttributeError)
    else:
        raise AssertionError('a module without __path__ cannot have children')
    try:
        next(iter_modules('one directory'))
    except ValueError as error:
        print(str(error))

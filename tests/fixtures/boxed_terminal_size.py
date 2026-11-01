import os
import shutil
import sys


def current():
    assert sys.__stdout__.fileno() == 1
    # Make fallback deterministic on both terminals and pipes.
    sys.__stdout__ = None
    for columns, lines in [('0', 'bad'), ('132', '-1'), ('', '47'), ('120', '35')]:
        os.environ['COLUMNS'] = columns
        os.environ['LINES'] = lines
        size = shutil.get_terminal_size(fallback=(71, 29))
        assert type(size) is os.terminal_size
        assert isinstance(size, tuple) and issubclass(type(size), tuple)
        assert type(size).__module__ == "os"
        assert size[0] == size.columns and size[1] == size.lines
        print(size)

    # When both dimensions are valid the fallback is never examined.
    assert shutil.get_terminal_size(None) == (120, 35)
    try:
        os.get_terminal_size(-1)
    except OSError as error:
        print(error.errno)
    else:
        raise AssertionError('invalid descriptor accepted')

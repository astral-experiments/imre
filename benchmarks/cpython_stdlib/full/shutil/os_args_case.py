"""Supplemental control for the actual argument objects observed by shutil callers."""
import shutil
import sys


def missing(path: str) -> object:
    try:
        shutil.disk_usage(path)
    except FileNotFoundError as exc:
        print(repr(exc.filename))
        print(len(exc.args))
        print(repr(exc.args[0]))
        print(repr(exc.args[-1]))
        return exc.args
    raise AssertionError('fixture path exists')


def main(path: str) -> None:
    saved = missing(path)
    print(repr(saved))
    print(saved == (2, 'No such file or directory'))
    try:
        raise OSError()
    except OSError as exc:
        print(repr(exc.args))
    try:
        raise FileNotFoundError('')
    except OSError as exc:
        print(repr(exc.args))
    try:
        raise PermissionError(b'argument')
    except OSError as exc:
        print(repr(exc.args))
        print(repr(exc))
        print(str(exc))
    items = [7]
    try:
        raise OSError(items)
    except OSError as exc:
        items.append(11)
        print(repr(exc.args))
        print(repr(exc))
        print(str(exc))


main(sys.argv[1])

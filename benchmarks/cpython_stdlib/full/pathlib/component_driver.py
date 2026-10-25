"""Native worker template. The evidence runner binds PATH_TYPE to the original fixture's class."""
from pathlib import PATH_TYPE
import sys


def raw(i: int) -> bytes:
    with open(sys.argv[1] + '/' + str(i), 'rb') as f:
        return f.read()


def arg(i: int) -> str:
    return raw(i).decode()


def save(paths: list[PATH_TYPE], path: PATH_TYPE) -> int:
    paths.append(path)
    return len(paths) - 1


def replacement() -> str | None:
    if arg(2) == 'none':
        return None
    return arg(1)


def relative(path: PATH_TYPE, paths: list[PATH_TYPE], form: str, walk: bool) -> PATH_TYPE:
    if form == 'missing':
        return path.relative_to()
    if form == 'bytes':
        return path.relative_to(raw(2), walk_up=walk)
    if form == 'path':
        return path.relative_to(paths[int(arg(2))], walk_up=walk)
    return path.relative_to(arg(2), walk_up=walk)


def is_relative(path: PATH_TYPE, paths: list[PATH_TYPE], form: str) -> bool:
    if form == 'missing':
        return path.is_relative_to()
    if form == 'bytes':
        return path.is_relative_to(raw(2))
    if form == 'path':
        return path.is_relative_to(paths[int(arg(2))])
    return path.is_relative_to(arg(2))


paths = [PATH_TYPE()]
print('READY')
while True:
    command = input()
    if command == 'quit':
        break
    try:
        if command == 'new':
            if arg(0) == '0':
                print(save(paths, PATH_TYPE()))
            else:
                print(save(paths, PATH_TYPE(arg(1))))
        else:
            path = paths[int(arg(0))]
            if command == 'text':
                print(repr(str(path)))
            elif command == 'equal':
                print(path == paths[int(arg(1))])
            elif command == 'with_name':
                print(save(paths, path.with_name(replacement())))
            elif command == 'with_stem':
                print(save(paths, path.with_stem(replacement())))
            elif command == 'with_suffix':
                print(save(paths, path.with_suffix(replacement())))
            elif command == 'relative_to':
                print(save(paths, relative(path, paths, arg(1), arg(3) == 'True')))
            elif command == 'is_relative_to':
                print(is_relative(path, paths, arg(1)))
            else:
                raise RuntimeError('unknown native component command')
    except TypeError as error:
        print('!TypeError ' + repr(str(error)))
    except ValueError as error:
        print('!ValueError ' + repr(str(error)))

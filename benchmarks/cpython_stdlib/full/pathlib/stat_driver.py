"""Hold real paths and stat results while the original unittest checks their fields."""
from os import stat_result
from pathlib import Path
import sys


def arg(index: int) -> str:
    with open(sys.argv[1] + '/' + str(index), 'rb') as stream:
        return stream.read().decode()


paths: list[Path] = []
stats: list[stat_result] = []
print('READY')
while True:
    op = input()
    if op == 'quit':
        break
    if op == 'new':
        paths.append(Path(arg(0)))
        print(len(paths) - 1)
    elif op == 'join':
        p = paths[int(arg(0))]
        if int(arg(1)) == 1:
            paths.append(p.joinpath(arg(2)))
        else:
            paths.append(p.joinpath(arg(2), arg(3)))
        print(len(paths) - 1)
    elif op == 'stat':
        stats.append(paths[int(arg(0))].stat())
        print(len(stats) - 1)
    else:
        observed = stats[int(arg(0))]
        if op == 'st_mode':
            print(observed.st_mode)
        elif op == 'st_ino':
            print(observed.st_ino)
        elif op == 'st_dev':
            print(observed.st_dev)
        else:
            raise AssertionError('unknown stat field')

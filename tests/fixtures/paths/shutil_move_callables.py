from pathlib import Path
from typing import Callable
import shutil


def move_path(op: Callable[[Path, Path], str | Path | None], src: Path, dst: Path) -> None:
    result = op(src, dst)
    print(isinstance(result, Path) and result is dst)


def bad_copier() -> int:
    print('evaluated before rename')
    return 42


source = Path('source')
target = Path('target')
source.write_bytes(b'payload')
move_path(shutil.move, source, target)
print(repr(target.read_bytes()))
print(source.exists())
rename: Callable[[str, str], str | None] = shutil.move
print(repr(rename('target', 'renamed')))
inside: Callable[[str, str], bool] = shutil._destinsrc
print(inside('empty-directory', 'empty-directory/child'))
print(repr(shutil.move('renamed', 'renamed-again', copy_function=bad_copier())))
print(repr(rename('empty-directory', 'empty-directory')))
shutil.copytree(Path('empty-directory'), 'empty-tree', copy_function=False)
Path('empty-tree').rmdir()
Path('renamed-again').unlink()

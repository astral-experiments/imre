from pathlib import Path


class Flag:
    def __bool__(self) -> bool:
        print('convert flag')
        return True


def flag() -> Flag:
    print('evaluate flag')
    return Flag()


root = Path('links')
root.mkdir()
target = root / 'file'
target.write_text('first')
link = root / 'symbolic'
link.symlink_to(target_is_directory=flag(), target='./file')
print(str(link.readlink()))
print(link.is_symlink())
hard = root / 'hard'
hard.hardlink_to(link)
print(hard.is_symlink())
hard.write_text('second')
print(target.read_text())
print(link.read_text())

dangling = root / 'dangling'
dangling.symlink_to(b'./missing//')
print(str(dangling.readlink()))
print(dangling.exists())
print(dangling.info.is_symlink())
try:
    dangling.symlink_to(b'./again//')
except FileExistsError as error:
    print(str(error))
    print(repr(error.filename))
    print(repr(error.filename2))
try:
    (root / 'none').hardlink_to(b'./missing//')
except FileNotFoundError as error:
    print(str(error))
    print(repr(error.filename))
    print(repr(error.filename2))
for path in [root / 'absent', target, Path('bad\0path')]:
    try:
        path.readlink()
    except OSError as error:
        print(str(error))
        print(repr(error.filename))
    except ValueError as error:
        print(str(error))
try:
    Path('bad\0dest').symlink_to(target, flag())
except ValueError as error:
    print(str(error))
try:
    Path('bad\0dest').symlink_to(b'bad\0source', flag())
except ValueError as error:
    print(str(error))
try:
    Path('bad\0dest').hardlink_to('bad\0source')
except ValueError as error:
    print(str(error))

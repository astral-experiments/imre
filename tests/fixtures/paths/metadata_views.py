from pathlib import Path
from shutil import copyfile


def main(root: str) -> None:
    base = Path(root)
    p = base / 'missing'
    view = p.info
    print(repr(view))
    print(view is p.info)
    print(view.exists())
    p.write_text('now regular')
    print(view.is_file())
    print(view.exists(follow_symlinks=False))
    print(p.exists())
    fresh = Path(p).info
    print(fresh is view)
    print(fresh.is_file())
    p.unlink()
    p.mkdir()
    print(fresh.is_file())
    print(fresh.is_dir())
    print(p.is_dir())
    print((base / 'missing').info.is_dir())
    p.rmdir()

    # Acquisition snapshots the directory entry type and closes the scan.
    entry_name = base / 'entry'
    entry_name.write_text('regular')
    for entry in base.iterdir():
        if entry.name == 'entry':
            entry.unlink()
            print(entry.info.exists(follow_symlinks=False))
            print(entry.info.is_file(follow_symlinks=False))
            print(entry.info.is_file())
            print(entry.info.exists())
            entry.mkdir()
            print(entry.info.exists())
            print(entry.info.is_file())
            print(entry.info.is_dir())
            print(entry.is_dir())
            entry.rmdir()

    # The fixture provides a symlink named 'dangling.json' whose target is initially missing.
    for link in base.iterdir():
        if link.name == 'dangling.json':
            target = base / 'missing-target'
            print(link.info.is_symlink())
            print(link.info.exists())
            print(link.info.exists(follow_symlinks=False))
            target.write_text('target')
            print(link.info.is_file())
            print(link.info.is_file(follow_symlinks=False))
            target.unlink()
            print(link.info.is_file())
            print(link.is_file())

    # The scan saw a regular file. DirEntry.stat reuses lstat for that kind
    # even when the name has since been replaced with a dangling symlink.
    replaced = base / 'replaced'
    replaced.write_text('old')
    for entry in base.iterdir():
        if entry.name == 'replaced':
            saved = entry.info
            entry.unlink()
            copyfile(str(base / 'dangling.json'), str(entry), follow_symlinks=False)
            print(saved.exists(), saved.exists(follow_symlinks=False))
            print(saved.is_file(), saved.is_symlink())
            print(entry.exists(), entry.is_file(), entry.is_symlink())
            entry.unlink()
            entry.mkdir()
            print(saved.exists(), saved.is_file(), saved.is_dir(), entry.is_dir())
            entry.rmdir()


main('tree')

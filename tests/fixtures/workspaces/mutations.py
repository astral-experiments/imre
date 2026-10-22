from pathlib import Path


def main() -> None:
    paths = [Path("created"), Path("parents/leaf")]
    alias = paths[0]
    print(alias.mkdir() is None)
    print(paths[0].is_dir())
    print(paths[1].mkdir(0o710, True, False) is None)
    print(paths[1].mkdir(exist_ok=True) is None)
    Path("directory-link").mkdir(exist_ok=True)
    Path("directory-link/child").mkdir(parents=True)
    Path(".").mkdir(parents=True, exist_ok=True)
    Path("unicode-λ/🙂").mkdir(parents=True)
    Path("zero-mode").mkdir(0)
    Path("negative-mode").mkdir(-1)
    Path("minimum-mode").mkdir(-2147483648)
    Path("maximum-mode").mkdir(2147483647)

    source = Path("source.txt")
    destination = source.replace(target=Path("destination.txt"))
    print(source)
    print(destination)
    print(source.exists())
    print(repr(destination.read_text(encoding="utf-8")))
    moved = destination.replace("./existing/../final.txt")
    print(moved)
    print(moved == Path("existing/../final.txt"))
    print(Path("final.txt").is_file())

    Path("raw-source").write_text("raw target spelling", encoding="utf-8")
    try:
        Path("raw-source").replace("raw-target/")
    except NotADirectoryError:
        print("string target retains trailing separator")
    print(Path("raw-source").exists())
    print(Path("raw-source").replace(Path("raw-target/")))

    print(Path("hard-a").replace("hard-b"))
    print(Path("hard-a").exists())
    Path("hard-b").write_text("still shared", encoding="utf-8")
    print(Path("hard-a").read_text(encoding="utf-8"))
    Path("file-link").unlink()
    Path("directory-link").unlink(False)
    Path("dangling-link").unlink(missing_ok=True)
    print(Path("plain-file").exists())
    print(Path("existing").is_dir())
    print(Path("absent").unlink(True) is None)
    print(Path("empty").replace("renamed-empty"))
    print(Path("renamed-empty").rmdir() is None)
    print(Path("created").rmdir() is None)


main()

from pathlib import Path


def segment(value: str) -> str:
    print("segment " + value)
    return value


def failed_segment() -> str:
    print("failed segment")
    raise ValueError("failed")


def returned_names() -> tuple[str, str]:
    original = "//host/δοκιμή/a-component-longer-than-inline-storage"
    path = Path(original)
    spelling = str(path)
    component = path.name
    original += "-new"
    changed = path.name
    changed += "!"
    rendered = str(path)
    rendered += "?"
    print(original)
    print(changed)
    print(rendered)
    print(path == Path(spelling))
    return (spelling, component)


spelling, component = returned_names()
print(spelling)
print(component)
for text in ["", ".", "./", "././", "/", "//", "///", "////", "a//b/./c/", "a/../b", "../..", "//host//share/file.tar.gz", "///host/file", ".hidden", "..hidden", ".hidden.ext", "...", "..", "α/λ🙂.json", "a\\b.txt", "a\0b"]:
    path = Path(text)
    print(str(path))
    print(path.as_posix())
    print(path.name)
    print(path.stem)
    print(path.suffix)
    print(str(path.parent))
    print(repr(path))
    print(bool(path))
    print(path == Path(path))
print(str(Path()))
print(str(Path("base", "nested", Path("item.json"))))
print(str(Path("base", "/reset", "last")))
print(str(Path("base") / "child"))
print(str("base" / Path("child")))
print(str(Path("base") / Path("/reset")))
print(str(Path("base") / ""))
print(str(Path("//host") / Path("share")))
print(str(Path(segment("first"), segment("second"), segment("third"))))
print(str(Path(segment("left")) / segment("right")))
paths = [Path("parent")]
print(paths[0].joinpath(paths.pop(), segment("last")))
print(Path("same").joinpath())
try:
    Path(segment("receiver")).joinpath(segment("first"), failed_segment(), segment("skipped"))
except ValueError:
    print("join interrupted")
print(Path("a/./b") == Path("a/b"))
print(Path("a/../b") != Path("b"))
print(Path("//a") != Path("/a"))

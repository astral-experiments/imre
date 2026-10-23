from pathlib import Path


def main() -> None:
    first = Path("specs//./temp/")
    same = Path("specs") / "temp"
    contents: dict[Path, list[int]] = {first: []}
    contents[same].append(3)
    contents[Path("specs/temp/..")] = [4]
    for path, items in contents.items():
        print(str(path))
        print(items[0])
    print(Path("specs") in contents)
    print(same in contents)

    paths = {first, same, Path("//a"), Path("/a"), Path("///a")}
    print(len(paths))
    print(Path("./specs/temp") in paths)
    paths.discard(same)
    print(first in paths)

    pairs = {(Path("./specs"), 1): "old"}
    pairs[(Path("specs"), 1)] = "new"
    print(len(pairs))
    print(pairs[(Path("specs"), 1)])
    del contents[same]
    try:
        contents[first]
    except KeyError as error:
        print(str(error))


main()

from pathlib import Path


def name(path: Path) -> str:
    return path.name


def json_file(path: Path) -> bool:
    return path.suffix == ".json"


print("|".join(sorted(str(path) for path in Path("tree").iterdir())))
print("|".join(sorted(path.name for path in Path("tree").glob("*.json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("?.json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("[ab].json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("[!a-c].json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("[a-z].json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("[[]*.json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("literal[.json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("*.JSON"))))
print("|".join(sorted(path.name for path in Path("tree").glob("α?.json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("*"))))
print("|".join(sorted(path.name for path in Path("tree").glob(".hidden.json"))))
print("|".join(sorted(path.name for path in Path("tree").glob("dangling.json"))))
print("|".join(sorted(map(name, filter(json_file, Path("tree").iterdir())))))
paths: list[Path] = list(Path("tree").glob("[ab].json"))
print("|".join(path.name for path in sorted(paths, key=name)))
print(any(path.name == "a.json" for path in Path("tree").iterdir()))
print(all(path.exists() for path in Path("tree").glob("[ab].json")))
print(len([path for index, path in enumerate(Path("tree").glob("[ab].json"), 7) if index >= 7]))
print(len([path for path, flag in zip(Path("tree").glob("[ab].json"), [True, False], strict=True) if flag]))
print(len([*Path("tree").glob("[ab].json")]))
for path in Path("empty-directory").iterdir():
    print("unreachable empty")
else:
    print("empty else")
for path in Path("tree").glob("*.json"):
    print("break")
    break
else:
    print("unreachable break else")

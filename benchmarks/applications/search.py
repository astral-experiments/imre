import sys


def search(path: str) -> None:
    with open(path, encoding="utf-8") as source:
        text = source.read()
    for number, line in enumerate(text.split("\n"), 1):
        if "TODO" in line:
            print(f"{path}:{number}")


def main() -> None:
    with open(sys.argv[1], encoding="utf-8") as manifest:
        paths = manifest.read().splitlines()
    for path in paths:
        search(path)


if __name__ == "__main__":
    main()

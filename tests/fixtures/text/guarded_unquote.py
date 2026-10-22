from dataclasses import dataclass
from urllib.parse import unquote


@dataclass
class Item:
    url: str
    keep: bool


def read(item: Item) -> str:
    print(item.url)
    if item.url == "fail":
        raise ValueError("source failed")
    return item.url


def collect(items: list[Item]) -> list[str]:
    names: list[str] = []
    for item in items:
        name = unquote(read(item).rsplit("/", 1)[-1])
        item.url = "changed before use"
        if not item.keep:
            continue
        names.append(name)
    return names


def dynamic_separator(items: list[Item], sep: str) -> None:
    for item in items:
        name = unquote(item.url.rsplit(sep, 1)[-1])
        if not item.keep:
            continue
        print(name)


def multiple_reads(items: list[Item]) -> None:
    for item in items:
        name = unquote(item.url.rsplit("/", 1)[-1])
        if not item.keep:
            print(name)
            continue
        print(name)


def direct(items: list[Item]) -> None:
    for item in items:
        name = unquote(read(item))
        item.url = "changed"
        if not item.keep:
            continue
        print(name)


def local_inputs(urls: list[str], replacement: str) -> list[str]:
    names: list[str] = []
    for url in urls:
        stable = unquote(url.rsplit("/", 1)[-1])
        if url.endswith(".sig"):
            continue
        names.append(stable)
    for url in urls:
        original = unquote(url)
        if (url := replacement).endswith(".sig"):
            continue
        names.append(original + "|" + url)
    return names


def main() -> None:
    rows = [Item("x/a%2Fb", True), Item("y/drop%20this", False), Item("z/%FF%00+%ZZ", True)]
    print(repr(collect(rows)))
    try:
        collect([Item("fail", False)])
    except ValueError as error:
        print(str(error))
    try:
        dynamic_separator([Item("skip", False)], "")
    except ValueError as error:
        print(str(error))
    multiple_reads([Item("a/b%20c", False), Item("d/+%2B", True)])
    direct([Item("drop", False), Item("saved%2Ftext", True)])
    print(repr(local_inputs(["x/a%2Fb", "skip.sig", "%FF+%00"], "new")))


main()

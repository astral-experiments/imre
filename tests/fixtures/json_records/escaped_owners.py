from dataclasses import dataclass
from urllib.parse import unquote

from relar.json import decode, encode


@dataclass
class Row:
    index: int
    url: str
    digest: str | None


def make_input() -> str:
    rows: list[Row] = []
    for index in range(96):
        name = "binary%2Bpackage" if index % 2 == 0 else "資料\x00%FF+%ZZ"
        url = f"https://downloads.example/long/path/{name}-{index}"
        rows.append(Row(index, url, f"digest-with-more-than-24-bytes-{index}"))
    return encode(rows)


def index_of(row: Row) -> int:
    return row.index


def take_owners(text: str) -> tuple[Row, Row, str, str]:
    rows = decode(text, list[Row])
    ordered = sorted(rows, key=index_of)
    reverse = rows[::-1]
    first = reverse[-1]
    middle = ordered[33]
    original = middle.url
    filename = unquote(original.rsplit("/", maxsplit=1)[-1])
    middle.url = first.url
    middle.digest = None
    print(rows[33].url == first.url)
    print(reverse[-34].digest is None)
    rows[33] = Row(33, "replacement", None)
    return first, middle, original, filename


first, middle, original, filename = take_owners(make_input())
first.url = "changed after decoding"
print(first.url)
print(first.digest or "")
print(middle.url)
print(middle.digest is None)
print(repr(original))
print(repr(filename))

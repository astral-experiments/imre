from dataclasses import dataclass

from relar.json import decode, encode


@dataclass
class Item:
    name: str
    count: int
    enabled: bool | None = None
    amount: float | None = None


@dataclass
class Table:
    rows: dict[str, list[Item]]


def main() -> None:
    rows = decode('[{"name":"é","count":3,"enabled":false,"amount":-0.0},{"name":"𐀀","count":1000000000000000000000000,"enabled":true}]', list[Item])
    for row in rows:
        print(row.name)
        print(row.count)
    print(encode(rows))
    table = decode('{"rows":{"second":[{"name":"two","count":2}],"first":[]}}', Table)
    print(encode(table, indent=2))
    copied = decode(encode(table.rows), dict[str, list[Item]])
    for key, values in copied.items():
        print(key)
        print(len(values))
    integers = decode('[1,-2,1000000000000000000000000000000]', list[int])
    print(encode(integers))
    empty: dict[str, list[int]] = {}
    print(encode(empty, indent=2))
    print(encode(decode('[null,null]', list[None])))
    for invalid in ['{"x":1,"x":2}', '{"x":null}', '{"x":true}', '{"x":1.5}', '[1]']:
        try:
            decode(invalid, dict[str, int])
        except ValueError:
            print("invalid dictionary")
    for invalid_rows in ['[{"name":"one","count":1},null]', '[{"name":"one","count":1},{"count":2}]', '{}']:
        try:
            decode(invalid_rows, list[Item])
        except ValueError:
            print("invalid record array")
    print(encode(rows, ensure_ascii=False))


main()
